use super::{
    handshake::{read_token, write_server_ack},
    *,
};
use crate::{client::TunnelMode, TunnelError};
use chrono::{Duration as ChronoDuration, Utc};
use lapdev_common::devbox::{DirectTunnelConfig, DirectTunnelCredential};
use rand::random;
use std::{net::SocketAddr, sync::Arc, time::Duration};

fn config_for_server(addr: SocketAddr, token: &str, certificate: &[u8]) -> DirectTunnelConfig {
    DirectTunnelConfig {
        credential: DirectTunnelCredential {
            token: token.to_string(),
            expires_at: Utc::now() + ChronoDuration::minutes(5),
        },
        server_certificate: Some(certificate.to_vec()),
        stun_observed_addr: Some(addr),
    }
}

async fn accept_direct_connection(endpoint: Arc<DirectEndpoint>) -> Result<String, TunnelError> {
    let incoming = endpoint
        .endpoint()
        .accept()
        .await
        .ok_or(TunnelError::ConnectionClosed)?;

    let connecting = incoming
        .accept()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let connection = connecting
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let presented = read_token(&mut recv).await?;
    if !endpoint.credential().remove(&presented).await {
        return Err(TunnelError::Remote("invalid direct credential".into()));
    }

    write_server_ack(&mut send).await?;
    send.finish()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    Ok(presented)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_endpoint_can_bind_on_loopback() {
    let endpoint = DirectEndpoint::bind_with_options(
        "127.0.0.1:0".parse().unwrap(),
        DirectEndpointOptions::default(),
    )
    .await
    .expect("bind direct endpoint");
    let addr = endpoint.local_addr().expect("local addr");
    assert!(addr.ip().is_loopback());
    endpoint.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_endpoint_tracks_server_certificate() {
    let (server_config, certificate) = build_server_config().expect("server config");
    let certificate_clone = certificate.clone();
    let endpoint = DirectEndpoint::bind_with_quinn_config(
        "127.0.0.1:0".parse().unwrap(),
        quinn::EndpointConfig::default(),
        Some((server_config, certificate)),
        DirectEndpointOptions::default(),
    )
    .await
    .expect("bind direct endpoint");
    let stored = endpoint.server_certificate();
    assert_eq!(stored, certificate_clone.as_slice());
    endpoint.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_endpoint_can_send_probe() {
    let endpoint = DirectEndpoint::bind_with_options(
        "127.0.0.1:0".parse().unwrap(),
        DirectEndpointOptions::default(),
    )
    .await
    .expect("bind direct endpoint");

    let listener = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind listener");
    listener
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set timeout");
    let target_addr = listener.local_addr().expect("listener addr");
    let recv_handle = tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 512];
        listener
            .recv_from(&mut buf)
            .map(|(len, addr)| (len, addr, buf))
    });

    let endpoint_addr = endpoint.local_addr().expect("endpoint addr");
    endpoint
        .send_probe(target_addr)
        .await
        .expect("send probe succeeds");

    let (len, addr, buf) = recv_handle.await.expect("recv join").expect("recv result");
    assert_eq!(addr.port(), endpoint_addr.port());
    assert!(len > 5);
    assert_eq!(buf[0], 0x17);
    assert_eq!(&buf[1..3], &[0x03, 0x03]);
    let declared_len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    assert_eq!(declared_len, len - 5);
    assert!(
        buf[5..len].iter().any(|byte| *byte != 0),
        "probe body should not be empty noise"
    );

    endpoint.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_endpoint_can_connect_tunnel() {
    let server = Arc::new(
        DirectEndpoint::bind_with_options(
            "127.0.0.1:0".parse().unwrap(),
            DirectEndpointOptions::default(),
        )
        .await
        .expect("bind direct server"),
    );
    let client_endpoint = DirectEndpoint::bind_with_options(
        "127.0.0.1:0".parse().unwrap(),
        DirectEndpointOptions::default(),
    )
    .await
    .expect("bind client endpoint");

    let token = format!("test-token-{}", random::<u64>());
    let expires_at = Utc::now() + ChronoDuration::minutes(5);
    server.credential().insert(token.clone(), expires_at).await;

    let server_addr = server.local_addr().expect("server address");
    let config = config_for_server(server_addr, &token, server.server_certificate());

    let accept_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { accept_direct_connection(server).await })
    };

    let client = client_endpoint
        .connect_tunnel(&config)
        .await
        .expect("connect tunnel");
    assert_eq!(client.mode(), TunnelMode::Direct);

    let accepted_token = accept_task
        .await
        .expect("accept task join")
        .expect("direct accept");
    assert_eq!(accepted_token, token);

    client_endpoint.close();
    server.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_direct_tunnel_can_bind_and_connect_on_loopback() {
    let server = Arc::new(
        DirectEndpoint::bind_with_options(
            "127.0.0.1:0".parse().unwrap(),
            DirectEndpointOptions::default(),
        )
        .await
        .expect("bind direct server"),
    );

    let token = format!("test-token-{}", random::<u64>());
    let expires_at = Utc::now() + ChronoDuration::minutes(5);
    server.credential().insert(token.clone(), expires_at).await;

    let server_addr = server.local_addr().expect("server address");
    let config = config_for_server(server_addr, &token, server.server_certificate());

    let accept_task = {
        let server = Arc::clone(&server);
        tokio::spawn(async move { accept_direct_connection(server).await })
    };

    let client = server
        .connect_tunnel(&config)
        .await
        .expect("direct connect succeeded");
    assert_eq!(client.mode(), TunnelMode::Direct);

    let accepted_token = accept_task
        .await
        .expect("accept task join")
        .expect("direct accept");
    assert_eq!(accepted_token, token);

    server.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_server_can_reuse_endpoint_for_outbound_connects() {
    let listener = Arc::new(
        DirectEndpoint::bind_with_options(
            "127.0.0.1:0".parse().unwrap(),
            DirectEndpointOptions::default(),
        )
        .await
        .expect("bind listener"),
    );
    let dialer = Arc::new(
        DirectEndpoint::bind_with_options(
            "127.0.0.1:0".parse().unwrap(),
            DirectEndpointOptions::default(),
        )
        .await
        .expect("bind dialer"),
    );

    let token = format!("test-token-{}", random::<u64>());
    let expires_at = Utc::now() + ChronoDuration::minutes(5);
    listener
        .credential()
        .insert(token.clone(), expires_at)
        .await;

    let server_addr = listener.local_addr().expect("listener addr");
    let config = config_for_server(server_addr, &token, listener.server_certificate());

    let accept_task = {
        let listener = Arc::clone(&listener);
        tokio::spawn(async move { accept_direct_connection(listener).await })
    };

    let client = dialer
        .connect_tunnel(&config)
        .await
        .expect("outbound connect");
    assert_eq!(client.mode(), TunnelMode::Direct);

    let accepted_token = accept_task
        .await
        .expect("accept task join")
        .expect("listener accept");
    assert_eq!(accepted_token, token);

    listener.close();
    dialer.close();
}
