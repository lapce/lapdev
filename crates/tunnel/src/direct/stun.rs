use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::Duration,
};

use rand::random;
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{self, timeout, MissedTickBehavior},
};
use tracing::{debug, info, warn};

use super::udp::DirectUdpSocket;

const DEFAULT_STUN_ENDPOINTS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
];
pub(super) const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
pub(super) const STUN_HEADER_SIZE: usize = 20;
pub(super) const STUN_BINDING_REQUEST: u16 = 0x0001;
pub(super) const STUN_BINDING_SUCCESS: u16 = 0x0101;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

pub(super) async fn run_stun_probes(
    socket: Arc<DirectUdpSocket>,
    servers: &[SocketAddr],
    wait_timeout: Duration,
) -> io::Result<Option<SocketAddr>> {
    if servers.is_empty() {
        return Ok(None);
    }

    let mut result = Ok(None);
    for server in servers {
        match send_async_stun_binding_request(Arc::clone(&socket), *server, wait_timeout).await {
            Ok(Some(addr)) => {
                debug!(?server, observed = %addr, "STUN reported public address");
                result = Ok(Some(addr));
                break;
            }
            Ok(None) => {
                debug!(?server, "STUN response missing mapped address attribute");
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                debug!(?server, "STUN probe timed out");
            }
            Err(err) => {
                warn!(?server, error = %err, "STUN probe error");
            }
        }
    }

    result
}

async fn send_async_stun_binding_request(
    socket: Arc<DirectUdpSocket>,
    server: SocketAddr,
    wait_timeout: Duration,
) -> io::Result<Option<SocketAddr>> {
    let transaction_id: [u8; 12] = random();

    let mut request = [0u8; STUN_HEADER_SIZE];
    request[0] = (STUN_BINDING_REQUEST >> 8) as u8;
    request[1] = STUN_BINDING_REQUEST as u8;
    request[2] = 0;
    request[3] = 0;
    request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    request[8..20].copy_from_slice(&transaction_id);

    let response = socket.register_stun_waiter(transaction_id);
    if let Err(err) = socket.send_raw(&request, server).await {
        socket.cancel_stun_waiter(&transaction_id);
        return Err(err);
    }

    match timeout(wait_timeout, response).await {
        Ok(Ok(datagram)) => parse_stun_response(&datagram.data, &transaction_id),
        Ok(Err(_)) => Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "STUN waiter dropped",
        )),
        Err(_) => {
            socket.cancel_stun_waiter(&transaction_id);
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "STUN keepalive timed out",
            ))
        }
    }
}

fn parse_stun_response(data: &[u8], transaction_id: &[u8; 12]) -> io::Result<Option<SocketAddr>> {
    if data.len() < STUN_HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "STUN response too short",
        ));
    }

    let message_type = u16::from_be_bytes([data[0], data[1]]);
    if message_type != STUN_BINDING_SUCCESS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected STUN response type",
        ));
    }

    let message_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data.len() < STUN_HEADER_SIZE + message_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "STUN response truncated",
        ));
    }

    if data[4..8] != STUN_MAGIC_COOKIE.to_be_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid STUN magic cookie",
        ));
    }

    if data[8..20] != transaction_id[..] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mismatched STUN transaction id",
        ));
    }

    let mut offset = STUN_HEADER_SIZE;
    let end = STUN_HEADER_SIZE + message_len;
    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = value_start + attr_len;
        if value_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "STUN attribute exceeds message length",
            ));
        }

        let value = &data[value_start..value_end];
        match attr_type {
            STUN_ATTR_XOR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_xor_mapped_address(value, transaction_id) {
                    return Ok(Some(addr));
                }
            }
            STUN_ATTR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_mapped_address(value) {
                    return Ok(Some(addr));
                }
            }
            _ => {}
        }

        let padding = (4 - (attr_len % 4)) % 4;
        offset = value_end + padding;
    }

    Ok(None)
}

fn parse_mapped_address(value: &[u8]) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let port = u16::from_be_bytes([value[2], value[3]]);
    match family {
        0x01 => {
            if value.len() < 8 {
                return None;
            }
            let addr = Ipv4Addr::new(value[4], value[5], value[6], value[7]);
            Some(SocketAddr::new(IpAddr::V4(addr), port))
        }
        0x02 => {
            if value.len() < 20 {
                return None;
            }
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&value[4..20]);
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(bytes)), port))
        }
        _ => None,
    }
}

fn parse_xor_mapped_address(value: &[u8], transaction_id: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    port ^= (STUN_MAGIC_COOKIE >> 16) as u16;
    match family {
        0x01 => {
            if value.len() < 8 {
                return None;
            }
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            let mut addr = [0u8; 4];
            for i in 0..4 {
                addr[i] = value[4 + i] ^ cookie[i];
            }
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3])),
                port,
            ))
        }
        0x02 => {
            if value.len() < 20 {
                return None;
            }
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            let mut addr = [0u8; 16];
            for i in 0..16 {
                let xor_byte = if i < 4 {
                    cookie[i]
                } else {
                    transaction_id[i - 4]
                };
                addr[i] = value[4 + i] ^ xor_byte;
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr)), port))
        }
        _ => None,
    }
}

pub(super) fn default_stun_servers() -> Vec<SocketAddr> {
    DEFAULT_STUN_ENDPOINTS
        .iter()
        .filter_map(|endpoint| endpoint.to_socket_addrs().ok()?.find(|a| a.is_ipv4()))
        .collect()
}

pub(super) fn start_stun_keepalive(
    socket: Arc<DirectUdpSocket>,
    servers: Vec<SocketAddr>,
    interval: Duration,
    timeout: Duration,
    observed_addr: Arc<Mutex<Option<SocketAddr>>>,
) -> StunKeepaliveHandle {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let mut ticker = time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let task = tokio::spawn({
        let servers = servers.clone();
        let socket = Arc::clone(&socket);
        let observed_addr = Arc::clone(&observed_addr);
        async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    _ = ticker.tick() => {
                        for server in &servers {
                            match send_async_stun_binding_request(Arc::clone(&socket), *server, timeout).await {
                                Ok(Some(addr)) => {
                                    if update_observed_addr(&observed_addr, addr) {
                                        info!(?server, ?addr, "Updated STUN observed address");
                                    }
                                }
                                Ok(None) => {
                                    debug!(?server, "STUN keepalive response missing mapped address");
                                }
                                Err(err) => {
                                    debug!(?server, error = %err, "STUN keepalive request failed");
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    StunKeepaliveHandle {
        shutdown: Some(shutdown_tx),
        task,
    }
}

fn update_observed_addr(
    observed_addr: &Arc<Mutex<Option<SocketAddr>>>,
    new_addr: SocketAddr,
) -> bool {
    let mut guard = observed_addr.lock().expect("observed addr mutex poisoned");
    if guard.map(|current| current != new_addr).unwrap_or(true) {
        *guard = Some(new_addr);
        true
    } else {
        false
    }
}

pub(super) struct StunKeepaliveHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl StunKeepaliveHandle {
    pub(super) fn stop(mut self) {
        info!("StunKeepaliveHandle stop");
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}
