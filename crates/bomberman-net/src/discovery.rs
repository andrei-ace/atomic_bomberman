use crate::{interfaces::interfaces, Compatibility, PORT};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

const QUERY: &[u8] = b"ATOMIC-BOMBERMAN-LAN-1";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Advertisement {
    pub name: String,
    pub compatibility: Compatibility,
}
#[derive(Clone, Debug)]
pub struct FoundGame {
    pub address: SocketAddr,
    pub advertisement: Advertisement,
}

/// Bind before announcing a hosted game, so failures reach the UI.
pub async fn bind() -> io::Result<tokio::net::UdpSocket> {
    tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT)).await
}
pub async fn advertise(
    socket: tokio::net::UdpSocket,
    compatibility: Compatibility,
) -> io::Result<()> {
    let reply = serde_json::to_vec(&Advertisement {
        name: "LAN game".into(),
        compatibility,
    })?;
    let mut bytes = [0; 128];
    loop {
        let (len, address) = socket.recv_from(&mut bytes).await?;
        if &bytes[..len] == QUERY {
            socket.send_to(&reply, address).await?;
        }
    }
}
pub fn host_addresses() -> io::Result<Vec<SocketAddr>> {
    Ok(interfaces()?
        .into_iter()
        .filter_map(|i| match i.address {
            IpAddr::V4(ip) if i.broadcast.is_some() => Some(SocketAddr::new(ip.into(), PORT)),
            _ => None,
        })
        .collect())
}

/// Repeated probes use sockets bound to each interface, so default route
/// selection cannot divert subnet broadcasts away from Wi-Fi or Ethernet.
pub fn browse() -> io::Result<Vec<FoundGame>> {
    let mut targets = vec![
        (Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST),
        (Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST),
    ];
    for interface in interfaces()? {
        if let (IpAddr::V4(address), Some(broadcast)) = (interface.address, interface.broadcast) {
            targets.push((address, broadcast));
        }
    }
    let mut sockets = Vec::new();
    let mut last_error = None;
    for (source, target) in targets {
        match (|| -> io::Result<UdpSocket> {
            let socket = UdpSocket::bind((source, 0))?;
            socket.set_broadcast(true)?;
            socket.set_nonblocking(true)?;
            Ok(socket)
        })() {
            Ok(socket) => sockets.push((socket, target)),
            Err(error) => last_error = Some(error),
        }
    }
    let started = Instant::now();
    let mut next_probe = started;
    let mut sent = false;
    let mut network_sent = false;
    let mut games: Vec<FoundGame> = Vec::new();
    let mut bytes = [0; 2048];
    while started.elapsed() < Duration::from_secs(2) {
        if Instant::now() >= next_probe {
            for (socket, target) in &sockets {
                match socket.send_to(QUERY, (*target, PORT)) {
                    Ok(_) => {
                        sent = true;
                        network_sent |= !target.is_loopback();
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            next_probe += Duration::from_millis(300);
        }
        for (socket, _) in &sockets {
            // Bound work even if another application floods UDP.
            for _ in 0..64 {
                match socket.recv_from(&mut bytes) {
                    Ok((len, from)) => {
                        if let Ok(advertisement) =
                            serde_json::from_slice::<Advertisement>(&bytes[..len])
                        {
                            let address = SocketAddr::new(from.ip(), PORT);
                            if from.port() == PORT
                                && from.is_ipv4()
                                && advertisement.name.len() < 64
                                && games.len() < 128
                                && !games.iter().any(|g| g.address == address)
                            {
                                games.push(FoundGame {
                                    address,
                                    advertisement,
                                });
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        last_error = Some(error);
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    if !sent || (!network_sent && last_error.is_some() && games.is_empty()) {
        return Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "No LAN interface available",
            )
        }));
    }
    games.sort_by_key(|game| game.address);
    Ok(games)
}
