use serde::{Deserialize, Serialize};

/// Abstraction representing the remote address a tunnel should connect to.
#[derive(Debug, Clone)]
pub struct TunnelTarget {
    pub host: String,
    pub port: u16,
}

impl TunnelTarget {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Open {
        tunnel_id: String,
        protocol: Protocol,
        target: Target,
    },
    OpenResult {
        tunnel_id: String,
        success: bool,
        error: Option<String>,
    },
    Data {
        tunnel_id: String,
        payload: Vec<u8>,
    },
    Close {
        tunnel_id: String,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub host: String,
    pub port: u16,
}
