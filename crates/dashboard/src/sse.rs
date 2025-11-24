use std::time::Duration;

use futures::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_timers::future::TimeoutFuture;

/// Runs an EventSource subscription with automatic reconnection and console logging.
pub async fn run_sse_with_retry<F>(
    url: String,
    event_name: &'static str,
    listener_name: String,
    mut handle_event: F,
) where
    F: FnMut(web_sys::MessageEvent) + 'static,
{
    let mut backoff = Duration::from_secs(1);

    loop {
        match EventSource::new(&url) {
            Ok(mut event_source) => {
                match event_source.subscribe(event_name) {
                    Ok(mut stream) => {
                        web_sys::console::log_1(&format!("[{listener_name}] connected").into());
                        backoff = Duration::from_secs(1);

                        while let Some(event) = stream.next().await {
                            match event {
                                Ok((_, message)) => {
                                    handle_event(message);
                                }
                                Err(err) => {
                                    web_sys::console::error_1(
                                        &format!("[{listener_name}] stream error: {err}").into(),
                                    );
                                    break;
                                }
                            }
                        }

                        web_sys::console::warn_1(
                            &format!("[{listener_name}] stream ended; attempting to reconnect")
                                .into(),
                        );
                    }
                    Err(err) => {
                        web_sys::console::error_1(
                            &format!(
                                "[{listener_name}] failed to subscribe to '{event_name}': {err}"
                            )
                            .into(),
                        );
                    }
                }

                event_source.close();
            }
            Err(err) => {
                web_sys::console::error_1(
                    &format!("[{listener_name}] failed to connect to EventSource: {err}").into(),
                );
            }
        }

        let wait_ms = backoff.as_millis().min(u128::from(u32::MAX)) as u32;
        web_sys::console::warn_1(
            &format!("[{listener_name}] reconnecting in {}s", backoff.as_secs()).into(),
        );
        TimeoutFuture::new(wait_ms).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}
