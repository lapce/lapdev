use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use russh::keys::key::KeyPair;
use russh::{Channel, ChannelId, ChannelMsg};
use tracing::debug;

pub struct SshProxyClient {}

pub struct ClientSession {
    pub handle: russh::client::Handle<SshProxyClient>,
}

#[async_trait]
impl russh::client::Handler for SshProxyClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

impl ClientSession {
    pub async fn connect(addr: &str, key: &KeyPair) -> Result<ClientSession> {
        let config = russh::client::Config {
            inactivity_timeout: Some(std::time::Duration::from_secs(30)),
            keepalive_interval: Some(std::time::Duration::from_secs(10)),
            ..<_>::default()
        };
        let config = Arc::new(config);
        let mut handle = russh::client::connect(config, addr, SshProxyClient {}).await?;
        handle
            .authenticate_publickey("root", Arc::new(key.to_owned()))
            .await?;

        Ok(ClientSession { handle })
    }
}

pub async fn handle_client_msg(
    id: ChannelId,
    server_channel: &mut Channel<russh::server::Msg>,
    server_handle: &russh::server::Handle,
    msg: ChannelMsg,
) -> Result<()> {
    match msg {
        ChannelMsg::Data { data } => {
            server_channel.data(&data[..]).await?;
        }
        ChannelMsg::ExtendedData { data, ext } => {
            server_channel.extended_data(ext, &data[..]).await?;
        }
        ChannelMsg::Success => {
            server_handle
                .channel_success(id)
                .await
                .map_err(|_| anyhow!("failed to send data"))?;
        }
        ChannelMsg::Failure => {
            server_handle
                .channel_failure(id)
                .await
                .map_err(|_| anyhow!("failed to send data"))?;
        }
        ChannelMsg::Close => {
            server_handle
                .close(id)
                .await
                .map_err(|_| anyhow!("failed to close ssh server"))?;
        }
        ChannelMsg::Eof => {
            server_channel.eof().await?;
        }
        ChannelMsg::ExitStatus { exit_status } => {
            server_handle
                .exit_status_request(id, exit_status)
                .await
                .map_err(|_| anyhow!("failed to request exit status"))?;
        }
        ChannelMsg::ExitSignal {
            signal_name,
            core_dumped,
            error_message,
            lang_tag,
        } => {
            server_handle
                .exit_signal_request(id, signal_name, core_dumped, error_message, lang_tag)
                .await
                .map_err(|_| anyhow!("failed to request exit signal"))?;
        }
        ChannelMsg::XonXoff { client_can_do } => {
            server_handle
                .xon_xoff_request(id, client_can_do)
                .await
                .map_err(|_| anyhow!("failed to xonxoff"))?;
        }
        _ => {
            debug!("unhandled ssh client msg: {msg:?}");
        }
    }
    Ok(())
}
