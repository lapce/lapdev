use std::time::{SystemTime, UNIX_EPOCH};

use crate::LAPDEV_API_HOST;

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

/// Resolve the Lapdev API host from an optional input string.
///
/// Accepts raw values that may include schemes (http/https/ws/wss),
/// trailing slashes, or the `/api` & `/api/v1` suffixes. Returns the
/// canonical host form (no scheme, no path). Falls back to the default
/// host when the input is `None` or empty.
pub fn resolve_api_host(raw: Option<&str>) -> String {
    let candidate = raw
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let mut host = match candidate {
        Some(value) => value,
        None => return LAPDEV_API_HOST.to_string(),
    };

    for prefix in ["https://", "http://", "wss://", "ws://"] {
        if let Some(stripped) = host.strip_prefix(prefix) {
            host = stripped.to_string();
            break;
        }
    }

    host = host
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string();

    for suffix in ["/api/v1", "/api"] {
        if let Some(stripped) = host.strip_suffix(suffix) {
            host = stripped.trim_end_matches('/').to_string();
            break;
        }
    }

    if host.is_empty() {
        LAPDEV_API_HOST.to_string()
    } else {
        host
    }
}
