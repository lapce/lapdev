use httparse;
use std::io;
use tracing::debug;

/// Parsed HTTP request information needed for routing
#[derive(Debug)]
pub struct ParsedHttpRequest {
    pub method: String,
    pub path: String,
    pub version: u8,
    pub headers: Vec<(String, String)>,
}

/// Parse HTTP request from a buffer using httparse
pub fn parse_http_request_from_buffer(buffer: &[u8]) -> io::Result<(ParsedHttpRequest, usize)> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    
    let result = req.parse(buffer)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("HTTP parse error: {}", e)))?;
    
    let body_start = match result {
        httparse::Status::Complete(headers_len) => headers_len,
        httparse::Status::Partial => {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Incomplete HTTP request"));
        }
    };
    
    let method = req.method.unwrap_or("UNKNOWN").to_string();
    let path = req.path.unwrap_or("UNKNOWN").to_string();
    let version = req.version.unwrap_or(1);
    
    // Convert headers to owned strings
    let parsed_headers: Vec<(String, String)> = req.headers
        .iter()
        .filter_map(|h| {
            std::str::from_utf8(h.value)
                .ok()
                .map(|v| (h.name.to_string(), v.to_string()))
        })
        .collect();
    
    debug!("Parsed HTTP request: {} {}", method, path);
    
    let parsed_request = ParsedHttpRequest {
        method,
        path,
        version,
        headers: parsed_headers,
    };
    
    Ok((parsed_request, body_start))
}


/// Extract Content-Length from httparse headers if present
pub fn get_content_length(headers: &[httparse::Header]) -> Option<usize> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-length"))
        .and_then(|h| std::str::from_utf8(h.value).ok())
        .and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_get_request() {
        let request_data = b"GET /api/health HTTP/1.1\r\nHost: localhost:8080\r\nUser-Agent: test\r\n\r\n";
        
        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();
        
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/health");
        assert_eq!(request.version, 1);
        
        // Check for host header
        let host_header = request.headers.iter().find(|(name, _)| name == "Host");
        assert!(host_header.is_some());
        assert_eq!(host_header.unwrap().1, "localhost:8080");
        
        assert_eq!(body_start, request_data.len());
    }

    #[test]
    fn test_parse_post_request_with_body() {
        let request_data = b"POST /api/data HTTP/1.1\r\nHost: localhost:8080\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"test\": true}";
        
        let (request, body_start) = parse_http_request_from_buffer(request_data).unwrap();
        
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/data");
        assert_eq!(request.version, 1);
        
        // Check for content-type header
        let content_type_header = request.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        assert!(content_type_header.is_some());
        assert_eq!(content_type_header.unwrap().1, "application/json");
        
        assert!(body_start < request_data.len());
        
        // Check body content
        let body = &request_data[body_start..];
        assert_eq!(body, b"{\"test\": true}");
        
        // Test get_content_length function - convert to httparse header format
        let httparse_headers: Vec<httparse::Header> = request.headers
            .iter()
            .map(|(name, value)| httparse::Header {
                name,
                value: value.as_bytes(),
            })
            .collect();
        assert_eq!(get_content_length(&httparse_headers), Some(13));
    }

    #[test]
    fn test_parse_request_with_otel_headers() {
        let request_data = b"GET /api/trace HTTP/1.1\r\nHost: localhost\r\ntraceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01\r\nbaggage: userId=alice,env=prod\r\n\r\n";
        
        let (request, _) = parse_http_request_from_buffer(request_data).unwrap();
        
        let traceparent_header = request.headers.iter().find(|(name, _)| name == "traceparent");
        assert!(traceparent_header.is_some());
        assert_eq!(
            traceparent_header.unwrap().1,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        
        let baggage_header = request.headers.iter().find(|(name, _)| name == "baggage");
        assert!(baggage_header.is_some());
        assert_eq!(baggage_header.unwrap().1, "userId=alice,env=prod");
    }

    #[test]
    fn test_incomplete_request() {
        let incomplete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost";
        assert!(parse_http_request_from_buffer(incomplete_data).is_err());
        
        let complete_data = b"GET /api/health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(parse_http_request_from_buffer(complete_data).is_ok());
    }
}