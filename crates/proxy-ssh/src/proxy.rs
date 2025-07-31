use std::sync::Arc;

use anyhow::{anyhow, Result};
use lapdev_common::WorkspaceStatus;
use lapdev_conductor::Conductor;
use lapdev_db::api::DbApi;
use russh::keys::{decode_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyBase64};
use russh::{
    server::{Auth, Msg, Session},
    Channel, ChannelMsg,
};
use tracing::{debug, info};

use crate::client::handle_client_msg;

pub struct SshProxy {
    pub id: usize,
    pub db: DbApi,
    pub conductor: Arc<Conductor>,
}

pub struct SshProxyHandler {
    #[allow(dead_code)]
    id: usize,
    ws_addr: Option<(String, u16)>,
    ws_session: Option<super::client::ClientSession>,
    ws_private_key: Option<PrivateKeyWithHashAlg>,
    ws_env: Vec<(String, String)>,
    db: DbApi,
    conductor: Arc<Conductor>,
}

impl russh::server::Server for SshProxy {
    type Handler = SshProxyHandler;

    fn new_client(&mut self, _peer_addr: Option<std::net::SocketAddr>) -> Self::Handler {
        self.id = self.id.saturating_add(1);
        SshProxyHandler {
            id: self.id,
            ws_addr: None,
            ws_session: None,
            ws_private_key: None,
            ws_env: Vec::new(),
            db: self.db.clone(),
            conductor: self.conductor.clone(),
        }
    }
}

impl russh::server::Handler for SshProxyHandler {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Reject {
            proceed_with_methods: None,
            partial_success: false,
        })
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _public_key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &russh::keys::PublicKey,
    ) -> Result<Auth, Self::Error> {
        let public_key = public_key.public_key_base64();
        println!("auth public key");
        let ws = self.db.get_workspace_by_name(user).await?;
        let workspace_host = self
            .db
            .get_workspace_host(ws.host_id)
            .await?
            .ok_or_else(|| anyhow!("can't find workspace host"))?;
        self.db
            .validate_ssh_public_key(ws.user_id, &public_key)
            .await?;
        self.ws_addr = Some((
            workspace_host.host,
            ws.ssh_port
                .ok_or_else(|| anyhow!("the workspace doesn't have a ssh port"))?
                as u16,
        ));
        self.ws_private_key = Some(PrivateKeyWithHashAlg::new(
            Arc::new(decode_secret_key(&ws.ssh_private_key, None)?),
            Some(HashAlg::Sha512),
        ));
        if let Some(Ok(env)) = ws.env.as_ref().map(|env| serde_json::from_str(env)) {
            self.ws_env = env;
        }

        if ws.status == WorkspaceStatus::Stopped.to_string()
            && self.conductor.enterprise.has_valid_license().await
            && self
                .conductor
                .enterprise
                .auto_start_stop
                .can_workspace_auto_start(&ws)
                .await
                .unwrap_or(false)
        {
            let ws = if ws.is_compose {
                if let Some(parent) = ws.compose_parent {
                    self.db.get_workspace(parent).await?
                } else {
                    ws
                }
            } else {
                ws
            };
            tracing::info!("auto start workspace {}", ws.name);
            let _ = self.conductor.start_workspace(ws, true, None, None).await;
        }

        Ok(Auth::Accept)
    }

    async fn auth_succeeded(&mut self, _session: &mut Session) -> Result<(), Self::Error> {
        println!("auth succedded");
        let (addr, port) = self
            .ws_addr
            .as_ref()
            .ok_or_else(|| anyhow!("it doesn't have workspace ssh addr"))?;
        let addr = format!("{addr}:{port}");
        let key = self
            .ws_private_key
            .as_ref()
            .ok_or_else(|| anyhow!("it doesn't have workspace private key"))?;
        match super::client::ClientSession::connect(&addr, key).await {
            Ok(session) => {
                self.ws_session = Some(session);
            }
            Err(e) => {
                println!("error connection: {e}");
            }
        }
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let ws_session = self
            .ws_session
            .as_mut()
            .ok_or_else(|| anyhow!("don't have ws session"))?;
        let ws_channel = ws_session.handle.channel_open_session().await?;
        for (name, value) in &self.ws_env {
            let _ = ws_channel.set_env(false, name, value).await;
        }

        let server_handle = session.handle();

        tokio::spawn(async move {
            let _ = forward_server_client(channel, ws_channel, server_handle).await;
        });

        Ok(true)
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        originator_address: &str,
        originator_port: u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let ws_session = self
            .ws_session
            .as_mut()
            .ok_or_else(|| anyhow!("don't have ws session"))?;
        let ws_channel = ws_session
            .handle
            .channel_open_direct_tcpip(
                host_to_connect,
                port_to_connect,
                originator_address,
                originator_port,
            )
            .await?;

        info!(
            "channel {} open direct tcpip from {originator_address}:{originator_port} to {host_to_connect}:{port_to_connect}",
            channel.id()
        );

        let server_handle = session.handle();

        tokio::spawn(async move {
            let _ = forward_server_client(channel, ws_channel, server_handle).await;
        });

        Ok(true)
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let address = address.to_string();
        let port = *port;

        info!("channel {} tcpip forward to {address}:{port}", self.id);

        if let Some(ws_session) = self.ws_session.as_mut() {
            ws_session.handle.tcpip_forward(address, port).await?;
        }

        session.request_success();

        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        address: &str,
        port: u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let address = address.to_string();

        info!(
            "channel {} cancel tcpip forward to {address}:{port}",
            self.id
        );

        if let Some(ws_session) = self.ws_session.as_mut() {
            ws_session
                .handle
                .cancel_tcpip_forward(address, port)
                .await?;
        }

        session.request_success();

        Ok(true)
    }

    async fn channel_open_x11(
        &mut self,
        channel: Channel<Msg>,
        originator_address: &str,
        originator_port: u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let ws_session = self
            .ws_session
            .as_mut()
            .ok_or_else(|| anyhow!("don't have ws session"))?;
        let ws_channel = ws_session
            .handle
            .channel_open_x11(originator_address, originator_port)
            .await?;

        let server_handle = session.handle();

        tokio::spawn(async move {
            let _ = forward_server_client(channel, ws_channel, server_handle).await;
        });

        Ok(true)
    }
}

async fn forward_server_client(
    mut channel: Channel<russh::server::Msg>,
    mut ws_channel: Channel<russh::client::Msg>,
    server_handle: russh::server::Handle,
) -> Result<()> {
    let channel_id = channel.id();
    info!("proxy connection started channel {channel_id}");
    let ws_channel_id = ws_channel.id();
    let mut channel_eof = false;
    let mut ws_channel_eof = false;
    loop {
        tokio::select! {
            msg = channel.wait() => {
                if let Some(msg) = msg {
                    if channel_eof && matches!(msg, ChannelMsg::Data {..} | ChannelMsg::ExtendedData {..}) {
                        // channel received eof, so we ignore data
                        continue;
                    }
                    match msg {
                        ChannelMsg::Close => {
                            info!("channel {channel_id} server received close msg");
                            ws_channel.close().await?;
                            break;
                        }
                        ChannelMsg::Eof => {
                            info!("channel {channel_id} server received eof msg");
                            ws_channel.eof().await?;
                            channel_eof = true;
                        }
                        _ => {
                            handle_server_msg(&ws_channel, msg).await?;
                        }
                    }
                } else {
                    info!("channel {channel_id} server msg channel closed");
                    break;
                }
            }
            msg = ws_channel.wait() => {
                if let Some(msg) = msg {
                    if ws_channel_eof && matches!(msg, ChannelMsg::Data {..} | ChannelMsg::ExtendedData {..}) {
                        // ws_channel received eof, so we ignore data
                        continue;
                    }
                    match msg {
                        ChannelMsg::Close => {
                            info!("channel {channel_id} client received close msg");
                            channel.close().await?;
                            break;
                        }
                        ChannelMsg::Eof => {
                            info!("channel {channel_id} client received eof msg");
                            channel.eof().await?;
                            ws_channel_eof = true;
                        }
                        _ => {
                            handle_client_msg(ws_channel_id, &mut channel, &server_handle, msg).await?;
                        }
                    }
                } else {
                    info!("channel {channel_id} client msg channel closed");
                    break;
                }
            }
        }
    }
    let _ = channel.close().await;
    let _ = ws_channel.close().await;
    info!("proxy connection closed channel {channel_id}");
    Ok(())
}

async fn handle_server_msg(
    client_channel: &Channel<russh::client::Msg>,
    msg: ChannelMsg,
) -> Result<()> {
    match msg {
        ChannelMsg::Data { data } => {
            client_channel.data(&data[..]).await?;
        }
        ChannelMsg::ExtendedData { data, ext } => {
            client_channel.extended_data(ext, &data[..]).await?;
        }
        ChannelMsg::Eof => {
            client_channel.eof().await?;
        }
        ChannelMsg::RequestPty {
            want_reply,
            term,
            col_width,
            row_height,
            pix_width,
            pix_height,
            terminal_modes,
        } => {
            client_channel
                .request_pty(
                    want_reply,
                    &term,
                    col_width,
                    row_height,
                    pix_width,
                    pix_height,
                    &terminal_modes,
                )
                .await?;
        }
        ChannelMsg::SetEnv {
            want_reply,
            variable_name,
            variable_value,
        } => {
            client_channel
                .set_env(want_reply, variable_name, variable_value)
                .await?;
        }
        ChannelMsg::RequestShell { want_reply } => {
            client_channel.request_shell(want_reply).await?;
        }
        ChannelMsg::Exec {
            want_reply,
            command,
        } => {
            client_channel.exec(want_reply, command).await?;
        }
        ChannelMsg::Signal { signal } => {
            client_channel.signal(signal).await?;
        }
        ChannelMsg::RequestSubsystem { want_reply, name } => {
            client_channel.request_subsystem(want_reply, name).await?;
        }
        ChannelMsg::RequestX11 {
            want_reply,
            single_connection,
            x11_authentication_protocol,
            x11_authentication_cookie,
            x11_screen_number,
        } => {
            client_channel
                .request_x11(
                    want_reply,
                    single_connection,
                    x11_authentication_protocol,
                    x11_authentication_cookie,
                    x11_screen_number,
                )
                .await?;
        }
        ChannelMsg::WindowChange {
            col_width,
            row_height,
            pix_width,
            pix_height,
        } => {
            client_channel
                .window_change(col_width, row_height, pix_width, pix_height)
                .await?;
        }
        ChannelMsg::WindowAdjusted { .. } => {}
        _ => {
            debug!("unhandled ssh server msg: {msg:?}");
        }
    }
    Ok(())
}
