use std::sync::Arc;

use anyhow::Result;
use lapdev_conductor::Conductor;
use russh::{MethodSet, Preferred};

use crate::key::host_keys;

pub async fn run(conductor: Conductor, bind: &str, port: u16) -> Result<()> {
    let keys = host_keys(&conductor.db).await?;

    let config = russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        auth_rejection_time: std::time::Duration::from_secs(3),
        auth_rejection_time_initial: Some(std::time::Duration::from_secs(0)),
        methods: MethodSet::NONE | MethodSet::PUBLICKEY,
        keys,
        preferred: Preferred {
            key: &[
                russh_keys::key::RSA_SHA2_512,
                russh_keys::key::RSA_SHA2_256,
                russh_keys::key::SSH_RSA,
                russh_keys::key::ED25519,
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let config = Arc::new(config);
    russh::server::run(
        config,
        (bind, port),
        super::proxy::SshProxy {
            id: 0,
            db: conductor.db.clone(),
            conductor: Arc::new(conductor),
        },
    )
    .await?;

    Ok(())
}
