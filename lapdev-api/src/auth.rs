use std::{borrow::Cow, collections::HashMap};

use anyhow::Result;
use lapdev_common::AuthProvider;
use lapdev_db::api::DbApi;
use oauth2::{
    basic::BasicClient, AccessToken, AuthUrl, AuthorizationCode, ClientId, ClientSecret,
    RedirectUrl, TokenResponse, TokenUrl,
};
use tokio::sync::RwLock;

use crate::gitlab::GitlabClient;

pub struct AuthConfig {
    pub client_id: &'static str,
    pub client_secret: &'static str,
    pub auth_url: &'static str,
    pub token_url: &'static str,
    pub scopes: &'static [&'static str],
    pub read_repo_scopes: &'static [&'static str],
}

impl AuthConfig {
    pub const GITHUB: Self = AuthConfig {
        client_id: "github-client-id",
        client_secret: "github-client-secret",
        auth_url: "https://github.com/login/oauth/authorize",
        token_url: "https://github.com/login/oauth/access_token",
        scopes: &["read:user", "user:email"],
        read_repo_scopes: &["read:user", "user:email", "repo"],
    };
    pub const GITLAB: Self = AuthConfig {
        client_id: "gitlab-client-id",
        client_secret: "gitlab-client-secret",
        auth_url: "https://gitlab.com/oauth/authorize",
        token_url: "https://gitlab.com/oauth/token",
        scopes: &["read_user"],
        read_repo_scopes: &["read_user", "read_repository"],
    };
}

pub struct Auth {
    pub clients: RwLock<HashMap<AuthProvider, (BasicClient, AuthConfig)>>,
    pub gitlab_client: GitlabClient,
}

impl Auth {
    pub async fn new(db: &DbApi) -> Self {
        Self {
            clients: RwLock::new(Self::get_clients(db).await),
            gitlab_client: GitlabClient::new(),
        }
    }

    pub async fn resync(&self, db: &DbApi) {
        let clients = Self::get_clients(db).await;
        *self.clients.write().await = clients;
    }

    async fn get_clients(db: &DbApi) -> HashMap<AuthProvider, (BasicClient, AuthConfig)> {
        let mut clients = HashMap::new();
        if let Ok(client) = Self::build_auth(db, &AuthConfig::GITHUB).await {
            clients.insert(AuthProvider::Github, (client, AuthConfig::GITHUB));
        }
        if let Ok(client) = Self::build_auth(db, &AuthConfig::GITLAB).await {
            clients.insert(AuthProvider::Gitlab, (client, AuthConfig::GITLAB));
        }
        clients
    }

    async fn build_auth(db: &DbApi, config: &AuthConfig) -> Result<BasicClient> {
        let client_id = db.get_config(config.client_id).await?;
        let client_secret = db.get_config(config.client_secret).await?;
        if client_id.trim().is_empty() || client_secret.trim().is_empty() {
            return Err(anyhow::anyhow!("client id or secret is empty"));
        }

        let client = BasicClient::new(
            ClientId::new(client_id),
            Some(ClientSecret::new(client_secret)),
            AuthUrl::new(config.auth_url.to_string())?,
            Some(TokenUrl::new(config.token_url.to_string())?),
        );
        Ok(client)
    }

    pub async fn authorize_url(
        &self,
        provider: AuthProvider,
        redirect_url: &str,
        no_read_repo: bool,
    ) -> Result<(String, String)> {
        let clients = self.clients.read().await;
        let (client, config) = clients
            .get(&provider)
            .ok_or_else(|| anyhow::anyhow!("can't find provider"))?;
        let mut client = client.authorize_url(oauth2::CsrfToken::new_random);
        for scope in if no_read_repo {
            config.scopes
        } else {
            config.read_repo_scopes
        } {
            client = client.add_scope(oauth2::Scope::new(scope.to_string()));
        }
        let redirect_url = oauth2::RedirectUrl::new(redirect_url.to_string())?;
        let (url, csrf) = client.set_redirect_uri(Cow::Borrowed(&redirect_url)).url();

        Ok((url.as_str().to_string(), csrf.secret().to_string()))
    }

    pub async fn exchange_code(
        &self,
        provider: &AuthProvider,
        code: String,
        redirect_url: String,
    ) -> Result<AccessToken> {
        let clients = self.clients.read().await;
        let (client, _) = clients
            .get(provider)
            .ok_or_else(|| anyhow::anyhow!("can't find provider"))?;
        let token = match client
            .exchange_code(AuthorizationCode::new(code))
            .set_redirect_uri(Cow::Borrowed(&RedirectUrl::new(redirect_url)?))
            .request_async(oauth2::reqwest::async_http_client)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("exchange code error: {e:?}");
                return Err(e)?;
            }
        };
        Ok(token.access_token().to_owned())
    }
}
