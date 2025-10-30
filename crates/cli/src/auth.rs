use anyhow::{Context, Result};

const SERVICE_NAME: &str = "dev.lap.cli";

/// Store authentication token in OS keychain
/// - macOS: Keychain
/// - Linux: Secret Service (GNOME Keyring / KWallet)
/// - Windows: Credential Manager
pub fn store_token(api_host: &str, token: &str) -> Result<()> {
    let entry =
        keyring::Entry::new(SERVICE_NAME, api_host).context("Failed to create keychain entry")?;
    entry
        .set_password(token)
        .context("Failed to store token in keychain")?;
    Ok(())
}

/// Retrieve authentication token from OS keychain
pub fn get_token(api_host: &str) -> Result<String> {
    let entry =
        keyring::Entry::new(SERVICE_NAME, api_host).context("Failed to create keychain entry")?;
    let token = entry
        .get_password()
        .context("No authentication token found. Please run 'lapdev devbox login' first.")?;
    Ok(token)
}

/// Delete authentication token from OS keychain
pub fn delete_token(api_host: &str) -> Result<()> {
    let entry =
        keyring::Entry::new(SERVICE_NAME, api_host).context("Failed to create keychain entry")?;
    entry
        .delete_credential()
        .context("Failed to delete token from keychain")?;
    Ok(())
}

/// Check if a token exists in the keychain
pub fn has_token(api_host: &str) -> bool {
    get_token(api_host).is_ok()
}
