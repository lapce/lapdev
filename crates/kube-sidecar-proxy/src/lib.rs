pub mod config;
pub mod connection_registry;
pub mod error;
pub mod http2_client;
pub mod http2_proxy;
pub mod otel_routing;
pub mod protocol_detector;
pub mod rpc;
pub mod server;

pub use server::SidecarProxyServer;
