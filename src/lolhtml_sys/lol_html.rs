//! Thin re-export shim over the `lol_html` pure Rust crate.
//!
//! Replaces the former C FFI binding layer (`lol_html_c_api` + `extern "C"` blocks)
//! with direct Rust types. The API surface is preserved as closely as possible to
//! minimize consumer changes, but some v3 API differences require adaptation:
//!
//! - `before/after/replace` content type: `bool` → `ContentType` enum
//! - `comment!` macro → `comments!`
//! - Handler method return types differ (some now return `()` not `Result`)

// Re-export the entire lol_html crate so consumers can use lol_html types directly.
pub use lol_html::*;

/// Encoding — maps the old C FFI Encoding enum.
/// lol_html v3 uses `AsciiCompatibleEncoding` internally; we expose UTF-8 by default.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Encoding {
    UTF8,
    UTF16,
}

impl Encoding {
    pub fn label(self) -> &'static [u8] {
        match self {
            Encoding::UTF8 => b"UTF-8",
            Encoding::UTF16 => b"UTF-16",
        }
    }
}

/// Memory settings — maps old C FFI MemorySettings.
#[derive(Copy, Clone)]
pub struct MemorySettings {
    pub preallocated_parsing_buffer_size: usize,
    pub max_allowed_memory_usage: usize,
}

/// Source location bytes — preserved for consumers.
#[derive(Copy, Clone, Debug)]
pub struct SourceLocationBytes {
    pub start: usize,
    pub end: usize,
}

/// Error type — re-exported from lol_html.
pub type Error = lol_html::errors::RewritingError;

/// ContentType helper — maps old `bool` html_content parameter.
/// In the old C API, `html_content: bool` meant `true = HTML, false = Text`.
/// lol_html v3 uses `ContentType::Html` / `ContentType::Text`.
pub use lol_html::html_content::ContentType;

/// Helper to convert old bool to ContentType (true = Html, false = Text).
#[inline(always)]
pub fn content_type_from_bool(html: bool) -> ContentType {
    if html {
        ContentType::Html
    } else {
        ContentType::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lol_html::{HtmlRewriter, Settings, element, text, comments};

    // ── Basic rewriter: element tag rename ──────────────────────────────────

    #[test]
    fn rewrite_element_tag_name() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("h1", |el| {
                el.set_tag_name("h2")?;
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<h1>Hello</h1>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("<h2>"), "h1 should be renamed to h2, got: {result}");
    }

    // ── Element attribute manipulation ──────────────────────────────────────

    #[test]
    fn rewrite_element_set_attribute() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("div", |el| {
                el.set_attribute("class", "modified")?;
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<div>content</div>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("class=\"modified\""), "got: {result}");
    }

    #[test]
    fn rewrite_element_get_attribute() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("a", |el| {
                let href = el.get_attribute("href").unwrap_or_default();
                el.set_attribute("data-original-href", &href)?;
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<a href=\"https://example.com\">link</a>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("data-original-href=\"https://example.com\""), "got: {result}");
    }

    // ── Content insertion ──────────────────────────────────────────────────

    #[test]
    fn rewrite_element_before_after() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("p", |el| {
                el.before("<span>before</span>", ContentType::Html);
                el.after("<span>after</span>", ContentType::Html);
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<p>text</p>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("<span>before</span>"), "got: {result}");
        assert!(result.contains("<span>after</span>"), "got: {result}");
    }

    #[test]
    fn rewrite_element_prepend_append() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("div", |el| {
                el.prepend("PREFIX:", ContentType::Text);
                el.append(":SUFFIX", ContentType::Text);
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<div>middle</div>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("PREFIX:"), "got: {result}");
        assert!(result.contains(":SUFFIX"), "got: {result}");
    }

    // ── Text content handler ───────────────────────────────────────────────

    #[test]
    fn rewrite_text_content() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(text!("p", |t| {
                if t.last_in_text_node() {
                    t.after(" [processed]", ContentType::Text);
                }
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<p>Hello world</p>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("[processed]"), "got: {result}");
    }

    // ── Comment handler ────────────────────────────────────────────────────

    #[test]
    fn rewrite_comment() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(comments!("div", |c| {
                c.set_text("modified comment")?;
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<div><!-- original --></div>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("modified comment"), "got: {result}");
    }

    // ── Streaming: multiple write chunks ───────────────────────────────────

    #[test]
    fn rewrite_streaming_chunks() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!("b", |el| {
                el.set_tag_name("strong")?;
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<b>Hel").unwrap();
        rewriter.write(b"lo</b>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(result.contains("<strong>Hello</strong>"), "got: {result}");
    }

    // ── Remove element ─────────────────────────────────────────────────────

    #[test]
    fn rewrite_remove_element() {
        let mut output = Vec::new();
        let settings = Settings::new()
            .append_element_content_handler(element!(".remove-me", |el| {
                el.remove();
                Ok(())
            }));
        let mut rewriter = HtmlRewriter::new(settings, |c: &[u8]| output.extend_from_slice(c));
        rewriter.write(b"<div><span class=\"remove-me\">gone</span><span class=\"keep\">kept</span></div>").unwrap();
        rewriter.end().unwrap();
        let result = String::from_utf8(output).unwrap();
        assert!(!result.contains("gone"), "removed element should not appear, got: {result}");
        assert!(result.contains("kept"), "kept element should remain, got: {result}");
    }

    // ── ContentType helper ─────────────────────────────────────────────────

    #[test]
    fn content_type_from_bool_mapping() {
        assert!(matches!(content_type_from_bool(true), ContentType::Html));
        assert!(matches!(content_type_from_bool(false), ContentType::Text));
    }

    // ── Encoding shim ──────────────────────────────────────────────────────

    #[test]
    fn encoding_label() {
        assert_eq!(Encoding::UTF8.label(), b"UTF-8");
        assert_eq!(Encoding::UTF16.label(), b"UTF-16");
    }
}
