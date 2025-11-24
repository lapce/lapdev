use std::net::{IpAddr, SocketAddr};

use rand::{rng, RngCore};

use super::stun::{
    STUN_BINDING_REQUEST, STUN_BINDING_SUCCESS, STUN_HEADER_SIZE, STUN_MAGIC_COOKIE,
};

const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// Build a STUN Binding Success response carrying an XOR-MAPPED-ADDRESS payload.
pub(super) fn build_response_probe_payload(target: SocketAddr) -> Vec<u8> {
    let mut rng = rng();
    let mut transaction_id = [0u8; 12];
    rng.fill_bytes(&mut transaction_id);

    let attr = encode_xor_mapped_address(target, &transaction_id);
    let mut packet = vec![0u8; STUN_HEADER_SIZE + attr.len()];
    packet[..2].copy_from_slice(&STUN_BINDING_SUCCESS.to_be_bytes());
    packet[2..4].copy_from_slice(&(attr.len() as u16).to_be_bytes());
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet[STUN_HEADER_SIZE..].copy_from_slice(&attr);
    packet
}

/// Build a STUN Binding Request payload.
pub(super) fn build_request_probe_payload() -> Vec<u8> {
    let mut rng = rng();
    let mut transaction_id = [0u8; 12];
    rng.fill_bytes(&mut transaction_id);

    let mut packet = vec![0u8; STUN_HEADER_SIZE];
    packet[..2].copy_from_slice(&STUN_BINDING_REQUEST.to_be_bytes());
    // Message length remains zero
    packet[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&transaction_id);
    packet
}

fn encode_xor_mapped_address(addr: SocketAddr, transaction_id: &[u8; 12]) -> Vec<u8> {
    let (family, raw_ip_bytes, port_bytes) = match addr.ip() {
        IpAddr::V4(v4) => {
            let port = addr.port() ^ (STUN_MAGIC_COOKIE >> 16) as u16;
            let mut ip_bytes = v4.octets();
            for (idx, byte) in STUN_MAGIC_COOKIE.to_be_bytes().iter().enumerate() {
                ip_bytes[idx] ^= byte;
            }
            (0x01, ip_bytes.to_vec(), port.to_be_bytes())
        }
        IpAddr::V6(v6) => {
            let port = addr.port() ^ (STUN_MAGIC_COOKIE >> 16) as u16;
            let mut ip_bytes = v6.octets();
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            for idx in 0..16 {
                let xor_byte = if idx < 4 {
                    cookie[idx]
                } else {
                    transaction_id[idx - 4]
                };
                ip_bytes[idx] ^= xor_byte;
            }
            (0x02, ip_bytes.to_vec(), port.to_be_bytes())
        }
    };

    let value_len = 4 + raw_ip_bytes.len();
    let mut attr = vec![0u8; 4 + value_len];
    attr[..2].copy_from_slice(&STUN_ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
    attr[2..4].copy_from_slice(&(value_len as u16).to_be_bytes());
    attr[4] = 0;
    attr[5] = family;
    attr[6..8].copy_from_slice(&port_bytes);
    attr[8..8 + raw_ip_bytes.len()].copy_from_slice(&raw_ip_bytes);
    attr
}
