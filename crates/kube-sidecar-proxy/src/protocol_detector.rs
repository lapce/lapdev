use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::debug;

/// HTTP methods that we recognize
const HTTP_METHODS: &[&[u8]] = &[
    b"GET ",
    b"POST ",
    b"PUT ",
    b"DELETE ",
    b"HEAD ",
    b"OPTIONS ",
    b"PATCH ",
    b"TRACE ",
    b"CONNECT ",
];

/// Result of protocol detection
#[derive(Debug, Clone)]
pub enum ProtocolType {
    Http { method: String, path: String },
    Tcp,
}

/// Detect if incoming data looks like HTTP
pub async fn detect_protocol<R>(reader: &mut R) -> std::io::Result<(ProtocolType, Vec<u8>)>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; 1024]; // Read up to 1KB to detect protocol
    let bytes_read = reader.read(&mut buffer).await?;

    if bytes_read == 0 {
        return Ok((ProtocolType::Tcp, buffer));
    }

    buffer.truncate(bytes_read);

    // Check if it starts with an HTTP method
    if let Some(protocol) = detect_http_in_buffer(&buffer) {
        debug!("Detected HTTP protocol: {:?}", protocol);
        Ok((protocol, buffer))
    } else {
        debug!("Detected TCP protocol (not HTTP)");
        Ok((ProtocolType::Tcp, buffer))
    }
}

/// Check if the buffer contains HTTP request data
fn detect_http_in_buffer(buffer: &[u8]) -> Option<ProtocolType> {
    // Check if buffer starts with known HTTP method
    for &method_bytes in HTTP_METHODS {
        if buffer.starts_with(method_bytes) {
            // Try to parse the HTTP request line
            if let Some(request_line) = get_request_line(buffer) {
                if let Some((method, path)) = parse_request_line(&request_line) {
                    return Some(ProtocolType::Http {
                        method: method.to_string(),
                        path: path.to_string(),
                    });
                }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
