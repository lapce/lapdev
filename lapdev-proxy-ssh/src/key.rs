use anyhow::{anyhow, Result};
use lapdev_conductor::server::encode_pkcs8_pem;
use lapdev_db::{api::DbApi, entities};
use russh_keys::{
    decode_secret_key,
    key::{KeyPair, SignatureHash},
};
use sea_orm::{ActiveModelTrait, ActiveValue, EntityTrait, TransactionTrait};

pub async fn load_key(kind: &str, db: &DbApi) -> Result<KeyPair> {
    let txn = db.conn.begin().await?;
    let key = if let Some(model) = entities::config::Entity::find_by_id(kind).one(&txn).await? {
        decode_secret_key(&model.value, None)?
    } else {
        let key = match kind {
            "host-ed25519" => {
                KeyPair::generate_ed25519().ok_or_else(|| anyhow!("can't generate server key"))?
            }
            "host-rsa" => KeyPair::generate_rsa(4096, SignatureHash::SHA2_512)
                .ok_or_else(|| anyhow!("can't generate server key"))?,
            _ => return Err(anyhow!("don't support {kind} host key")),
        };
        let secret = encode_pkcs8_pem(&key);
        entities::config::ActiveModel {
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

pub async fn host_keys(db: &DbApi) -> Result<Vec<KeyPair>> {
    let mut keys = Vec::new();

    let key = load_key("host-ed25519", db).await?;
    keys.push(key);

    let key = load_key("host-rsa", db).await?;
    if let Some(key) = key.with_signature_hash(SignatureHash::SHA2_512) {
        keys.push(key)
    }
    if let Some(key) = key.with_signature_hash(SignatureHash::SHA2_256) {
        keys.push(key)
    }
    if let Some(key) = key.with_signature_hash(SignatureHash::SHA1) {
        keys.push(key)
    }

    Ok(keys)
}
