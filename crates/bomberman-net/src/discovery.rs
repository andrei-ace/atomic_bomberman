use crate::{Compatibility, PORT};
use serde::{Deserialize, Serialize};
use std::{
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

pub async fn advertise(compatibility: Compatibility) -> std::io::Result<()> {
    let socket = tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT)).await?;
    let reply = serde_json::to_vec(&Advertisement {
        name: "Atomic Bomberman LAN game".into(),
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
/// Runs off the render thread. No interface names or saved endpoints are needed.
pub fn browse() -> std::io::Result<Vec<FoundGame>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(150)))?;
    for ip in [Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST] {
        let _ = socket.send_to(QUERY, (ip, PORT));
    }
    let started = Instant::now();
    let mut games: Vec<FoundGame> = Vec::new();
    let mut bytes = [0; 2048];
    while started.elapsed() < Duration::from_secs(2) {
        if let Ok((len, from)) = socket.recv_from(&mut bytes) {
            if let Ok(advertisement) = serde_json::from_slice::<Advertisement>(&bytes[..len]) {
                let address = SocketAddr::new(
                    IpAddr::V4(match from.ip() {
                        IpAddr::V4(ip) => ip,
                        _ => continue,
                    }),
                    PORT,
                );
                if !games.iter().any(|game| game.address == address)
                    && advertisement.name.len() < 64
                {
                    games.push(FoundGame {
                        address,
                        advertisement,
                    });
                }
            }
        }
    }
    Ok(games)
}
