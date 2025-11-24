use lapdev_common::utils::resolve_api_host;

/// Resolve the Lapdev API host and base HTTPS URL.
///
/// Preference order:
/// 1. CLI argument (`--api-host`)
/// 2. `LAPDEV_API_HOST` environment variable
/// 3. Built-in default (`lapdev_common::LAPDEV_API_HOST`)
pub fn resolve_api_base_url(arg_host: Option<String>) -> (String, String) {
    let candidate = arg_host.or_else(|| std::env::var("LAPDEV_API_HOST").ok());

    let host = resolve_api_host(candidate.as_deref());
    let base_url = format!("https://{}", host);
    (host, base_url)
}
