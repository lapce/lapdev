use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use uuid::Uuid;

pub struct LapdevClient {
    client: Client,
    base_url: String,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CliTokenResponse {
    pub token: String,
    pub expires_in: u32,
}

#[derive(Debug, Deserialize)]
pub struct WhoamiResponse {
    pub user_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub organization_id: Uuid,
    pub device_name: String,
    pub authenticated_at: Option<String>,
    pub expires_at: Option<String>,
}

impl LapdevClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            token: None,
        }
    }

    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    /// Poll for CLI token after browser auth
    /// Returns Ok(Some(response)) if token is ready
    /// Returns Ok(None) if still pending (404)
    /// Returns Err for other errors
    pub async fn poll_cli_token(&self, session_id: Uuid) -> Result<Option<CliTokenResponse>> {
        let url = format!(
            "{}/api/v1/auth/cli/token?session_id={}",
            self.base_url, session_id
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send token poll request")?;

        match resp.status() {
            StatusCode::OK => {
                let token_resp = resp
                    .json()
                    .await
                    .context("Failed to parse token response")?;
                Ok(Some(token_resp))
            }
            StatusCode::NOT_FOUND => Ok(None),
            status => Err(anyhow::anyhow!(
                "Unexpected status code from server: {}",
                status
            )),
        }
    }

    /// Get current user info
    pub async fn whoami(&self) -> Result<WhoamiResponse> {
        let token = self
            .token
            .as_ref()
            .context("Not authenticated. Please run 'lapdev devbox login' first.")?;

        let resp = self
            .client
            .get(format!("{}/api/v1/devbox/whoami", self.base_url))
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .context("Failed to send whoami request")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Request failed with status {}: {}",
                status,
                if text.is_empty() { "No error message" } else { &text }
            );
        }

        resp.json()
            .await
            .context("Failed to parse whoami response")
    }
}
