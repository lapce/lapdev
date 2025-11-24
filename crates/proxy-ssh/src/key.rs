use anyhow::{anyhow, Result};
use lapdev_conductor::server::encode_pkcs8_pem;
use lapdev_db::api::DbApi;
use russh::keys::{decode_secret_key, ssh_key::rand_core::OsRng, Algorithm, HashAlg, PrivateKey};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, TransactionTrait};

pub async fn load_key(kind: &str, db: &DbApi) -> Result<PrivateKey> {
    let txn = db.conn.begin().await?;
    let key = if let Some(model) = lapdev_db_entities::config::Entity::find_by_id(kind)
        .one(&txn)
        .await?
    {
        decode_secret_key(&model.value, None)?
    } else {
        let algorithm = match kind {
            "host-ed25519" => Algorithm::Ed25519,
            "host-rsa" => Algorithm::Rsa {
                hash: Some(HashAlg::Sha512),
            },
            _ => return Err(anyhow!("don't support {kind} host key")),
        };
        let key = PrivateKey::random(&mut OsRng, algorithm)?;
        let secret = encode_pkcs8_pem(&key)?;
        lapdev_db_entities::config::ActiveModel {
            name: ActiveValue::Set(kind.to_string()),
            value: ActiveValue::Set(secret),
        }
        .insert(&txn)
        .await?;
        key
    };
    txn.commit().await?;
    Ok(key)
}

pub async fn host_keys(db: &DbApi) -> Result<Vec<PrivateKey>> {
    let mut keys = Vec::new();

    let key = load_key("host-ed25519", db).await?;
    keys.push(key);

    let key = load_key("host-rsa", db).await?;
    keys.push(key);

    Ok(keys)
}
