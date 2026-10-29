//! Enumerate real interfaces without assuming names, masks, or a default route.
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub address: IpAddr,
    pub broadcast: Option<Ipv4Addr>,
}

pub fn interfaces() -> io::Result<Vec<Interface>> {
    let mut head = std::ptr::null_mut();
    // getifaddrs owns this list until freeifaddrs. Family checks precede every
    // sockaddr cast, and no pointer escapes the traversal.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut result = Vec::new();
    let mut next = head;
    while !next.is_null() {
        let item = unsafe { &*next };
        next = item.ifa_next;
        if item.ifa_addr.is_null()
            || item.ifa_flags & libc::IFF_UP as u32 == 0
            || item.ifa_flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 != 0
        {
            continue;
        }
        let family = unsafe { (*item.ifa_addr).sa_family as i32 };
        let interface = match family {
            libc::AF_INET => {
                let ip = unsafe { &*item.ifa_addr.cast::<libc::sockaddr_in>() };
                let address = Ipv4Addr::from(ip.sin_addr.s_addr.to_ne_bytes());
                let broadcast = if !item.ifa_netmask.is_null()
                    && item.ifa_flags & libc::IFF_BROADCAST as u32 != 0
                {
                    let mask = unsafe { &*item.ifa_netmask.cast::<libc::sockaddr_in>() };
                    Some(subnet_broadcast(
                        address,
                        Ipv4Addr::from(mask.sin_addr.s_addr.to_ne_bytes()),
                    ))
                } else {
                    None
                };
                Interface {
                    address: address.into(),
                    broadcast,
                }
            }
            libc::AF_INET6 => {
                let ip = unsafe { &*item.ifa_addr.cast::<libc::sockaddr_in6>() };
                Interface {
                    address: Ipv6Addr::from(ip.sin6_addr.s6_addr).into(),
                    broadcast: None,
                }
            }
            _ => continue,
        };
        if !result.contains(&interface) {
            result.push(interface);
        }
    }
    unsafe { libc::freeifaddrs(head) };
    Ok(result)
}

pub fn subnet_broadcast(address: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(address) | !u32::from(mask))
}

/// Only directly assigned public addresses are advertised automatically. A
/// NAT or ambiguous multi-address setup must supply --advertise explicitly.
pub fn public_address(addresses: &[Interface]) -> io::Result<IpAddr> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for interface in addresses {
        match interface.address {
            IpAddr::V4(ip) if public_v4(ip) => {
                if !v4.contains(&ip) {
                    v4.push(ip);
                }
            }
            IpAddr::V6(ip)
                if ip.segments()[0] & 0xe000 == 0x2000
                    && ip.segments()[..2] != [0x2001, 0x0db8] =>
            {
                if !v6.contains(&ip) {
                    v6.push(ip);
                }
            }
            _ => {}
        }
    }
    match (v4.as_slice(), v6.as_slice()) {
        ([ip], _) => Ok((*ip).into()),
        ([], [ip]) => Ok((*ip).into()),
        _ => Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "Cannot identify one public IP; specify --advertise <host:port>",
        )),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_documentation()
        && !ip.is_broadcast()
        && !ip.is_multicast()
        && a != 0
        && a < 240
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 198 && (b == 18 || b == 19))
        && !(a == 192 && b == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn broadcast_uses_actual_subnet_and_public_detection_rejects_nat_and_ambiguity() {
        assert_eq!(
            subnet_broadcast(
                "10.2.3.218".parse().unwrap(),
                "255.255.252.0".parse().unwrap()
            ),
            "10.2.3.255".parse::<Ipv4Addr>().unwrap()
        );
        let entry = |ip: &str| Interface {
            address: ip.parse().unwrap(),
            broadcast: None,
        };
        assert!(public_address(&[entry("192.168.1.2"), entry("100.64.0.1")]).is_err());
        assert_eq!(
            public_address(&[entry("8.8.8.8"), entry("2001:4860::1")]).unwrap(),
            "8.8.8.8".parse::<IpAddr>().unwrap()
        );
        assert!(public_address(&[entry("8.8.8.8"), entry("8.8.4.4")]).is_err());
        assert_eq!(
            public_address(&[entry("fe80::1"), entry("2001:4860::1")]).unwrap(),
            "2001:4860::1".parse::<IpAddr>().unwrap()
        );
    }
}
