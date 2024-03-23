use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};

pub fn rand_string(n: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(n)
        .map(|i| i as char)
        .collect::<String>()
        .to_lowercase()
}

pub fn sha256(input: &str) -> String {
    let hash = Sha256::digest(input.as_bytes());
    base16ct::lower::encode_string(&hash)
}
