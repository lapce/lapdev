use anyhow::{Context, Result};
use colored::Colorize;
use uuid::Uuid;

use crate::{api::client::LapdevClient, auth};

const POLL_INTERVAL_SECONDS: u64 = 2;
const TIMEOUT_SECONDS: u64 = 300; // 5 minutes

pub async fn execute(api_url: &str, device_name: Option<String>) -> Result<()> {
    // Check if already logged in
    if auth::has_token(api_url) {
        println!(
            "{}",
            "Already authenticated. Use 'lapdev logout' to sign out.".yellow()
        );
        return Ok(());
    }

    // Get or generate device name
    let device_name = device_name.unwrap_or_else(|| {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "Unknown Device".to_string())
    });

    // Generate session ID
    let session_id = Uuid::new_v4();

    // Build auth URL
    let auth_url = format!(
        "{}/auth/cli?session_id={}&device_name={}",
        api_url,
        session_id,
        urlencoding::encode(&device_name)
    );

    println!("{}", "Opening browser for authentication...".cyan());
    println!();
    println!("If browser doesn't open automatically, visit:");
    println!("  {}", auth_url.bright_blue().underline());
    println!();

    // Open browser
    if let Err(e) = webbrowser::open(&auth_url) {
        tracing::warn!("Failed to open browser: {}", e);
        println!(
            "{}",
            "Could not open browser automatically. Please open the URL above manually.".yellow()
        );
    }

    println!("{}", "Waiting for authentication...".cyan());

    // Poll for token
    let client = LapdevClient::new(api_url.to_string());
    let start_time = std::time::Instant::now();
    let mut poll_count = 0;

    loop {
        // Check timeout
        if start_time.elapsed().as_secs() > TIMEOUT_SECONDS {
            eprintln!("{}", "Authentication timeout. Please try again.".red());
            anyhow::bail!("Authentication timed out after {} seconds", TIMEOUT_SECONDS);
        }

        // Poll for token
        match client.poll_cli_token(session_id).await {
            Ok(Some(response)) => {
                // Success! Store the token
                auth::store_token(api_url, &response.token)
                    .context("Failed to store authentication token")?;

                // Fetch user info to display
                let client_with_token =
                    LapdevClient::new(api_url.to_string()).with_token(response.token);

                match client_with_token.whoami().await {
                    Ok(whoami) => {
                        println!();
                        println!("{}", "✓ Authentication successful!".green().bold());
                        println!();
                        println!("  User:         {}", whoami.email.bright_white());
                        println!("  Organization: {}", whoami.organization_id);
                        println!("  Device:       {}", whoami.device_name.bright_white());
                        println!(
                            "  Expires:      {} days",
                            (response.expires_in / 86400).to_string().bright_white()
                        );
                    }
                    Err(e) => {
                        // Token stored successfully but couldn't fetch user info
                        println!();
                        println!("{}", "✓ Authentication successful!".green().bold());
                        tracing::debug!("Could not fetch user info: {}", e);
                    }
                }

                return Ok(());
            }
            Ok(None) => {
                // Still pending, continue polling
                poll_count += 1;
                if poll_count % 5 == 0 {
                    // Print progress every 10 seconds
                    println!(
                        "  Still waiting... ({}s)",
                        poll_count * POLL_INTERVAL_SECONDS
                    );
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(POLL_INTERVAL_SECONDS)).await;
            }
            Err(e) => {
                eprintln!("{}", format!("Error polling for token: {}", e).red());
                anyhow::bail!("Failed to authenticate: {}", e);
            }
        }
    }
}

// URL encoding helper (simple implementation)
mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}
