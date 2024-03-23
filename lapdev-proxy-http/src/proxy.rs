use lapdev_db::{api::DbApi, entities};

pub async fn forward_workspace(
    hostname: String,
    db: &DbApi,
) -> Option<(
    entities::workspace::Model,
    entities::workspace_host::Model,
    Option<entities::workspace_port::Model>,
)> {
    let prefix = hostname.split('.').next()?;
    {
        let mut parts = prefix.splitn(2, '-');
        let port = parts.next().and_then(|p| p.parse::<u16>().ok());
        if let Some(port) = port {
            if let Some(ws_name) = parts.next() {
                if let Ok(ws) = db.get_workspace_by_name(ws_name).await {
                    let workspace_host = db.get_workspace_host(ws.host_id).await.ok()??;
                    let port = db.get_workspace_port(ws.id, port).await.ok()??;
                    return Some((ws, workspace_host, Some(port)));
                }
            }
        }
    }
    let ws = db.get_workspace_by_name(prefix).await.ok()?;
    let workspace_host = db.get_workspace_host(ws.host_id).await.ok()??;
    Some((ws, workspace_host, None))
}
