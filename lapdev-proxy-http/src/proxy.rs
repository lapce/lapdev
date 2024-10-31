use lapdev_db::{
    api::DbApi,
    entities::{self},
};

pub enum WorkspaceForwardError {
    WorkspaceNotFound,
    PortNotForwarded,
    InvalidHostname,
    Unauthorised,
}

pub async fn forward_workspace(
    hostname: &str,
    db: &DbApi,
    user: Option<&entities::user::Model>,
) -> Result<
    (
        entities::workspace::Model,
        Option<entities::workspace_port::Model>,
    ),
    WorkspaceForwardError,
> {
    let prefix = hostname
        .split('.')
        .next()
        .ok_or(WorkspaceForwardError::InvalidHostname)?;

    {
        let mut parts = prefix.splitn(2, '-');
        let port = parts.next().and_then(|p| p.parse::<u16>().ok());
        if let Some(port) = port {
            if let Some(ws_name) = parts.next() {
                if let Ok(ws) = db.get_workspace_by_name(ws_name).await {
                    let port = db.get_workspace_port(ws.id, port).await.ok().flatten();
                    if port.as_ref().map(|p| p.public) == Some(true) {
                        return Ok((ws, port));
                    }
                    if let Some(user) = user {
                        if user.id == ws.user_id {
                            match port {
                                Some(port) => {
                                    return Ok((ws, Some(port)));
                                }
                                None => return Err(WorkspaceForwardError::PortNotForwarded),
                            }
                        } else if db
                            .get_organization_member(user.id, ws.organization_id)
                            .await
                            .is_ok()
                        {
                            match port {
                                Some(port) => {
                                    if port.shared {
                                        return Ok((ws, Some(port)));
                                    } else {
                                        return Err(WorkspaceForwardError::Unauthorised);
                                    }
                                }
                                None => return Err(WorkspaceForwardError::PortNotForwarded),
                            }
                        }
                    }
                }
            }
        }
    }

    let ws = db
        .get_workspace_by_name(prefix)
        .await
        .map_err(|_| WorkspaceForwardError::WorkspaceNotFound)?;
    if let Some(user) = user {
        if user.id == ws.user_id {
            return Ok((ws, None));
        }
    }
    Err(WorkspaceForwardError::Unauthorised)
}
