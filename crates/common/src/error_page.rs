use std::sync::OnceLock;

const TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/pages/error_page.html"
));
const MESSAGE_PLACEHOLDER: &str = "{{MESSAGE}}";

/// Render the shared error page template with the provided message.
pub fn render_error_page(message: &str) -> String {
    TEMPLATE.replace(MESSAGE_PLACEHOLDER, &escape_html(message))
}

/// Lazily renders a specific error page message once and reuses the HTML.
pub struct LazyErrorPage {
    message: &'static str,
    html: OnceLock<String>,
}

impl LazyErrorPage {
    pub const fn new(message: &'static str) -> Self {
        Self {
            message,
            html: OnceLock::new(),
        }
    }

    pub fn get(&'static self) -> &'static str {
        self.html
            .get_or_init(|| render_error_page(self.message))
            .as_str()
    }
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_placeholder() {
        let page = render_error_page("Example");
        assert!(page.contains("Example"));
        assert!(!page.contains(MESSAGE_PLACEHOLDER));
    }

    #[test]
    fn escapes_html() {
        let page = render_error_page("<script>alert('xss')</script>");
        assert!(page.contains("&lt;script&gt;alert(&#x27;xss&#x27;)&lt;/script&gt;"));
    }
}
