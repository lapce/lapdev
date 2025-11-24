mod hosts;
mod ip_allocator;
mod service_bridge;

pub use hosts::HostsManager;
pub use ip_allocator::SyntheticIpAllocator;
pub use service_bridge::{ServiceBridge, ServiceBridgeStartReport};

use lapdev_devbox_rpc::ServiceInfo;
use std::net::IpAddr;

/// Returns true if the provided protocol string represents TCP.
pub fn is_tcp_protocol(protocol: &str) -> bool {
    let trimmed = protocol.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("tcp")
}

/// A service endpoint with synthetic IP and port
#[derive(Debug, Clone)]
pub struct ServiceEndpoint {
    pub service_name: String,
    pub namespace: String,
    pub port: u16,
    pub protocol: String,
    pub synthetic_ip: IpAddr,
    /// Fully qualified domain name: service.namespace.svc
    pub fqdn: String,
}

impl ServiceEndpoint {
    pub fn new(info: &ServiceInfo, port: u16, protocol: String, synthetic_ip: IpAddr) -> Self {
        let fqdn = format!("{}.{}.svc", info.name, info.namespace);
        let normalized_protocol = if is_tcp_protocol(&protocol) {
            "TCP".to_string()
        } else {
            protocol
        };
        Self {
            service_name: info.name.clone(),
            namespace: info.namespace.clone(),
            port,
            protocol: normalized_protocol,
            synthetic_ip,
            fqdn,
        }
    }

    /// Returns all DNS aliases for this service
    pub fn aliases(&self) -> Vec<String> {
        vec![
            self.service_name.clone(),
            format!("{}.{}", self.service_name, self.namespace),
            format!("{}.{}.svc", self.service_name, self.namespace),
            self.fqdn.clone(),
        ]
    }
}
