use anyhow::Result;
use colored::Colorize;

use crate::auth;

pub async fn execute(api_host: &str) -> Result<()> {
    // Check if token exists
    if !auth::has_token(api_host) {
        println!(
            "{}",
            "Not currently authenticated. Use 'lapdev devbox login' to sign in.".yellow()
        );
        return Ok(());
    }

    // Delete token from keychain
    auth::delete_token(api_host)?;

    println!("{}", "✓ Logged out successfully".green());
    println!();
    println!(
        "{}",
        "Your authentication token has been removed from the keychain.".dimmed()
    );
    println!(
        "{}",
        "Note: This does not invalidate the token on the server.".dimmed()
    );

    Ok(())
}
