use std::{borrow::Cow, sync::Arc};

use anyhow::{Context, Result};
use lapdev_conductor::Conductor;
use russh::{server::Server, MethodSet, Preferred};

use crate::{key::host_keys, proxy::SshProxy};

pub async fn run(conductor: Conductor, bind: &str, port: u16) -> Result<()> {
    let keys = host_keys(&conductor.db)
        .await
        .with_context(|| "when get host keys")?;

    let config = russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        keepalive_interval: Some(std::time::Duration::from_secs(10)),
        auth_rejection_time: std::time::Duration::from_secs(3),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        methods: MethodSet::NONE | MethodSet::PUBLICKEY,
        keys,
        preferred: Preferred {
            key: Cow::Borrowed(&[
                russh::keys::key::RSA_SHA2_512,
                russh::keys::key::RSA_SHA2_256,
                russh::keys::key::SSH_RSA,
                russh::keys::key::ED25519,
            ]),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = Arc::new(config);
    let mut proxy = SshProxy {
        id: 0,
        db: conductor.db.clone(),
        conductor: Arc::new(conductor),
    };
    proxy
        .run_on_address(config, (bind, port))
        .await
        .with_context(|| "when run proxy on address")?;

    Ok(())
}
