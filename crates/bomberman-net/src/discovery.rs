use crate::{interfaces::interfaces, Compatibility, PORT};
use serde::{Deserialize, Serialize};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

pub(crate) const QUERY: &[u8] = b"ATOMIC-BOMBERMAN-LAN-1";
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

fn found_game(from: SocketAddr, bytes: &[u8]) -> Option<FoundGame> {
    if from.port() != PORT || !from.is_ipv4() || from.ip().is_loopback() {
        return None;
    }
    let advertisement: Advertisement = serde_json::from_slice(bytes).ok()?;
    if advertisement.name.len() >= 64 {
        return None;
    }
    Some(FoundGame {
        address: from,
        advertisement,
    })
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
    let mut targets = vec![(Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST)];
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
                        if let Some(game) = found_game(from, &bytes[..len]) {
                            if games.len() < 128 && !games.iter().any(|g| g.address == game.address)
                            {
                                games.push(game);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_host_does_not_appear_as_both_loopback_and_its_network_adapter() {
        let reply = serde_json::to_vec(&Advertisement {
            name: "LAN game".into(),
            compatibility: Compatibility::new("a".repeat(64)),
        })
        .unwrap();
        let addresses = [
            "127.0.0.1:1997",
            "127.10.0.1:1997",
            "[::1]:1997",
            "192.168.50.4:1997",
        ];
        let games: Vec<_> = addresses
            .iter()
            .filter_map(|address| found_game(address.parse().unwrap(), &reply))
            .collect();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].address, "192.168.50.4:1997".parse().unwrap());
        assert!(found_game("192.168.50.4:1998".parse().unwrap(), &reply).is_none());
    }
}
