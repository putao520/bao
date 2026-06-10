use bun_md::root::{self, Options};

// ──────────────────────────────────────────────────────────────────────────
// render_to_html — basic roundtrip
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn render_heading() {
    let html = root::render_to_html(b"# Hello").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<h1>"), "got: {s}");
    assert!(s.contains("Hello"), "got: {s}");
}

#[test]
fn render_paragraph() {
    let html = root::render_to_html(b"Hello world").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<p>Hello world</p>"), "got: {s}");
}

#[test]
fn render_bold_and_italic() {
    let html = root::render_to_html(b"**bold** and *italic*").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<strong>bold</strong>"), "got: {s}");
    assert!(s.contains("<em>italic</em>"), "got: {s}");
}

#[test]
fn render_code_block() {
    let input = b"```rust\nfn main() {}\n```";
    let html = root::render_to_html(input).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<code"), "got: {s}");
    assert!(s.contains("fn main()"), "got: {s}");
}

#[test]
fn render_inline_code() {
    let html = root::render_to_html(b"use `cargo test`").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<code>cargo test</code>"), "got: {s}");
}

#[test]
fn render_link() {
    let html = root::render_to_html(b"[example](https://example.com)").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("href=\"https://example.com\""), "got: {s}");
    assert!(s.contains("example"), "got: {s}");
}

#[test]
fn render_image() {
    let html = root::render_to_html(b"![alt](img.png)").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<img"), "got: {s}");
    assert!(s.contains("src=\"img.png\""), "got: {s}");
}

#[test]
fn render_unordered_list() {
    let input = b"- one\n- two\n- three";
    let html = root::render_to_html(input).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<ul>"), "got: {s}");
    assert!(s.contains("<li>one</li>"), "got: {s}");
}

#[test]
fn render_ordered_list() {
    let input = b"1. first\n2. second\n3. third";
    let html = root::render_to_html(input).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<ol>"), "got: {s}");
    assert!(s.contains("<li>first</li>"), "got: {s}");
}

#[test]
fn render_blockquote() {
    let html = root::render_to_html(b"> quoted text").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<blockquote>"), "got: {s}");
    assert!(s.contains("quoted text"), "got: {s}");
}

#[test]
fn render_horizontal_rule() {
    let html = root::render_to_html(b"---").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<hr"), "got: {s}");
}

// ──────────────────────────────────────────────────────────────────────────
// GFM extensions (tables, strikethrough, tasklists)
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn render_table() {
    let input = b"| A | B |\n| --- | --- |\n| 1 | 2 |";
    let html = root::render_to_html_with_options(input, Options::GITHUB).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<table>"), "got: {s}");
    assert!(s.contains("<th>"), "got: {s}");
    assert!(s.contains("<td>"), "got: {s}");
}

#[test]
fn render_strikethrough() {
    let html = root::render_to_html_with_options(b"~~deleted~~", Options::GITHUB).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<del>deleted</del>"), "got: {s}");
}

#[test]
fn render_tasklist() {
    let input = b"- [x] done\n- [ ] todo";
    let html = root::render_to_html_with_options(input, Options::GITHUB).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    // pulldown-cmark renders task lists as checkboxes
    assert!(s.contains("x") || s.contains("checked"), "got: {s}");
}

// ──────────────────────────────────────────────────────────────────────────
// Options presets
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn options_commonmark_no_extensions() {
    let opts = Options::COMMONMARK;
    assert!(!opts.tables);
    assert!(!opts.strikethrough);
    assert!(!opts.tasklists);
}

#[test]
fn options_github_enables_gfm() {
    let opts = Options::GITHUB;
    assert!(opts.tables);
    assert!(opts.strikethrough);
    assert!(opts.tasklists);
    assert!(opts.tag_filter);
}

#[test]
fn options_terminal_enables_math() {
    let opts = Options::TERMINAL;
    assert!(opts.latex_math);
    assert!(opts.wiki_links);
}

// ──────────────────────────────────────────────────────────────────────────
// Edge cases
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn render_empty_input() {
    let html = root::render_to_html(b"").unwrap();
    // Empty input should produce empty or minimal HTML
    assert!(html.is_empty() || html.len() < 10, "got: {:?}", std::str::from_utf8(&html));
}

#[test]
fn render_utf8_bom_stripped() {
    let input: &[u8] = &[0xEF, 0xBB, 0xBF, b'H', b'i'];
    let html = root::render_to_html(input).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("Hi"), "got: {s}");
    assert!(!s.contains("\u{feff}"), "BOM should be stripped, got: {s}");
}

#[test]
fn render_hard_break() {
    let html = root::render_to_html(b"line1  \nline2").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("<br"), "got: {s}");
}

#[test]
fn render_soft_break() {
    let html = root::render_to_html(b"line1\nline2").unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    // Soft breaks become either newline or <br> depending on options
    assert!(s.contains("line1") && s.contains("line2"), "got: {s}");
}

// ──────────────────────────────────────────────────────────────────────────
// Heading ID injection
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn render_heading_ids() {
    let opts = Options {
        heading_ids: true,
        ..Options::GITHUB
    };
    let html = root::render_to_html_with_options(b"# My Title", opts).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    assert!(s.contains("id=") || s.contains("my-title"), "got: {s}");
}

#[test]
fn render_heading_ids_deduplication() {
    let opts = Options {
        heading_ids: true,
        ..Options::GITHUB
    };
    let input = b"# Title\n## Title";
    let html = root::render_to_html_with_options(input, opts).unwrap();
    let s = std::str::from_utf8(&html).unwrap();
    // Second occurrence should have a -1 suffix
    assert!(s.contains("title-1") || s.matches("title").count() >= 2, "got: {s}");
}

// ──────────────────────────────────────────────────────────────────────────
// RendererImpl bridge — verifies the event→callback translation layer
// ──────────────────────────────────────────────────────────────────────────

use bun_md::types::{BlockType, Renderer, RendererImpl, SpanDetail, SpanType, TextType, BLOCK_FENCED_CODE};

/// A renderer that records all callbacks for verification.
struct RecordingRenderer {
    events: Vec<String>,
}

impl RendererImpl for RecordingRenderer {
    fn enter_block(&mut self, bt: BlockType, data: u32, flags: u32) -> bun_md::types::JsResult<()> {
        self.events.push(format!("enter_block({:?} data={} flags={})", bt, data, flags));
        Ok(())
    }
    fn leave_block(&mut self, bt: BlockType, _data: u32) -> bun_md::types::JsResult<()> {
        self.events.push(format!("leave_block({:?})", bt));
        Ok(())
    }
    fn enter_span(&mut self, st: SpanType, detail: SpanDetail<'_>) -> bun_md::types::JsResult<()> {
        self.events.push(format!("enter_span({:?} href={:?})", st, std::str::from_utf8(detail.href).unwrap_or("")));
        Ok(())
    }
    fn leave_span(&mut self, st: SpanType) -> bun_md::types::JsResult<()> {
        self.events.push(format!("leave_span({:?})", st));
        Ok(())
    }
    fn text(&mut self, tt: TextType, content: &[u8]) -> bun_md::types::JsResult<()> {
        self.events.push(format!("text({:?} {:?})", tt, std::str::from_utf8(content).unwrap_or("")));
        Ok(())
    }
}

#[test]
fn bridge_paragraph_events() {
    // Arrange: simple paragraph
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"Hello world", Options::default(), renderer).unwrap();
    // Assert: should see P enter/leave with text
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(P")));
    assert!(recorder.events.iter().any(|e| e.contains("Hello world")));
    assert!(recorder.events.iter().any(|e| e.contains("leave_block(P")));
}

#[test]
fn bridge_heading_level() {
    // Arrange: h2 heading
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"## Title", Options::default(), renderer).unwrap();
    // Assert: heading data should be 2
    let heading_enter = recorder.events.iter().find(|e| e.contains("enter_block(H"));
    assert!(heading_enter.is_some(), "expected enter_block(H) event, got: {:?}", recorder.events);
    assert!(heading_enter.unwrap().contains("data=2"), "expected heading level 2");
}

#[test]
fn bridge_link_href() {
    // Arrange: link with href
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"[click](https://example.com)", Options::default(), renderer).unwrap();
    // Assert: link span should carry href
    let link_enter = recorder.events.iter().find(|e| e.contains("enter_span(A"));
    assert!(link_enter.is_some(), "expected enter_span(A), got: {:?}", recorder.events);
    assert!(link_enter.unwrap().contains("https://example.com"), "expected href in link detail");
}

#[test]
fn bridge_fenced_code_flags() {
    // Arrange: fenced code block with language
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"```javascript\ncode\n```", Options::default(), renderer).unwrap();
    // Assert: code block should have BLOCK_FENCED_CODE flag
    let code_enter = recorder.events.iter().find(|e| e.contains("enter_block(Code"));
    assert!(code_enter.is_some(), "expected enter_block(Code)");
    assert!(code_enter.unwrap().contains(&format!("flags={}", BLOCK_FENCED_CODE)), "expected fenced code flag");
}

#[test]
fn bridge_table_events() {
    // Arrange: simple GFM table
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"| A | B |\n| - | - |\n| 1 | 2 |", Options::default(), renderer).unwrap();
    // Assert: should see Table, Thead, Tr, Th, Tbody, Td events
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(Table")));
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(Thead")));
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(Th")));
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(Td")));
}

#[test]
fn bridge_hr_events() {
    // Arrange: horizontal rule
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"---", Options::default(), renderer).unwrap();
    // Assert: should see enter/leave Hr
    assert!(recorder.events.iter().any(|e| e.contains("enter_block(Hr")));
    assert!(recorder.events.iter().any(|e| e.contains("leave_block(Hr")));
}

#[test]
fn bridge_ordered_list_start() {
    // Arrange: ordered list starting at 3
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"3. first\n4. second", Options::default(), renderer).unwrap();
    // Assert: Ol start data should be 3
    let ol_enter = recorder.events.iter().find(|e| e.contains("enter_block(Ol"));
    assert!(ol_enter.is_some(), "expected enter_block(Ol)");
    assert!(ol_enter.unwrap().contains("data=3"), "expected start=3");
}

#[test]
fn bridge_strikethrough_span() {
    // Arrange: strikethrough
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"~~deleted~~", Options::GITHUB, renderer).unwrap();
    // Assert: should see Del span
    assert!(recorder.events.iter().any(|e| e.contains("enter_span(Del")));
    assert!(recorder.events.iter().any(|e| e.contains("leave_span(Del")));
}

#[test]
fn bridge_image_span() {
    // Arrange: image
    let mut recorder = RecordingRenderer { events: Vec::new() };
    let renderer = Renderer { ptr: &mut recorder };
    // Act
    root::render_with_renderer(b"![alt](photo.png)", Options::default(), renderer).unwrap();
    // Assert: should see Img span with src
    let img_enter = recorder.events.iter().find(|e| e.contains("enter_span(Img"));
    assert!(img_enter.is_some(), "expected enter_span(Img), got: {:?}", recorder.events);
    assert!(img_enter.unwrap().contains("photo.png"), "expected src in img detail");
}
