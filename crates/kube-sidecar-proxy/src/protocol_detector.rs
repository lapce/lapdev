use lapdev_kube_rpc::http_parser::{self, HeaderReadStatus};
use std::io;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    time::{timeout, Duration, Instant},
};
use tracing::debug;

/// Maximum bytes to read while attempting to detect HTTP.
const MAX_DETECTION_BYTES: usize = 8192;
const READ_CHUNK_SIZE: usize = 1024;

/// HTTP/2 client preface sent at the start of cleartext connections.
const HTTP2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Result of protocol detection
#[derive(Debug, Clone)]
pub enum ProtocolType {
    Http { method: String, path: String },
    Http2,
    Tcp,
}

/// Result of protocol detection attempt.
pub struct ProtocolDetectionResult {
    pub protocol: ProtocolType,
    pub buffer: Vec<u8>,
    pub timed_out: bool,
}

/// Detect if incoming data looks like HTTP, with an optional timeout budget.
pub async fn detect_protocol<R>(
    reader: &mut R,
    timeout_duration: Option<Duration>,
) -> std::io::Result<ProtocolDetectionResult>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(1024);
    let mut timed_out = false;
    let deadline = timeout_duration.map(|d| Instant::now() + d);

    loop {
        if buffer.len() >= MAX_DETECTION_BYTES {
            break;
        }

        let remaining_capacity = MAX_DETECTION_BYTES - buffer.len();
        if remaining_capacity == 0 {
            break;
        }

        let mut chunk = [0u8; READ_CHUNK_SIZE];
        let read_len = remaining_capacity.min(READ_CHUNK_SIZE);
        let read_result =
            read_chunk_with_deadline(reader, &mut chunk[..read_len], deadline, &mut timed_out)
                .await?;

        let bytes_read = match read_result {
            Some(n) => n,
            None => break,
        };

        if bytes_read == 0 {
            break;
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);

        // Stop once we've seen the end of the first line.
        if buffer.iter().any(|&b| b == b'\n') {
            break;
        }
    }

    if buffer.is_empty() {
        return Ok(ProtocolDetectionResult {
            protocol: ProtocolType::Tcp,
            buffer,
            timed_out,
        });
    }

    // Check if it starts with the HTTP/2 client connection preface
    if buffer.starts_with(HTTP2_PREFACE)
        || (buffer.len() < HTTP2_PREFACE.len() && HTTP2_PREFACE.starts_with(&buffer))
    // partial preface
    {
        debug!("Detected HTTP/2 protocol preface");
        return Ok(ProtocolDetectionResult {
            protocol: ProtocolType::Http2,
            buffer,
            timed_out,
        });
    }

    // Check if it starts with an HTTP method (HTTP/1.x)
    if let Some(protocol) = detect_http_in_buffer(&buffer) {
        debug!("Detected HTTP protocol: {:?}", protocol);
        match http_parser::read_http_headers_with_deadline(reader, &mut buffer, deadline).await {
            Ok(HeaderReadStatus::TimedOut) => {
                timed_out = true;
            }
            Ok(HeaderReadStatus::Complete) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof
                ) =>
            {
                debug!(
                    "HTTP header read finished early while detecting protocol: {}",
                    err
                );
            }
            Err(err) => return Err(err),
        }
        Ok(ProtocolDetectionResult {
            protocol,
            buffer,
            timed_out,
        })
    } else {
        debug!("Detected TCP protocol (not HTTP)");
        Ok(ProtocolDetectionResult {
            protocol: ProtocolType::Tcp,
            buffer,
            timed_out,
        })
    }
}

/// Check if the buffer contains HTTP request data
fn detect_http_in_buffer(buffer: &[u8]) -> Option<ProtocolType> {
    if let Some(request_line) = get_request_line(buffer) {
        if let Some((method, path)) = parse_request_line(&request_line) {
            if method.chars().all(|c| c.is_ascii_uppercase()) {
                return Some(ProtocolType::Http {
                    method: method.to_string(),
                    path: path.to_string(),
                });
            }
        }
    }

    None
}

/// Extract the first line (request line) from HTTP request
fn get_request_line(buffer: &[u8]) -> Option<String> {
    // Find the first \r\n or \n
    let line_end = buffer.iter().position(|&b| b == b'\r' || b == b'\n')?;

    String::from_utf8(buffer[..line_end].to_vec()).ok()
}

/// Parse HTTP request line to extract method and path
fn parse_request_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.len() >= 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

async fn read_chunk_with_deadline<R>(
    reader: &mut R,
    chunk: &mut [u8],
    deadline: Option<Instant>,
    timed_out: &mut bool,
) -> std::io::Result<Option<usize>>
where
    R: AsyncRead + Unpin,
{
    if chunk.is_empty() {
        return Ok(Some(0));
    }

    if let Some(deadline) = deadline {
        let now = Instant::now();
        if now >= deadline {
            *timed_out = true;
            return Ok(None);
        }

        let remaining = deadline - now;
        match timeout(remaining, reader.read(chunk)).await {
            Ok(result) => result.map(Some),
            Err(_) => {
                *timed_out = true;
                Ok(None)
            }
        }
    } else {
        reader.read(chunk).await.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncRead, ReadBuf};

    #[test]
    fn test_detect_http_get() {
        let buffer = b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let result = detect_http_in_buffer(buffer);

        match result {
            Some(ProtocolType::Http { method, path }) => {
                assert_eq!(method, "GET");
                assert_eq!(path, "/api/health");
            }
            _ => panic!("Expected HTTP detection"),
        }
    }

    #[test]
    fn test_detect_http_post() {
        let buffer =
            b"POST /api/data HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"test\": true}";
        let result = detect_http_in_buffer(buffer);

        match result {
            Some(ProtocolType::Http { method, path }) => {
                assert_eq!(method, "POST");
                assert_eq!(path, "/api/data");
            }
            _ => panic!("Expected HTTP detection"),
        }
    }

    #[test]
    fn test_detect_non_http() {
        let buffer = b"\x16\x03\x01\x00\xf4\x01\x00\x00\xf0\x03\x03"; // TLS handshake
        let result = detect_http_in_buffer(buffer);

        assert!(result.is_none());
    }

    #[test]
    fn test_detect_binary() {
        let buffer = b"\x00\x01\x02\x03\x04\x05"; // Binary data
        let result = detect_http_in_buffer(buffer);

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_detect_http2_preface() {
        let buffer = super::HTTP2_PREFACE;
        let detection = super::detect_protocol(&mut &buffer[..], None)
            .await
            .unwrap();

        match detection.protocol {
            ProtocolType::Http2 => {}
            _ => panic!("Expected HTTP/2 detection"),
        }
    }

    #[tokio::test]
    async fn test_detect_http_uncommon_verb() {
        let request = b"PROPFIND /calendar HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let detection = detect_protocol(&mut std::io::Cursor::new(request), None)
            .await
            .unwrap();

        match detection.protocol {
            ProtocolType::Http { method, path } => {
                assert_eq!(method, "PROPFIND");
                assert_eq!(path, "/calendar");
            }
            _ => panic!("Expected HTTP detection for PROPFIND"),
        }
    }

    #[tokio::test]
    async fn test_detect_http_reads_until_headers_complete() {
        let headers = b"GET / HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n";
        let mut data = headers.to_vec();
        data.extend_from_slice(b"body-should-remain");
        let mut reader = ChunkedReader::new(data, headers.len());

        let detection = detect_protocol(&mut reader, None).await.unwrap();
        let ProtocolDetectionResult {
            protocol, buffer, ..
        } = detection;

        match protocol {
            ProtocolType::Http { .. } => {
                assert_eq!(buffer.len(), headers.len());
                assert_eq!(buffer, headers.to_vec());
            }
            _ => panic!("Expected HTTP detection"),
        }
    }

    #[tokio::test]
    async fn test_detect_http_gathers_headers_across_chunks() {
        let headers =
            b"POST /submit HTTP/1.1\r\nHost: example.com\r\nContent-Type: text/plain\r\n\r\n";
        let mut reader = ChunkedReader::new(headers.to_vec(), 7);
        let detection = detect_protocol(&mut reader, None).await.unwrap();
        let ProtocolDetectionResult {
            protocol, buffer, ..
        } = detection;

        match protocol {
            ProtocolType::Http { .. } => {
                assert!(buffer.ends_with(b"\r\n\r\n"));
            }
            _ => panic!("Expected HTTP detection"),
        }
    }

    #[tokio::test]
    async fn test_detect_http_long_first_line() {
        let long_path = format!("/{}", "a".repeat(5000));
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n",
            long_path
        );
        let mut reader = ChunkedReader::new(request.into_bytes(), 512);
        let detection = detect_protocol(&mut reader, None).await.unwrap();

        match detection.protocol {
            ProtocolType::Http { method, path } => {
                assert_eq!(method, "GET");
                assert_eq!(path, long_path);
            }
            _ => panic!("Expected HTTP detection for long request line"),
        }
    }

    struct ChunkedReader {
        data: Vec<u8>,
        position: usize,
        chunk_size: usize,
    }

    impl ChunkedReader {
        fn new(data: Vec<u8>, chunk_size: usize) -> Self {
            Self {
                data,
                position: 0,
                chunk_size,
            }
        }
    }

    impl AsyncRead for ChunkedReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.position >= self.data.len() {
                return Poll::Ready(Ok(()));
            }

            let end = (self.position + self.chunk_size).min(self.data.len());
            let slice = &self.data[self.position..end];
            buf.put_slice(slice);
            self.position = end;
            Poll::Ready(Ok(()))
        }
    }
}
