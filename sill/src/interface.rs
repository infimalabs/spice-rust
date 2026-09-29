//! Addresses assigned to this host's network interfaces.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// Every IPv4 and IPv6 address assigned to a local interface, with port zero.
///
/// A link-local IPv6 address keeps its scope, without which a connection to
/// it names no interface. Interfaces that carry no address, or an address of
/// another family, contribute nothing.
///
/// # Errors
///
/// Returns the operating system's error when the interface table cannot be
/// read.
pub fn local_addresses() -> io::Result<Vec<SocketAddr>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `head` is a valid destination for the list pointer; on success
    // the list is released below with `freeifaddrs`, exactly once.
    if unsafe { libc::getifaddrs(&raw mut head) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut addresses = Vec::new();
    let mut entry = head;
    while !entry.is_null() {
        // SAFETY: `entry` is a non-null node of the list `getifaddrs` returned,
        // which stays allocated until `freeifaddrs` below.
        let node = unsafe { &*entry };
        if let Some(address) = socket_address(node.ifa_addr) {
            addresses.push(address);
        }
        entry = node.ifa_next;
    }
    // SAFETY: `head` came from a successful `getifaddrs` and is freed once;
    // no reference into the list outlives this call.
    unsafe { libc::freeifaddrs(head) };
    Ok(addresses)
}

fn socket_address(address: *const libc::sockaddr) -> Option<SocketAddr> {
    if address.is_null() {
        return None;
    }
    // SAFETY: a non-null interface address points at a sockaddr whose family
    // field is always readable.
    let family = libc::c_int::from(unsafe { (*address).sa_family });
    match family {
        libc::AF_INET => {
            // SAFETY: an AF_INET address is a complete `sockaddr_in`.
            let inet = unsafe { &*address.cast::<libc::sockaddr_in>() };
            Some(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::from(u32::from_be(inet.sin_addr.s_addr)),
                0,
            )))
        }
        libc::AF_INET6 => {
            // SAFETY: an AF_INET6 address is a complete `sockaddr_in6`.
            let inet6 = unsafe { &*address.cast::<libc::sockaddr_in6>() };
            Some(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(inet6.sin6_addr.s6_addr),
                0,
                0,
                inet6.sin6_scope_id,
            )))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::local_addresses;

    #[test]
    fn the_loopback_interface_is_listed() {
        let addresses = local_addresses().expect("the interface table reads");
        assert!(
            addresses.iter().any(|address| address.ip().is_loopback()),
            "{addresses:?}"
        );
    }
}
