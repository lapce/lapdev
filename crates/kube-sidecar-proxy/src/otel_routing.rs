use std::collections::HashMap;
use tracing::{debug, warn};

/// OpenTelemetry header names
pub const TRACEPARENT_HEADER: &str = "traceparent";
pub const TRACESTATE_HEADER: &str = "tracestate";
pub const BAGGAGE_HEADER: &str = "baggage";

/// Custom routing headers that can influence routing decisions
pub const X_ROUTING_KEY: &str = "x-routing-key";
pub const X_SERVICE_VERSION: &str = "x-service-version";
pub const X_CANARY_DEPLOYMENT: &str = "x-canary-deployment";

/// OpenTelemetry trace context
#[derive(Debug, Clone)]
pub struct TraceContext {
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub trace_flags: Option<String>,
    pub trace_state: Option<String>,
    pub baggage: HashMap<String, String>,
}

/// Routing decision based on headers
#[derive(Debug, Clone)]
pub struct RoutingContext {
    pub trace_context: TraceContext,
    pub routing_key: Option<String>,
    pub service_version: Option<String>,
    pub canary_deployment: Option<String>,
    /// Additional custom headers that might influence routing
    pub custom_headers: HashMap<String, String>,
}

/// Extract OpenTelemetry and routing context from parsed headers
pub fn extract_routing_context(headers: &[(String, String)]) -> RoutingContext {
    let trace_context = extract_trace_context(headers);

    RoutingContext {
        trace_context,
        routing_key: get_header_value(headers, X_ROUTING_KEY),
        service_version: get_header_value(headers, X_SERVICE_VERSION),
        canary_deployment: get_header_value(headers, X_CANARY_DEPLOYMENT),
        custom_headers: extract_custom_headers(headers),
    }
}

/// Extract OpenTelemetry trace context from headers
fn extract_trace_context(headers: &[(String, String)]) -> TraceContext {
    let mut trace_context = TraceContext {
        trace_id: None,
        span_id: None,
        trace_flags: None,
        trace_state: None,
        baggage: HashMap::new(),
    };

    // Parse traceparent header (W3C Trace Context)
    if let Some(traceparent) = get_header_value(headers, TRACEPARENT_HEADER) {
        if let Some(parsed) = parse_traceparent(&traceparent) {
            trace_context.trace_id = parsed.0;
            trace_context.span_id = parsed.1;
            trace_context.trace_flags = parsed.2;
        }
    }

    // Parse tracestate header
    if let Some(tracestate) = get_header_value(headers, TRACESTATE_HEADER) {
        trace_context.trace_state = Some(tracestate);
    }

    // Parse baggage header
    if let Some(baggage) = get_header_value(headers, BAGGAGE_HEADER) {
        trace_context.baggage = parse_baggage(&baggage);
    }

    trace_context
}

/// Get header value as String from parsed headers
fn get_header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(h_name, _)| h_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// Parse W3C traceparent header
/// Format: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
fn parse_traceparent(
    traceparent: &str,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let parts: Vec<&str> = traceparent.split('-').collect();

    if parts.len() != 4 {
        warn!("Invalid traceparent format: {}", traceparent);
        return None;
    }

    Some((
        Some(parts[1].to_string()), // trace-id
        Some(parts[2].to_string()), // parent-id
        Some(parts[3].to_string()), // trace-flags
    ))
}

/// Parse baggage header into key-value pairs
/// Format: key1=value1,key2=value2
fn parse_baggage(baggage: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();

    for item in baggage.split(',') {
        let item = item.trim();
        if let Some(eq_pos) = item.find('=') {
            let key = item[..eq_pos].trim();
            let value = item[eq_pos + 1..].trim();
            result.insert(key.to_string(), value.to_string());
        }
    }

    result
}

/// Extract custom routing headers (prefixed with x-routing-)
fn extract_custom_headers(headers: &[(String, String)]) -> HashMap<String, String> {
    let mut custom_headers = HashMap::new();

    for (name, value) in headers {
        if name.starts_with("x-routing-") {
            custom_headers.insert(name.clone(), value.clone());
        }
    }

    custom_headers
}

/// Determine routing target based on context
pub fn determine_routing_target(
    routing_context: &RoutingContext,
    original_port: u16,
) -> RoutingTarget {
    // Example routing logic based on headers

    // Check for canary deployment
    if let Some(canary) = &routing_context.canary_deployment {
        if canary.to_lowercase() == "true" {
            debug!("Routing to canary deployment");
            return RoutingTarget::Canary {
                port: original_port,
                weight: 10, // 10% traffic to canary
            };
        }
    }

    // Check for specific service version
    if let Some(version) = &routing_context.service_version {
        debug!("Routing to service version: {}", version);
        return RoutingTarget::Version {
            port: original_port,
            version: version.clone(),
        };
    }

    // Check trace context for sampling or special routing
    if let Some(trace_id) = &routing_context.trace_context.trace_id {
        // Example: Route high-value traces to premium instances
        if is_high_priority_trace(trace_id) {
            debug!("Routing high-priority trace to premium instance");
            return RoutingTarget::Premium {
                port: original_port,
            };
        }
    }

    // Check custom routing headers
    if let Some(routing_key) = &routing_context.routing_key {
        debug!("Using custom routing key: {}", routing_key);
        return RoutingTarget::Custom {
            port: original_port,
            key: routing_key.clone(),
        };
    }

    // Default routing
    RoutingTarget::Default {
        port: original_port,
    }
}

/// Routing target options
#[derive(Debug, Clone)]
pub enum RoutingTarget {
    Default { port: u16 },
    Canary { port: u16, weight: u8 },
    Version { port: u16, version: String },
    Premium { port: u16 },
    Custom { port: u16, key: String },
}

impl RoutingTarget {
    /// Get the target port for this routing decision
    pub fn get_port(&self) -> u16 {
        match self {
            RoutingTarget::Default { port } => *port,
            RoutingTarget::Canary { port, .. } => *port,
            RoutingTarget::Version { port, .. } => *port,
            RoutingTarget::Premium { port } => *port,
            RoutingTarget::Custom { port, .. } => *port,
        }
    }

    /// Get routing metadata for logging
    pub fn get_metadata(&self) -> String {
        match self {
            RoutingTarget::Default { .. } => "default".to_string(),
            RoutingTarget::Canary { weight, .. } => format!("canary({}%)", weight),
            RoutingTarget::Version { version, .. } => format!("version({})", version),
            RoutingTarget::Premium { .. } => "premium".to_string(),
            RoutingTarget::Custom { key, .. } => format!("custom({})", key),
        }
    }
}

/// Example function to determine if a trace is high priority
/// This could be based on trace sampling, customer tier, etc.
fn is_high_priority_trace(trace_id: &str) -> bool {
    // Example: Check if trace_id ends with specific patterns
    // In practice, this might check against a database or rules engine
    trace_id.ends_with("0001") || trace_id.ends_with("0002")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_headers(headers: &[(&str, &str)]) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn test_extract_trace_context() {
        let headers = create_test_headers(&[
            (
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
            ("baggage", "userId=alice,serverNode=DF28"),
        ]);

        let context = extract_routing_context(&headers);

        assert_eq!(
            context.trace_context.trace_id,
            Some("4bf92f3577b34da6a3ce929d0e0e4736".to_string())
        );
        assert_eq!(
            context.trace_context.span_id,
            Some("00f067aa0ba902b7".to_string())
        );
        assert_eq!(
            context.trace_context.baggage.get("userId"),
            Some(&"alice".to_string())
        );
    }

    #[test]
    fn test_custom_routing_headers() {
        let headers = create_test_headers(&[
            ("x-routing-key", "premium-customer"),
            ("x-canary-deployment", "true"),
        ]);

        let context = extract_routing_context(&headers);

        assert_eq!(context.routing_key, Some("premium-customer".to_string()));
        assert_eq!(context.canary_deployment, Some("true".to_string()));
    }

    #[test]
    fn test_routing_target_determination() {
        let mut context = RoutingContext {
            trace_context: TraceContext {
                trace_id: None,
                span_id: None,
                trace_flags: None,
                trace_state: None,
                baggage: HashMap::new(),
            },
            routing_key: None,
            service_version: None,
            canary_deployment: Some("true".to_string()),
            custom_headers: HashMap::new(),
        };

        let target = determine_routing_target(&context, 8080);

        match target {
            RoutingTarget::Canary { port, weight } => {
                assert_eq!(port, 8080);
                assert_eq!(weight, 10);
            }
            _ => panic!("Expected canary routing"),
        }
    }
}
