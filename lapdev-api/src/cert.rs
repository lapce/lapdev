use std::{
    collections::HashMap,
    io::{BufReader, Cursor},
    sync::{Arc, RwLock},
};

use anyhow::{anyhow, Result};
use tokio_rustls::rustls::{
    crypto::ring::sign::any_supported_type,
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
    ServerConfig,
};
use webpki::EndEntityCert;

pub type CertStore = Arc<RwLock<Arc<HashMap<String, Arc<CertifiedKey>>>>>;

pub fn tls_config(certs: CertStore) -> Result<ServerConfig> {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(CertResolver { certs }));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

struct CertResolver {
    certs: CertStore,
}

impl std::fmt::Debug for CertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CertResolver")
    }
}

impl ResolvesServerCert for CertResolver {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if let Some(domain) = client_hello.server_name() {
            let certs = { self.certs.read().map(|certs| certs.clone()) };
            if let Ok(certs) = certs {
                if let Some(cert) = certs.get(domain) {
                    return Some(cert.to_owned());
                }
                let mut parts = domain.splitn(2, '.');
                parts.next();
                let second_part = parts.next()?;
                if let Some(cert) = certs.get(&format!("*.{second_part}")) {
                    return Some(cert.to_owned());
                }
            }
        }
        None
    }
}

pub fn load_cert(cert: &str, key: &str) -> Result<(Vec<String>, CertifiedKey)> {
    let mut key = BufReader::new(Cursor::new(key));
    let key =
        rustls_pemfile::private_key(&mut key)?.ok_or_else(|| anyhow!("can't parse privte key"))?;
    let key = any_supported_type(&key)?;
    let mut cert = BufReader::new(Cursor::new(cert));
    let cert = rustls_pemfile::certs(&mut cert)
        .filter_map(|c| c.ok())
        .collect::<Vec<_>>();

    let dns_names = cert
        .iter()
        .filter_map(|cert| {
            Some(
                EndEntityCert::try_from(cert)
                    .ok()?
                    .valid_dns_names()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect::<Vec<_>>();

    let key = CertifiedKey::new(cert, key);
    Ok((dns_names, key))
}
