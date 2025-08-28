use std::io;
use std::mem;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::fd::AsRawFd;
use tokio::net::TcpStream;

// Linux netfilter constants
const SOL_IP: libc::c_int = 0;
const SO_ORIGINAL_DST: libc::c_int = 80;

#[repr(C)]
struct sockaddr_in {
    sin_family: libc::sa_family_t,
    sin_port: u16,
    sin_addr: libc::in_addr,
    sin_zero: [u8; 8],
}

/// Extract the original destination address from an iptables-redirected TCP connection
/// This uses the SO_ORIGINAL_DST socket option to get the original target before iptables redirect
pub fn get_original_destination(stream: &TcpStream) -> io::Result<SocketAddr> {
    let fd = stream.as_raw_fd();
    let mut addr: sockaddr_in = unsafe { mem::zeroed() };
    let mut len = mem::size_of::<sockaddr_in>() as libc::socklen_t;

    let result = unsafe {
        libc::getsockopt(
            fd,
            SOL_IP,
            SO_ORIGINAL_DST,
            &mut addr as *mut sockaddr_in as *mut libc::c_void,
            &mut len,
        )
    };

    if result != 0 {
        return Err(io::Error::last_os_error());
    }

    // Convert from network byte order
    let port = u16::from_be(addr.sin_port);
    let ip_addr = u32::from_be(addr.sin_addr.s_addr);
    
    let ipv4 = Ipv4Addr::from(ip_addr);
    let socket_addr = SocketAddr::new(IpAddr::V4(ipv4), port);

    Ok(socket_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sockaddr_size() {
        // Ensure our struct matches the expected size
        assert_eq!(mem::size_of::<sockaddr_in>(), 16);
    }
}