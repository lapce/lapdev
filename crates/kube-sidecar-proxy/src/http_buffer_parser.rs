use axum::http::{Request, HeaderMap, Method, Uri, Version};
use bytes::Bytes;
use std::io;
use tracing::{debug, warn};

/// Parse HTTP request from a buffer using axum's HTTP parsing
pub fn parse_http_request_from_buffer(buffer: &[u8]) -> io::Result<(Request<Bytes>, usize)> {
    // Find the end of headers (double CRLF or double LF)
    let headers_end = find_headers_end(buffer)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Incomplete HTTP headers"))?;
    
    // Split headers and body
    let headers_part = &buffer[..headers_end];
    let body_start = headers_end + get_header_separator_len(buffer, headers_end);
    let body_part = &buffer[body_start..];
    
    // Parse the HTTP request line and headers
    let headers_str = String::from_utf8_lossy(headers_part);
    let mut lines = headers_str.lines();
    
    // Parse request line (METHOD PATH VERSION)
    let request_line = lines.next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing request line"))?;
    
    let (method, uri, version) = parse_request_line(request_line)?;
    
    // Parse headers
    let mut header_map = HeaderMap::new();
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        
        if let Some((name, value)) = parse_header_line(line) {
            if let (Ok(header_name), Ok(header_value)) = (
                name.parse::<axum::http::HeaderName>(),
                value.parse::<axum::http::HeaderValue>()
            ) {
                header_map.insert(header_name, header_value);
            } else {
                warn!("Failed to parse header: {}: {}", name, value);
            }
        }
    }
    
    // Build the axum Request
    let mut request_builder = Request::builder()
        .method(method)
        .uri(uri)
        .version(version);
    
    // Add headers
    let headers_mut = request_builder.headers_mut().unwrap();
    *headers_mut = header_map;
    
    // Add body
    let request = request_builder
        .body(Bytes::copy_from_slice(body_part))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Failed to build request: {}", e)))?;
    
    debug!("Parsed HTTP request: {} {}", request.method(), request.uri());
    
    Ok((request, body_start))
}

/// Find the end of HTTP headers (looks for \r\n\r\n or \n\n)
fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    // Look for \r\n\r\n
    if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(pos);
    }
    
    // Look for \n\n (less common but valid)
    if let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
        return Some(pos);
    }
    
    None
}

/// Get the length of the header separator at the given position
fn get_header_separator_len(buffer: &[u8], pos: usize) -> usize {
    if pos + 4 <= buffer.len() && &buffer[pos..pos + 4] == b"\r\n\r\n" {
        4
    } else if pos + 2 <= buffer.len() && &buffer[pos..pos + 2] == b"\n\n" {
        2
    } else {
        0
    }
}

/// Parse the HTTP request line
fn parse_request_line(line: &str) -> io::Result<(Method, Uri, Version)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    
    if parts.len() < 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid request line format"
        ));
    }
    
    let method = parts[0].parse::<Method>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid method: {}", e)))?;
    
    let uri = parts[1].parse::<Uri>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid URI: {}", e)))?;
    
    let version = match parts[2] {
        "HTTP/1.0" => Version::HTTP_10,
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/2.0" => Version::HTTP_2,
        _ => {
            warn!("Unknown HTTP version: {}, defaulting to HTTP/1.1", parts[2]);
            Version::HTTP_11
        }
    };
    
    Ok((method, uri, version))
}

/// Parse a header line
fn parse_header_line(line: &str) -> Option<(String, String)> {
    let colon_pos = line.find(':')?;
    let name = line[..colon_pos].trim();
    let value = line[colon_pos + 1..].trim();
    
    Some((name.to_string(), value.to_string()))
}

/// Check if buffer contains a complete HTTP request
pub fn is_http_request_complete(buffer: &[u8]) -> bool {
    if let Some(headers_end) = find_headers_end(buffer) {
        // We have complete headers
        let body_start = headers_end + get_header_separator_len(buffer, headers_end);
        
        // For now, assume request is complete if we have headers
        // In a more sophisticated implementation, we'd check Content-Length
        // or Transfer-Encoding: chunked to determine if body is complete
        buffer.len() >= body_start
    } else {
        false
    }
}

/// Extract Content-Length from headers if present
pub fn get_content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_get_request() {
        let request_data = b"GET /api/health HTTP/1.1\r\nHost: localhost:8080\r\nUser-Agent: test\r\n\r\n";
        
        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();
        
        assert_eq!(request.method(), &Method::GET);
        assert_eq!(request.uri().path(), "/api/health");
        assert_eq!(request.headers().get("host").unwrap(), "localhost:8080");
        assert_eq!(body_start, request_data.len());
    }

    #[test]
    fn test_parse_post_request_with_body() {
        let request_data = b"POST /api/data HTTP/1.1\r\nHost: localhost:8080\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"test\": true}";
        
        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();
        
        assert_eq!(request.method(), &Method::POST);
        assert_eq!(request.uri().path(), "/api/data");
        assert_eq!(request.headers().get("content-type").unwrap(), "application/json");
        assert!(body_start < request_data.len());
        
        let body = request.body();
        assert_eq!(body.as_ref(), b"{\"test\": true}");
    }

    #[test]
    fn test_parse_request_with_otel_headers() {
        let request_data = b"GET /api/trace HTTP/1.1\r\nHost: localhost\r\ntraceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01\r\nbaggage: userId=alice,env=prod\r\n\r\n";
        
        let (request, _) = parse_http_request_from_buffer(request_data).unwrap();
        
        assert!(request.headers().contains_key("traceparent"));
        assert!(request.headers().contains_key("baggage"));
        assert_eq!(
            request.headers().get("traceparent").unwrap(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    #[test]
    fn test_incomplete_request() {
        let incomplete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost";
        assert!(!is_http_request_complete(incomplete_data));
        
        let complete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(is_http_request_complete(complete_data));
    }
}