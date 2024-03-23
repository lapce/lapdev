use anyhow::Result;
use oauth2::AccessToken;
use reqwest::{header, Client};
use serde::{de::DeserializeOwned, Deserialize};

const GITLAB_API_ENDPOINT: &str = "https://gitlab.com/api/v4";
const LAPDEV_USER_AGENT: &str = "lapdev";

#[derive(Debug, Deserialize)]
pub struct GitlabUser {
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    pub id: i32,
    pub username: String,
    pub name: Option<String>,
}

#[derive(Clone)]
pub struct GitlabClient {
    base_url: String,
    client: Client,
}

impl Default for GitlabClient {
    fn default() -> Self {
        GitlabClient::new()
    }
}

impl GitlabClient {
    pub fn new() -> Self {
        let client = reqwest::Client::new();
        Self {
            base_url: GITLAB_API_ENDPOINT.to_string(),
            client,
        }
    }

    async fn request<T>(&self, url: &str, auth: &AccessToken) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}{}", self.base_url, url);

        let result = self
            .client
            .get(&url)
            .header(header::AUTHORIZATION, format!("Bearer {}", auth.secret()))
            .header(header::USER_AGENT, LAPDEV_USER_AGENT)
            .send()
            .await?
            .json()
            .await?;
        Ok(result)
    }

    pub async fn current_user(&self, auth: &AccessToken) -> Result<GitlabUser> {
        self.request("/user", auth).await
    }
}
