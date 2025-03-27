use std::time::{SystemTime, UNIX_EPOCH};

use rand::{distr::Alphanumeric, Rng};
use sha2::{Digest, Sha256};

pub fn rand_string(n: usize) -> String {
    rand::rng()
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

pub fn format_repo_url(repo: &str) -> String {
    let repo = repo.trim().to_lowercase();
    let repo = if !repo.starts_with("http://")
        && !repo.starts_with("https://")
        && !repo.starts_with("ssh://")
    {
        format!("https://{repo}")
    } else {
        repo.to_string()
    };
    repo.strip_suffix('/')
        .map(|r| r.to_string())
        .unwrap_or(repo)
}

pub fn unix_timestamp() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
