use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::{api::client::LapdevClient, auth};

pub async fn execute(api_url: &str) -> Result<()> {
    // Get token from keychain
    let token = auth::get_token(api_url)?;

    // Create client and fetch user info
    let client = LapdevClient::new(api_url.to_string()).with_token(token);
    let whoami = client.whoami().await.context("Failed to fetch user info")?;

    // Display user info
    println!();
    println!("{}", "Current Session:".bright_white().bold());
    println!();
    println!("  User:         {}", whoami.email.bright_cyan());

    if let Some(name) = whoami.name {
        println!("  Name:         {}", name);
    }

    println!("  User ID:      {}", whoami.user_id.to_string().dimmed());
    println!("  Organization: {}", whoami.organization_id);
    println!("  Device:       {}", whoami.device_name.bright_white());

    if let Some(auth_at) = whoami.authenticated_at {
        if let Ok(dt) = auth_at.parse::<DateTime<Utc>>() {
            println!(
                "  Authenticated: {}",
                dt.format("%Y-%m-%d %H:%M:%S UTC").to_string().dimmed()
            );
        }
    }

    if let Some(exp_at) = whoami.expires_at {
        if let Ok(dt) = exp_at.parse::<DateTime<Utc>>() {
            let now = Utc::now();
            let remaining = dt.signed_duration_since(now);
            let days_remaining = remaining.num_days();

            println!(
                "  Expires:      {} ({} days remaining)",
                dt.format("%Y-%m-%d %H:%M:%S UTC").to_string().dimmed(),
                if days_remaining > 7 {
                    days_remaining.to_string().green()
                } else if days_remaining > 0 {
                    days_remaining.to_string().yellow()
                } else {
                    "EXPIRED".to_string().red()
                }
            );
        }
    }

    println!("  API URL:      {}", api_url.dimmed());
    println!();

    Ok(())
}
