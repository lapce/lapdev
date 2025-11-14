use std::{
    net::{IpAddr, SocketAddr},
    time::{SystemTime, UNIX_EPOCH},
};

use rand::{rng, rngs::ThreadRng, RngCore};

/// Length of the TLS-style record header prepended to every probe.
const TLS_RECORD_HEADER_LEN: usize = 5;
const TLS_APPLICATION_DATA_TYPE: u8 = 0x17;
const TLS_VERSION: [u8; 2] = [0x03, 0x03];

const MIN_BODY_LEN: usize = 48;
const MAX_BODY_LEN: usize = 128;
const METADATA_LEN: usize = 1    // version
    + 1                         // flags
    + 4                         // timestamp seconds
    + 2                         // sender port
    + 2                         // target port
    + 16; // nonce derived from sender + randomness

/// Builds a TLS-looking probe payload which still carries some identifying metadata.
pub(super) fn build_probe_payload(sender: SocketAddr, target: SocketAddr) -> Vec<u8> {
    let mut rng = rng();
    let span = (MAX_BODY_LEN - MIN_BODY_LEN + 1) as u32;
    let body_len = MIN_BODY_LEN + (rng.next_u32() % span) as usize;

    let mut body = vec![0u8; body_len];
    rng.fill_bytes(&mut body);

    let metadata = ProbeMetadata::new(sender, target, &mut rng);
    metadata.mix_into(&mut body);

    let mut packet = Vec::with_capacity(TLS_RECORD_HEADER_LEN + body_len);
    packet.push(TLS_APPLICATION_DATA_TYPE);
    packet.extend_from_slice(&TLS_VERSION);
    packet.extend_from_slice(&(body_len as u16).to_be_bytes());
    packet.extend_from_slice(&body);
    packet
}

struct ProbeMetadata {
    timestamp_secs: u32,
    sender_port: u16,
    target_port: u16,
    nonce: [u8; 16],
}

impl ProbeMetadata {
    fn new(sender: SocketAddr, target: SocketAddr, rng: &mut ThreadRng) -> Self {
        let timestamp_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .min(u32::MAX as u64) as u32;

        let mut nonce = [0u8; 16];
        rng.fill_bytes(&mut nonce);
        let mapped = match sender.ip() {
            IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
            IpAddr::V6(v6) => v6.octets(),
        };
        for (idx, byte) in mapped.iter().take(nonce.len()).enumerate() {
            nonce[idx] ^= byte;
        }

        Self {
            timestamp_secs,
            sender_port: sender.port(),
            target_port: target.port(),
            nonce,
        }
    }

    fn mix_into(&self, body: &mut [u8]) {
        if body.len() < METADATA_LEN {
            return;
        }

        let mut encoded = [0u8; METADATA_LEN];
        encoded[0] = 1; // version
        encoded[1] = 0; // reserved flags
        encoded[2..6].copy_from_slice(&self.timestamp_secs.to_be_bytes());
        encoded[6..8].copy_from_slice(&self.sender_port.to_be_bytes());
        encoded[8..10].copy_from_slice(&self.target_port.to_be_bytes());
        encoded[10..].copy_from_slice(&self.nonce);

        let offset = body.len() - METADATA_LEN;
        for (idx, byte) in encoded.iter().enumerate() {
            body[offset + idx] ^= byte;
        }
    }
}
