pub mod server;
pub mod tunnel;
pub mod preview_url;
pub mod http_proxy;

use anyhow::Result;
use std::sync::Arc;
use lapdev_db::api::DbApi;
use crate::{http_proxy::PreviewUrlProxy, tunnel::TunnelRegistry};

pub async fn run() -> Result<()> {
    Ok(())
}

/// Start the standalone PreviewUrlProxy TCP server
pub async fn start_preview_url_proxy_server(
    db: DbApi,
    tunnel_registry: Arc<TunnelRegistry>,
    bind_addr: &str,
) -> Result<()> {
    let proxy = Arc::new(PreviewUrlProxy::new(db, tunnel_registry));
    
    tracing::info!("Starting PreviewUrlProxy TCP server on {}", bind_addr);
    
    proxy.start_tcp_server(bind_addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start preview URL proxy server: {}", e))?;
    
    Ok(())
}
