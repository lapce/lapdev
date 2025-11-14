use std::{sync::Arc, time::Duration};

use quinn::rustls::{
    self,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use quinn::{self, IdleTimeout, TransportConfig};
use rcgen::generate_simple_self_signed;

use crate::error::TunnelError;

const CERTIFICATE_ALT_NAMES: &[&str] = &["lapdev.devbox", "lapdev-direct", "lapdev-relay"];
const QUIC_KEEP_ALIVE_INTERVAL_SECS: u64 = 15;
const QUIC_IDLE_TIMEOUT_SECS: u64 = 60 * 60;

pub(crate) fn build_server_config() -> Result<(quinn::ServerConfig, Vec<u8>), TunnelError> {
    let names: Vec<String> = CERTIFICATE_ALT_NAMES
        .iter()
        .map(|name| name.to_string())
        .collect();
    let rcgen::CertifiedKey { cert, signing_key } = generate_simple_self_signed(names)
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let cert_der_bytes = cert.der().as_ref().to_vec();
    let cert_chain = vec![CertificateDer::from(cert)];
    let private_key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());

    let mut config = quinn::ServerConfig::with_single_cert(cert_chain, private_key.into())
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    config.transport_config(tunnel_transport_config()?);
    Ok((config, cert_der_bytes))
}

pub(super) fn client_config_with_pinned_certificate(
    server_certificate: &[u8],
) -> Result<quinn::ClientConfig, TunnelError> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(server_certificate.to_vec()))
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    crypto.enable_early_data = true;

    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))
        .expect("invalid QUIC client crypto config");
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
    client_config.transport_config(tunnel_transport_config()?);
    Ok(client_config)
}

pub(crate) fn client_config() -> Result<quinn::ClientConfig, TunnelError> {
    let roots = Arc::new(rustls::RootCertStore::empty());
    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    crypto
        .dangerous()
        .set_certificate_verifier(Arc::new(SkipServerVerification));
    crypto.enable_early_data = true;

    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))
        .expect("invalid QUIC client crypto config");
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
    client_config.transport_config(tunnel_transport_config()?);
    Ok(client_config)
}

fn tunnel_transport_config() -> Result<Arc<TransportConfig>, TunnelError> {
    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(QUIC_KEEP_ALIVE_INTERVAL_SECS)));
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(Duration::from_secs(QUIC_IDLE_TIMEOUT_SECS))
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?,
    ));
    Ok(Arc::new(transport))
}

#[derive(Debug)]
struct SkipServerVerification;

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
        ]
    }
}
