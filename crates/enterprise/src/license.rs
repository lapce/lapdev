use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, FixedOffset};
use lapdev_common::EnterpriseLicense;
use lapdev_db::api::DbApi;
use lapdev_rpc::error::ApiError;
use pasetors::{
    claims::{Claims, ClaimsValidationRules},
    keys::{AsymmetricPublicKey, AsymmetricSecretKey},
    token::UntrustedToken,
    version4::V4,
};
use tokio::sync::RwLock;

pub const LAPDEV_ENTERPRISE_LICENSE: &str = "lapdev-enterprise-license";
const PUBLIC_KEY: &str = include_str!("../keys/public");
const LICENSE_EXPIRES_AT: &str = "expires-at";
const LICENSE_USERS: &str = "users";
const LICENSE_HOSTNAME: &str = "hostname";

#[derive(Clone)]
pub struct License {
    pub license: Arc<RwLock<Option<EnterpriseLicense>>>,
    public_key: Arc<AsymmetricPublicKey<V4>>,
    db: DbApi,
}

impl License {
    pub async fn new(db: DbApi) -> Result<License> {
        let public_key = Arc::new(parse_public_key()?);
        let license = License {
            public_key,
            db,
            license: Arc::new(RwLock::new(None)),
        };

        if let Ok(l) = license.get_license().await {
            *license.license.write().await = Some(l);
        }

        Ok(license)
    }

    pub async fn has_valid(&self) -> bool {
        self.license
            .read()
            .await
            .as_ref()
            .map(|l| l.expires_at + chrono::Days::new(15) > chrono::Utc::now())
            .unwrap_or(false)
    }

    pub async fn update_license(&self, token: &str) -> Result<EnterpriseLicense, ApiError> {
        let license = self
            .verify_license(token)
            .map_err(|_| ApiError::InvalidRequest("License key is invalid".to_string()))?;
        self.db
            .update_config(LAPDEV_ENTERPRISE_LICENSE, token)
            .await?;
        *self.license.write().await = Some(license.clone());
        Ok(license)
    }

    pub fn verify_license(&self, token: &str) -> Result<EnterpriseLicense> {
        let untrusted_token = UntrustedToken::try_from(token)?;
        let mut rules = ClaimsValidationRules::new();
        rules.allow_non_expiring();
        let token =
            pasetors::public::verify(&self.public_key, &untrusted_token, &rules, None, None)?;
        let claims = token
            .payload_claims()
            .ok_or_else(|| anyhow!("token doens't have claims"))?;
        let expires_at = claims
            .get_claim(LICENSE_EXPIRES_AT)
            .ok_or_else(|| anyhow!("claims doens't have expired at"))?;
        let users = claims
            .get_claim(LICENSE_USERS)
            .ok_or_else(|| anyhow!("claims doens't have users"))?;
        let hostname = claims
            .get_claim(LICENSE_HOSTNAME)
            .ok_or_else(|| anyhow!("claims doens't have hostname"))?;
        let expires_at: DateTime<FixedOffset> = serde_json::from_value(expires_at.to_owned())?;
        let users: usize = serde_json::from_value(users.to_owned())?;
        let hostname: String = serde_json::from_value(hostname.to_owned())?;
        Ok(EnterpriseLicense {
            expires_at,
            users,
            hostname,
        })
    }

    pub async fn resync_license(&self) {
        let license = self.get_license().await.ok();
        *self.license.write().await = license;
    }

    async fn get_license(&self) -> Result<EnterpriseLicense> {
        let token = self.db.get_config(LAPDEV_ENTERPRISE_LICENSE).await?;
        self.verify_license(&token)
    }

    pub fn sign_new_license(
        &self,
        secret: &str,
        expires_at: DateTime<FixedOffset>,
        users: usize,
        hostname: String,
    ) -> Result<String> {
        let secret = base64::engine::general_purpose::STANDARD.decode(secret)?;
        let secret = AsymmetricSecretKey::from(&secret)?;

        let mut claims = Claims::new()?;
        claims.non_expiring();
        claims.add_additional(LICENSE_EXPIRES_AT, serde_json::to_value(expires_at)?)?;
        claims.add_additional(LICENSE_USERS, users)?;
        claims.add_additional(LICENSE_HOSTNAME, hostname)?;
        let token = pasetors::public::sign(&secret, &claims, None, None)?;

        Ok(token)
    }
}

fn parse_public_key() -> Result<AsymmetricPublicKey<V4>> {
    let key = STANDARD
        .decode(PUBLIC_KEY.trim())
        .with_context(|| "base64 decode public key")?;
    let key = AsymmetricPublicKey::from(&key)?;
    Ok(key)
}
