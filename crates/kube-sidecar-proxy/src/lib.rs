pub mod config;
pub mod discovery;
pub mod error;
pub mod http_buffer_parser;
pub mod original_dest;
pub mod otel_routing;
pub mod protocol_detector;
pub mod proxy;
pub mod rpc;
pub mod server;

pub use server::SidecarProxyServer;
