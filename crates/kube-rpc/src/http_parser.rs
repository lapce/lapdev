use std::io;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    time::{timeout, Instant},
};
use tracing::debug;

/// Parsed HTTP request information
#[derive(Debug, Clone)]
pub struct ParsedHttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

/// Result of parsing HTTP headers with body start position
pub type HttpParseResult = (ParsedHttpRequest, usize);

const MAX_HEADER_SIZE: usize = 8192;
const READ_CHUNK_SIZE: usize = 1024;
const HEADER_END_MARKER: &[u8] = b"\r\n\r\n";

/// Outcome of attempting to read HTTP headers with an optional deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderReadStatus {
    Complete,
    TimedOut,
}

/// Parse HTTP request from a buffer using httparse
pub fn parse_http_request_from_buffer(buffer: &[u8]) -> io::Result<HttpParseResult> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);

    let result = req.parse(buffer).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("HTTP parse error: {}", e),
        )
    })?;

    let body_start = match result {
        httparse::Status::Complete(headers_len) => headers_len,
        httparse::Status::Partial => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Incomplete HTTP request",
            ));
        }
    };

    let method = req
        .method
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing HTTP method"))?
        .to_string();

    let path = req
        .path
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing HTTP path"))?
        .to_string();

    let headers = req
        .headers
        .iter()
        .map(|h| {
            let name = h.name.to_string();
            let value = String::from_utf8_lossy(h.value).to_string();
            (name, value)
        })
        .collect();

    let parsed_request = ParsedHttpRequest {
        method,
        path,
        headers,
    };

    debug!(
        "Parsed HTTP request: {} {}",
        parsed_request.method, parsed_request.path
    );
    Ok((parsed_request, body_start))
}

/// Read from the stream until complete HTTP headers are buffered.
/// Returns Ok(()) once `buffer` contains the full header block (ending with CRLFCRLF).
pub async fn read_http_headers<R>(stream: &mut R, buffer: &mut Vec<u8>) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let status = read_http_headers_with_deadline(stream, buffer, None).await?;
    debug_assert!(matches!(status, HeaderReadStatus::Complete));
    Ok(())
}

/// Read HTTP headers while honoring an optional deadline. Returns whether the read completed
/// before the deadline expired.
pub async fn read_http_headers_with_deadline<R>(
    stream: &mut R,
    buffer: &mut Vec<u8>,
    deadline: Option<Instant>,
) -> io::Result<HeaderReadStatus>
where
    R: AsyncRead + Unpin,
{
    if headers_complete(buffer, HEADER_END_MARKER) {
        return Ok(HeaderReadStatus::Complete);
    }

    let mut status = HeaderReadStatus::Complete;

    while buffer.len() < MAX_HEADER_SIZE {
        let prev_len = buffer.len();
        let remaining_capacity = MAX_HEADER_SIZE - prev_len;
        let read_len = remaining_capacity.min(READ_CHUNK_SIZE);
        let mut temp_buffer = vec![0u8; read_len];
        let read_result = read_chunk_with_deadline(stream, &mut temp_buffer, deadline).await?;

        let bytes_read = match read_result {
            Some(n) => n,
            None => {
                status = HeaderReadStatus::TimedOut;
                break;
            }
        };

        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Connection closed before complete HTTP headers received",
            ));
        }

        buffer.extend_from_slice(&temp_buffer[..bytes_read]);

        let search_start = if prev_len >= HEADER_END_MARKER.len() - 1 {
            prev_len - (HEADER_END_MARKER.len() - 1)
        } else {
            0
        };

        if headers_complete(&buffer[search_start..], HEADER_END_MARKER) {
            debug!(
                "Successfully read HTTP headers after buffering {} bytes",
                buffer.len()
            );
            return Ok(HeaderReadStatus::Complete);
        }
    }

    if matches!(status, HeaderReadStatus::TimedOut) {
        Ok(HeaderReadStatus::TimedOut)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "HTTP headers exceed maximum size of {} bytes",
                MAX_HEADER_SIZE
            ),
        ))
    }
}

fn headers_complete(buffer: &[u8], marker: &[u8]) -> bool {
    buffer.windows(marker.len()).any(|window| window == marker)
}

/// Extract Content-Length from parsed headers if present
pub fn get_content_length(headers: &[(String, String)]) -> Option<usize> {
    headers
        .iter()
        .find(|(name, _)| name.to_lowercase() == "content-length")
        .and_then(|(_, value)| value.parse().ok())
}

/// Extract Host header from parsed headers
pub fn get_host_header(headers: &[(String, String)]) -> Option<&String> {
    headers
        .iter()
        .find(|(name, _)| name.to_lowercase() == "host")
        .map(|(_, value)| value)
}

/// Extract a specific header value (case-insensitive)
pub fn get_header_value<'a>(
    headers: &'a [(String, String)],
    header_name: &str,
) -> Option<&'a String> {
    let target_name = header_name.to_lowercase();
    headers
        .iter()
        .find(|(name, _)| name.to_lowercase() == target_name)
        .map(|(_, value)| value)
}

async fn read_chunk_with_deadline<R>(
    reader: &mut R,
    chunk: &mut [u8],
    deadline: Option<Instant>,
) -> io::Result<Option<usize>>
where
    R: AsyncRead + Unpin,
{
    if chunk.is_empty() {
        return Ok(Some(0));
    }

    if let Some(deadline) = deadline {
        match deadline.checked_duration_since(Instant::now()) {
            Some(remaining) if !remaining.is_zero() => {
                match timeout(remaining, reader.read(chunk)).await {
                    Ok(result) => result.map(Some),
                    Err(_) => Ok(None),
                }
            }
            _ => Ok(None),
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
    use tokio::{
        io::{AsyncRead, ReadBuf},
        time::{Duration, Instant},
    };

    #[test]
    fn test_parse_basic_get_request() {
        let request_data =
            b"GET /api/health HTTP/1.1\r\nHost: localhost:8080\r\nUser-Agent: test\r\n\r\n";

        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();

        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/health");
        assert_eq!(body_start, request_data.len());

        let host = get_host_header(&request.headers);
        assert_eq!(host, Some(&"localhost:8080".to_string()));
    }

    #[test]
    fn test_parse_post_with_body() {
        let request_data = b"POST /api/data HTTP/1.1\r\nHost: localhost:8080\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"test\": true}";

        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();

        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/data");

        let body = &request_data[body_start..];
        assert_eq!(body, b"{\"test\": true}");

        let content_length = get_content_length(&request.headers);
        assert_eq!(content_length, Some(13));
    }

    #[test]
    fn test_get_header_value() {
        let headers = vec![
            ("Host".to_string(), "example.com".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), "Bearer token123".to_string()),
        ];

        assert_eq!(
            get_header_value(&headers, "host"),
            Some(&"example.com".to_string())
        );
        assert_eq!(
            get_header_value(&headers, "HOST"),
            Some(&"example.com".to_string())
        );
        assert_eq!(
            get_header_value(&headers, "content-type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(
            get_header_value(&headers, "authorization"),
            Some(&"Bearer token123".to_string())
        );
        assert_eq!(get_header_value(&headers, "nonexistent"), None);
    }

    #[test]
    fn test_incomplete_request() {
        let incomplete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost";
        assert!(parse_http_request_from_buffer(incomplete_data).is_err());

        let complete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(parse_http_request_from_buffer(complete_data).is_ok());
    }

    #[test]
    fn test_parse_request_with_multiple_headers() {
        // Test parsing a request with various headers to ensure all functions work together
        let request_data = b"POST /api/test HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test-client\r\nContent-Type: application/json\r\nContent-Length: 26\r\nAuthorization: Bearer abc123\r\n\r\n{\"message\": \"hello world\"}";

        let (parsed_request, body_start) = parse_http_request_from_buffer(request_data).unwrap();

        // Test basic parsing
        assert_eq!(parsed_request.method, "POST");
        assert_eq!(parsed_request.path, "/api/test");

        // Test all utility functions
        assert_eq!(
            get_host_header(&parsed_request.headers),
            Some(&"example.com".to_string())
        );
        assert_eq!(get_content_length(&parsed_request.headers), Some(26));
        assert_eq!(
            get_header_value(&parsed_request.headers, "user-agent"),
            Some(&"test-client".to_string())
        );
        assert_eq!(
            get_header_value(&parsed_request.headers, "content-type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(
            get_header_value(&parsed_request.headers, "authorization"),
            Some(&"Bearer abc123".to_string())
        );

        // Test body extraction
        let body = &request_data[body_start..];
        assert_eq!(body, b"{\"message\": \"hello world\"}");
        assert_eq!(body.len(), 26); // Should match content-length
    }

    #[test]
    fn test_get_host_header() {
        let headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Host".to_string(), "api.example.com".to_string()),
            ("Authorization".to_string(), "Bearer token".to_string()),
        ];

        assert_eq!(
            get_host_header(&headers),
            Some(&"api.example.com".to_string())
        );

        // Test case-insensitive
        let headers_mixed_case = vec![("host".to_string(), "lowercase.example.com".to_string())];
        assert_eq!(
            get_host_header(&headers_mixed_case),
            Some(&"lowercase.example.com".to_string())
        );

        // Test missing host header
        let headers_no_host = vec![("Content-Type".to_string(), "application/json".to_string())];
        assert_eq!(get_host_header(&headers_no_host), None);
    }

    #[test]
    fn test_get_content_length() {
        let headers = vec![
            ("Host".to_string(), "example.com".to_string()),
            ("Content-Length".to_string(), "42".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        assert_eq!(get_content_length(&headers), Some(42));

        // Test case-insensitive
        let headers_mixed_case = vec![("content-length".to_string(), "1024".to_string())];
        assert_eq!(get_content_length(&headers_mixed_case), Some(1024));

        // Test invalid content-length
        let headers_invalid = vec![("Content-Length".to_string(), "not-a-number".to_string())];
        assert_eq!(get_content_length(&headers_invalid), None);

        // Test missing content-length
        let headers_no_cl = vec![("Host".to_string(), "example.com".to_string())];
        assert_eq!(get_content_length(&headers_no_cl), None);
    }

    #[tokio::test]
    async fn test_boundary_spanning_with_chunked_stream() {
        use std::pin::Pin;
        use std::task::{Context, Poll};
        use tokio::io::{AsyncRead, ReadBuf};

        // Mock stream that returns data in controlled chunks to test boundary scenarios
        struct ChunkedMockStream {
            chunks: Vec<Vec<u8>>,
            current_chunk: usize,
        }

        impl ChunkedMockStream {
            fn new(chunks: Vec<Vec<u8>>) -> Self {
                Self {
                    chunks,
                    current_chunk: 0,
                }
            }
        }

        impl AsyncRead for ChunkedMockStream {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.current_chunk >= self.chunks.len() {
                    // No more data (EOF)
                    return Poll::Ready(Ok(()));
                }

                let chunk = &self.chunks[self.current_chunk];
                let to_copy = std::cmp::min(chunk.len(), buf.remaining());
                buf.put_slice(&chunk[..to_copy]);

                self.current_chunk += 1;
                Poll::Ready(Ok(()))
            }
        }

        // Test case 1: Header end marker \r\n\r\n spans across chunks (\r | \n\r\n)
        let chunks = vec![
            b"GET /test HTTP/1.1\r\nHost: example.com\r".to_vec(), // Ends with \r
            b"\n\r\n".to_vec(), // Starts with \n, completes marker
            b"Body data here".to_vec(),
        ];

        let mut mock_stream = ChunkedMockStream::new(chunks);
        let mut buffer = Vec::new();

        let read_result = read_http_headers(&mut mock_stream, &mut buffer).await;
        assert!(
            read_result.is_ok(),
            "Should successfully read request with boundary-spanning marker"
        );

        let (parsed_request, _body_start) =
            parse_http_request_from_buffer(&buffer).expect("Failed to parse request");
        assert_eq!(parsed_request.method, "GET");
        assert_eq!(parsed_request.path, "/test");
        assert_eq!(
            get_host_header(&parsed_request.headers),
            Some(&"example.com".to_string())
        );

        // Test case 2: Different boundary split (\r\n | \r\n)
        let chunks2 = vec![
            b"POST /api HTTP/1.1\r\nContent-Type: application/json\r\n".to_vec(),
            b"\r\n".to_vec(), // Complete header terminator in second chunk
            b"{\"data\": \"test\"}".to_vec(),
        ];

        let mut mock_stream2 = ChunkedMockStream::new(chunks2);
        let mut buffer2 = Vec::new();

        let read_result2 = read_http_headers(&mut mock_stream2, &mut buffer2).await;
        assert!(
            read_result2.is_ok(),
            "Should handle different boundary split"
        );

        let (parsed_request2, _) =
            parse_http_request_from_buffer(&buffer2).expect("Failed to parse request");
        assert_eq!(parsed_request2.method, "POST");
        assert_eq!(parsed_request2.path, "/api");
        assert_eq!(
            get_header_value(&parsed_request2.headers, "content-type"),
            Some(&"application/json".to_string())
        );

        // Test case 3: Very fragmented - each byte of \r\n\r\n in separate chunks
        let chunks3 = vec![
            b"PUT /upload HTTP/1.1\r\nHost: api.example.com".to_vec(),
            b"\r".to_vec(), // Just \r
            b"\n".to_vec(), // Just \n
            b"\r".to_vec(), // Just \r
            b"\n".to_vec(), // Just \n - completes header marker
            b"Upload content".to_vec(),
        ];

        let mut mock_stream3 = ChunkedMockStream::new(chunks3);
        let mut buffer3 = Vec::new();

        let read_result3 = read_http_headers(&mut mock_stream3, &mut buffer3).await;
        assert!(
            read_result3.is_ok(),
            "Should handle extremely fragmented boundary marker"
        );

        let (parsed_request3, _) =
            parse_http_request_from_buffer(&buffer3).expect("Failed to parse request");
        assert_eq!(parsed_request3.method, "PUT");
        assert_eq!(parsed_request3.path, "/upload");
        assert_eq!(
            get_host_header(&parsed_request3.headers),
            Some(&"api.example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_all_possible_boundary_splits() {
        use std::pin::Pin;
        use std::task::{Context, Poll};
        use tokio::io::{AsyncRead, ReadBuf};

        // Mock stream that returns data in controlled chunks
        struct ChunkedMockStream {
            chunks: Vec<Vec<u8>>,
            current_chunk: usize,
        }

        impl ChunkedMockStream {
            fn new(chunks: Vec<Vec<u8>>) -> Self {
                Self {
                    chunks,
                    current_chunk: 0,
                }
            }
        }

        impl AsyncRead for ChunkedMockStream {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<std::io::Result<()>> {
                if self.current_chunk >= self.chunks.len() {
                    return Poll::Ready(Ok(()));
                }

                let chunk = &self.chunks[self.current_chunk];
                let to_copy = std::cmp::min(chunk.len(), buf.remaining());
                buf.put_slice(&chunk[..to_copy]);

                self.current_chunk += 1;
                Poll::Ready(Ok(()))
            }
        }

        let base_request = b"HEAD /status HTTP/1.1\r\nHost: test.example.com";
        let marker = b"\r\n\r\n";
        let body = b"Optional body content";

        // Test all possible ways to split \r\n\r\n across chunks
        let test_cases = vec![
            // Split position 1: \r | \n\r\n
            (1, "\\r | \\n\\r\\n"),
            // Split position 2: \r\n | \r\n
            (2, "\\r\\n | \\r\\n"),
            // Split position 3: \r\n\r | \n
            (3, "\\r\\n\\r | \\n"),
        ];

        for (split_pos, description) in test_cases {
            let first_part = [base_request, &marker[..split_pos]].concat();
            let second_part = [&marker[split_pos..], body].concat();

            let chunks = vec![first_part, second_part];
            let mut mock_stream = ChunkedMockStream::new(chunks);
            let mut buffer = Vec::new();

            let read_result = read_http_headers(&mut mock_stream, &mut buffer).await;
            assert!(
                read_result.is_ok(),
                "Should handle boundary split at position {} ({})",
                split_pos,
                description
            );

            let (parsed_request, _) =
                parse_http_request_from_buffer(&buffer).expect("Failed to parse request");
            assert_eq!(parsed_request.method, "HEAD");
            assert_eq!(parsed_request.path, "/status");
            assert_eq!(
                get_host_header(&parsed_request.headers),
                Some(&"test.example.com".to_string())
            );
        }

        // Test additional complex splits
        let complex_cases = vec![
            // Three-way split: base | \r | \n\r\n + body
            (
                vec![
                    base_request.to_vec(),
                    b"\r".to_vec(),
                    [&b"\n\r\n"[..], body].concat(),
                ],
                "three-way: base | \\r | \\n\\r\\n+body",
            ),
            // Four-way split: base | \r | \n | \r\n + body
            (
                vec![
                    base_request.to_vec(),
                    b"\r".to_vec(),
                    b"\n".to_vec(),
                    [&b"\r\n"[..], body].concat(),
                ],
                "four-way: base | \\r | \\n | \\r\\n+body",
            ),
            // Five-way split: base | \r | \n | \r | \n + body
            (
                vec![
                    base_request.to_vec(),
                    b"\r".to_vec(),
                    b"\n".to_vec(),
                    b"\r".to_vec(),
                    [&b"\n"[..], body].concat(),
                ],
                "five-way: base | \\r | \\n | \\r | \\n+body",
            ),
            // Six-way split: completely separate
            (
                vec![
                    base_request.to_vec(),
                    b"\r".to_vec(),
                    b"\n".to_vec(),
                    b"\r".to_vec(),
                    b"\n".to_vec(),
                    body.to_vec(),
                ],
                "six-way: all separate",
            ),
        ];

        for (chunks, description) in complex_cases {
            let mut mock_stream = ChunkedMockStream::new(chunks);
            let mut buffer = Vec::new();

            let read_result = read_http_headers(&mut mock_stream, &mut buffer).await;
            assert!(
                read_result.is_ok(),
                "Should handle complex boundary split: {}",
                description
            );

            let (parsed_request, _) =
                parse_http_request_from_buffer(&buffer).expect("Failed to parse request");
            assert_eq!(parsed_request.method, "HEAD");
            assert_eq!(parsed_request.path, "/status");
            assert_eq!(
                get_host_header(&parsed_request.headers),
                Some(&"test.example.com".to_string())
            );
        }
    }

    #[tokio::test]
    async fn test_read_http_headers_immediate_deadline_times_out() {
        let mut stream = tokio::io::empty();
        let mut buffer = Vec::new();

        let status =
            read_http_headers_with_deadline(&mut stream, &mut buffer, Some(Instant::now()))
                .await
                .expect("Immediate deadline should not error");

        assert_eq!(status, HeaderReadStatus::TimedOut);
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn test_read_http_headers_deadline_succeeds_before_timeout() {
        struct SingleChunkStream {
            data: Vec<u8>,
            sent: bool,
        }

        impl AsyncRead for SingleChunkStream {
            fn poll_read(
                mut self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
                buf: &mut ReadBuf<'_>,
            ) -> Poll<io::Result<()>> {
                if self.sent {
                    return Poll::Ready(Ok(()));
                }

                buf.put_slice(&self.data);
                self.sent = true;
                Poll::Ready(Ok(()))
            }
        }

        let request = b"GET /ok HTTP/1.1\r\nHost: example.com\r\nUser-Agent: test\r\n\r\n".to_vec();
        let mut stream = SingleChunkStream {
            data: request.clone(),
            sent: false,
        };
        let mut buffer = Vec::new();

        let status = read_http_headers_with_deadline(
            &mut stream,
            &mut buffer,
            Some(Instant::now() + Duration::from_secs(5)),
        )
        .await
        .expect("Should finish before timeout");

        assert_eq!(status, HeaderReadStatus::Complete);
        assert_eq!(buffer, request);
    }
}
