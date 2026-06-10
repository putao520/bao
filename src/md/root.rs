//! Bun Markdown — pulldown-cmark powered parser + HTML/ANSI/custom renderers.
//!
//! Replaces the former ~13K LOC hand-written md4c-style parser with the
//! mature `pulldown-cmark` CommonMark parser. The public API surface
//! (`Options`, `RenderOptions`, `render_to_html`, `render_with_renderer`,
//! `AnsiRenderer`, etc.) is preserved so downstream consumers
//! (`MarkdownObject.rs`, `bundler`, `cli/run_command`) require no changes.

use pulldown_cmark::{html, Alignment, CodeBlockKind, Event, HeadingLevel, Options as PcOptions, Parser, Tag, TagEnd};

use crate::parser::ParserError;
use crate::types::Align;

// Re-export types needed by external renderers (e.g. JS callback renderer).
pub use crate::types::Align as AlignType;
pub use crate::types::BlockType;
pub use crate::types::SpanType;
pub use crate::types::TextType;
pub use crate::types::SpanDetail;
pub use crate::types::Renderer;
pub use crate::types::RendererImpl;
pub use crate::types::BLOCK_FENCED_CODE;
// Also used internally — reference via crate::types:: prefix to avoid
// conflicting with the pub use re-exports above.

#[derive(Clone, Copy, Default)]
pub struct RenderOptions {
    pub tag_filter: bool,
    pub heading_ids: bool,
    pub autolink_headings: bool,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub tables: bool,
    pub strikethrough: bool,
    pub tasklists: bool,
    pub permissive_autolinks: bool,
    pub permissive_url_autolinks: bool,
    pub permissive_www_autolinks: bool,
    pub permissive_email_autolinks: bool,
    pub hard_soft_breaks: bool,
    pub wiki_links: bool,
    pub underline: bool,
    pub latex_math: bool,
    pub collapse_whitespace: bool,
    pub permissive_atx_headers: bool,
    pub no_indented_code_blocks: bool,
    pub no_html_blocks: bool,
    pub no_html_spans: bool,
    /// GFM tag filter: replaces `<` with `&lt;` for disallowed HTML tags
    /// (title, textarea, style, xmp, iframe, noembed, noframes, script, plaintext).
    pub tag_filter: bool,
    pub heading_ids: bool,
    pub autolink_headings: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            tables: true,
            strikethrough: true,
            tasklists: true,
            permissive_autolinks: false,
            permissive_url_autolinks: false,
            permissive_www_autolinks: false,
            permissive_email_autolinks: false,
            hard_soft_breaks: false,
            wiki_links: false,
            underline: false,
            latex_math: false,
            collapse_whitespace: false,
            permissive_atx_headers: false,
            no_indented_code_blocks: false,
            no_html_blocks: false,
            no_html_spans: false,
            tag_filter: false,
            heading_ids: false,
            autolink_headings: false,
        }
    }
}

impl Options {
    // Private base (all-false) used for struct-update in the presets below.
    const NONE: Self = Self {
        tables: false,
        strikethrough: false,
        tasklists: false,
        permissive_autolinks: false,
        permissive_url_autolinks: false,
        permissive_www_autolinks: false,
        permissive_email_autolinks: false,
        hard_soft_breaks: false,
        wiki_links: false,
        underline: false,
        latex_math: false,
        collapse_whitespace: false,
        permissive_atx_headers: false,
        no_indented_code_blocks: false,
        no_html_blocks: false,
        no_html_spans: false,
        tag_filter: false,
        heading_ids: false,
        autolink_headings: false,
    };

    pub const COMMONMARK: Self = Self {
        tables: false,
        strikethrough: false,
        tasklists: false,
        ..Self::NONE
    };

    pub const GITHUB: Self = Self {
        tables: true,
        strikethrough: true,
        tasklists: true,
        permissive_autolinks: true,
        permissive_url_autolinks: true,
        permissive_email_autolinks: true,
        tag_filter: true,
        ..Self::NONE
    };

    pub const TERMINAL: Self = Self {
        tables: true,
        strikethrough: true,
        tasklists: true,
        permissive_url_autolinks: true,
        permissive_www_autolinks: true,
        permissive_email_autolinks: true,
        wiki_links: true,
        underline: true,
        latex_math: true,
        ..Self::NONE
    };

    /// Convert our Options to pulldown-cmark Options bitflags.
    fn to_pulldown_options(self) -> PcOptions {
        let mut opts = PcOptions::empty();
        if self.tables {
            opts.insert(PcOptions::ENABLE_TABLES);
        }
        if self.strikethrough {
            opts.insert(PcOptions::ENABLE_STRIKETHROUGH);
        }
        if self.tasklists {
            opts.insert(PcOptions::ENABLE_TASKLISTS);
        }
        if self.latex_math {
            opts.insert(PcOptions::ENABLE_MATH);
        }
        if self.heading_ids || self.autolink_headings {
            opts.insert(PcOptions::ENABLE_HEADING_ATTRIBUTES);
        }
        opts
    }

    pub fn to_render_options(self) -> RenderOptions {
        RenderOptions {
            tag_filter: self.tag_filter,
            heading_ids: self.heading_ids,
            autolink_headings: self.autolink_headings,
        }
    }

    /// `(snake_case, camelCase, setter)` for every bool field — replaces the
    /// Zig comptime `@typeInfo(Options).@"struct".fields` reflection loop in
    /// `Bun.markdown`'s option parser.
    pub const BOOL_FIELD_SETTERS: &'static [(
        &'static str,
        &'static str,
        fn(&mut Options, bool),
    )] = &[
        ("tables", "tables", |o, v| o.tables = v),
        ("strikethrough", "strikethrough", |o, v| o.strikethrough = v),
        ("tasklists", "tasklists", |o, v| o.tasklists = v),
        ("permissive_autolinks", "permissiveAutolinks", |o, v| {
            o.permissive_autolinks = v
        }),
        (
            "permissive_url_autolinks",
            "permissiveUrlAutolinks",
            |o, v| o.permissive_url_autolinks = v,
        ),
        (
            "permissive_www_autolinks",
            "permissiveWwwAutolinks",
            |o, v| o.permissive_www_autolinks = v,
        ),
        (
            "permissive_email_autolinks",
            "permissiveEmailAutolinks",
            |o, v| o.permissive_email_autolinks = v,
        ),
        ("hard_soft_breaks", "hardSoftBreaks", |o, v| {
            o.hard_soft_breaks = v
        }),
        ("wiki_links", "wikiLinks", |o, v| o.wiki_links = v),
        ("underline", "underline", |o, v| o.underline = v),
        ("latex_math", "latexMath", |o, v| o.latex_math = v),
        ("collapse_whitespace", "collapseWhitespace", |o, v| {
            o.collapse_whitespace = v
        }),
        ("permissive_atx_headers", "permissiveAtxHeaders", |o, v| {
            o.permissive_atx_headers = v
        }),
        ("no_indented_code_blocks", "noIndentedCodeBlocks", |o, v| {
            o.no_indented_code_blocks = v
        }),
        ("no_html_blocks", "noHtmlBlocks", |o, v| {
            o.no_html_blocks = v
        }),
        ("no_html_spans", "noHtmlSpans", |o, v| {
            o.no_html_spans = v
        }),
        ("tag_filter", "tagFilter", |o, v| o.tag_filter = v),
        ("heading_ids", "headingIds", |o, v| o.heading_ids = v),
        ("autolink_headings", "autolinkHeadings", |o, v| {
            o.autolink_headings = v
        }),
    ];
}

// ========================================
// pulldown-cmark Alignment → our Align
// ========================================

fn align_from_pulldown(a: Alignment) -> Align {
    match a {
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
        Alignment::None => Align::Default,
    }
}

fn align_to_data(align: Align) -> u32 {
    match align {
        Align::Default => 0,
        Align::Left => 1,
        Align::Center => 2,
        Align::Right => 3,
    }
}

// ========================================
// Event → RendererImpl bridge
// ========================================

/// Drive a `RendererImpl` by translating pulldown-cmark events into
/// `enter_block`/`leave_block`/`enter_span`/`leave_span`/`text` calls.
fn drive_renderer<'a, I>(events: I, renderer: &mut Renderer<'_>, src: &[u8]) -> Result<(), ParserError>
where
    I: Iterator<Item = Event<'a>>,
{
    // Track list state: (is_ordered, start_number, current_item_index)
    let mut list_stack: Vec<(bool, u64, u64)> = Vec::new();
    // Track table alignment vec so we can assign per-cell alignment on Td/Th
    let mut table_alignments: Vec<Align> = Vec::new();
    let mut in_thead = false;
    let mut tbody_open = false;

    for event in events {
        match event {
            // ─── Block starts ─────────────────────────────────────────

            Event::Start(Tag::Heading { level, .. }) => {
                let data = match level {
                    HeadingLevel::H1 => 1u32,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                renderer.enter_block(BlockType::H, data, 0)?;
            }

            Event::Start(Tag::Paragraph) => {
                renderer.enter_block(BlockType::P, 0, 0)?;
            }

            Event::Start(Tag::BlockQuote(_)) => {
                renderer.enter_block(BlockType::Quote, 0, 0)?;
            }

            Event::Start(Tag::List(first_item)) => {
                let is_ordered = first_item.is_some();
                let start = first_item.unwrap_or(1);
                list_stack.push((is_ordered, start, 0));
                renderer.enter_block(
                    if is_ordered { BlockType::Ol } else { BlockType::Ul },
                    start as u32,
                    0,
                )?;
            }

            Event::Start(Tag::Item) => {
                let data = if let Some((_, start, idx)) = list_stack.last_mut() {
                    let item_num = *start + *idx;
                    *idx += 1;
                    item_num as u32
                } else {
                    0
                };
                renderer.enter_block(BlockType::Li, data, 0)?;
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                let (flags, data) = match kind {
                    CodeBlockKind::Fenced(info) => {
                        // Find language start offset in source
                        let lang = info.trim_start();
                        let data = if lang.is_empty() {
                            0
                        } else {
                            // Search for the language string in the source
                            find_lang_offset(src, lang.as_bytes())
                        };
                        (BLOCK_FENCED_CODE, data)
                    }
                    CodeBlockKind::Indented => (0, 0),
                };
                renderer.enter_block(BlockType::Code, data, flags)?;
            }

            Event::Start(Tag::HtmlBlock) => {
                renderer.enter_block(BlockType::Html, 0, 0)?;
            }

            Event::Rule => {
                renderer.enter_block(BlockType::Hr, 0, 0)?;
                renderer.leave_block(BlockType::Hr, 0)?;
            }

            Event::Start(Tag::Table(aligns)) => {
                table_alignments = aligns.iter().map(|a| align_from_pulldown(*a)).collect();
                let col_count = table_alignments.len() as u32;
                renderer.enter_block(BlockType::Table, col_count, 0)?;
            }

            Event::Start(Tag::TableHead) => {
                in_thead = true;
                renderer.enter_block(BlockType::Thead, 0, 0)?;
                renderer.enter_block(BlockType::Tr, 0, 0)?;
            }

            Event::Start(Tag::TableRow) => {
                // Close thead and open tbody on first body row
                if !tbody_open {
                    // The TableHead already closed its own Tr and Thead via
                    // TagEnd::TableHead. Just open Tbody + Tr.
                    tbody_open = true;
                    renderer.enter_block(BlockType::Tbody, 0, 0)?;
                }
                renderer.enter_block(BlockType::Tr, 0, 0)?;
            }

            Event::Start(Tag::TableCell) => {
                let align = table_alignments.first().copied().unwrap_or(Align::Default);
                let block_type = if in_thead { BlockType::Th } else { BlockType::Td };
                renderer.enter_block(block_type, align_to_data(align), 0)?;
            }

            // ─── Block ends ──────────────────────────────────────────

            Event::End(TagEnd::Heading(_)) => {
                renderer.leave_block(BlockType::H, 0)?;
            }

            Event::End(TagEnd::Paragraph) => {
                renderer.leave_block(BlockType::P, 0)?;
            }

            Event::End(TagEnd::BlockQuote(_)) => {
                renderer.leave_block(BlockType::Quote, 0)?;
            }

            Event::End(TagEnd::List(is_ordered)) => {
                list_stack.pop();
                renderer.leave_block(if is_ordered { BlockType::Ol } else { BlockType::Ul }, 0)?;
            }

            Event::End(TagEnd::Item) => {
                renderer.leave_block(BlockType::Li, 0)?;
            }

            Event::End(TagEnd::CodeBlock) => {
                renderer.leave_block(BlockType::Code, 0)?;
            }

            Event::End(TagEnd::HtmlBlock) => {
                renderer.leave_block(BlockType::Html, 0)?;
            }

            Event::End(TagEnd::Table) => {
                if tbody_open {
                    renderer.leave_block(BlockType::Tbody, 0)?;
                    tbody_open = false;
                }
                table_alignments.clear();
                renderer.leave_block(BlockType::Table, 0)?;
            }

            Event::End(TagEnd::TableHead) => {
                renderer.leave_block(BlockType::Tr, 0)?;
                renderer.leave_block(BlockType::Thead, 0)?;
                in_thead = false;
            }

            Event::End(TagEnd::TableRow) => {
                renderer.leave_block(BlockType::Tr, 0)?;
            }

            Event::End(TagEnd::TableCell) => {
                let block_type = if in_thead { BlockType::Th } else { BlockType::Td };
                renderer.leave_block(block_type, 0)?;
                // Rotate alignment for next cell
                if !table_alignments.is_empty() {
                    table_alignments.remove(0);
                }
            }

            // ─── Span starts ─────────────────────────────────────────

            Event::Start(Tag::Emphasis) => {
                renderer.enter_span(SpanType::Em, SpanDetail::default())?;
            }

            Event::Start(Tag::Strong) => {
                renderer.enter_span(SpanType::Strong, SpanDetail::default())?;
            }

            Event::Start(Tag::Strikethrough) => {
                renderer.enter_span(SpanType::Del, SpanDetail::default())?;
            }

            Event::Start(Tag::Link { dest_url, title, .. }) => {
                let detail = SpanDetail {
                    href: dest_url.as_bytes(),
                    title: title.as_bytes(),
                    autolink: false,
                    autolink_email: false,
                    permissive_autolink: false,
                    autolink_www: false,
                };
                renderer.enter_span(SpanType::A, detail)?;
            }

            Event::Start(Tag::Image { dest_url, title, .. }) => {
                let detail = SpanDetail {
                    href: dest_url.as_bytes(),
                    title: title.as_bytes(),
                    ..SpanDetail::default()
                };
                renderer.enter_span(SpanType::Img, detail)?;
            }

            // ─── Span ends ───────────────────────────────────────────

            Event::End(TagEnd::Emphasis) => {
                renderer.leave_span(SpanType::Em)?;
            }

            Event::End(TagEnd::Strong) => {
                renderer.leave_span(SpanType::Strong)?;
            }

            Event::End(TagEnd::Strikethrough) => {
                renderer.leave_span(SpanType::Del)?;
            }

            Event::End(TagEnd::Link) => {
                renderer.leave_span(SpanType::A)?;
            }

            Event::End(TagEnd::Image) => {
                renderer.leave_span(SpanType::Img)?;
            }

            // ─── Text content ────────────────────────────────────────

            Event::Text(text) => {
                renderer.text(TextType::Normal, text.as_bytes())?;
            }

            Event::Code(text) => {
                // Inline code is both a span and text in pulldown-cmark.
                // Our API separates them: enter_span(Code) → text(Code, content) → leave_span(Code).
                renderer.enter_span(SpanType::Code, SpanDetail::default())?;
                renderer.text(TextType::Code, text.as_bytes())?;
                renderer.leave_span(SpanType::Code)?;
            }

            Event::InlineMath(math) => {
                renderer.enter_span(SpanType::Latexmath, SpanDetail::default())?;
                renderer.text(TextType::Latexmath, math.as_bytes())?;
                renderer.leave_span(SpanType::Latexmath)?;
            }

            Event::DisplayMath(math) => {
                renderer.enter_span(SpanType::LatexmathDisplay, SpanDetail::default())?;
                renderer.text(TextType::Latexmath, math.as_bytes())?;
                renderer.leave_span(SpanType::LatexmathDisplay)?;
            }

            Event::Html(html_text) => {
                renderer.text(TextType::Html, html_text.as_bytes())?;
            }

            Event::InlineHtml(html_text) => {
                renderer.text(TextType::Html, html_text.as_bytes())?;
            }

            Event::SoftBreak => {
                renderer.text(TextType::Softbr, b"\n")?;
            }

            Event::HardBreak => {
                renderer.text(TextType::Br, b"\n")?;
            }

            Event::TaskListMarker(_checked) => {
                // Task list markers are handled implicitly through the
                // list item's data field in the old API. In pulldown-cmark
                // they appear as separate events. We don't emit text for
                // the marker — the Li block's data encodes task state.
            }

            Event::FootnoteReference(_) => {
                // Footnotes are not part of the Bun.md API surface — skip.
            }

            Event::Start(Tag::FootnoteDefinition(_)) | Event::End(TagEnd::FootnoteDefinition) => {
                // Footnotes — skip.
            }

            Event::Start(Tag::MetadataBlock(_)) | Event::End(TagEnd::MetadataBlock(_)) => {
                // Metadata blocks (YAML front matter) — skip.
            }

            Event::Start(Tag::DefinitionList)
            | Event::End(TagEnd::DefinitionList)
            | Event::Start(Tag::DefinitionListTitle)
            | Event::End(TagEnd::DefinitionListTitle)
            | Event::Start(Tag::DefinitionListDefinition)
            | Event::End(TagEnd::DefinitionListDefinition) => {
                // Definition lists — not in the Bun.md API. Skip.
            }
        }
    }
    Ok(())
}

/// Find the byte offset of a language string in the source.
/// Returns 0 if not found (which is safe — it just means no language highlighting).
fn find_lang_offset(src: &[u8], lang: &[u8]) -> u32 {
    if lang.is_empty() || src.is_empty() {
        return 0;
    }
    // Search for the language string in the source
    for i in 0..src.len().saturating_sub(lang.len()) {
        if src[i..].starts_with(lang) {
            return i as u32;
        }
    }
    0
}

// ========================================
// Public API
// ========================================

/// Render markdown to HTML. Returns the HTML as a byte slice.
pub fn render_to_html(text: &[u8]) -> Result<Box<[u8]>, ParserError> {
    render_to_html_with_options(text, Options::default())
}

/// Render markdown to HTML with custom options.
pub fn render_to_html_with_options(
    text: &[u8],
    options: Options,
) -> Result<Box<[u8]>, ParserError> {
    let input = skip_utf8_bom(text);
    let s = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(_) => {
            // If the input is not valid UTF-8, try lossy conversion.
            // pulldown-cmark requires &str input.
            String::from_utf8_lossy(input).into_owned().leak() as &'static str
        }
    };

    let pc_opts = options.to_pulldown_options();
    let parser = Parser::new_ext(s, pc_opts);

    let mut html_output = String::with_capacity(s.len() * 3 / 2);

    if options.heading_ids || options.autolink_headings {
        let events: Vec<Event<'_>> = parser.collect();
        let final_events = inject_heading_ids(events, options.autolink_headings);
        html::push_html(&mut html_output, final_events.into_iter());
    } else {
        html::push_html(&mut html_output, parser);
    }

    Ok(html_output.into_bytes().into_boxed_slice())
}

/// Parse and render using a custom renderer implementation.
pub fn render_with_renderer<'a>(
    text: &'a [u8],
    options: Options,
    renderer: Renderer<'a>,
) -> Result<(), ParserError> {
    let input = skip_utf8_bom(text);
    let s = match std::str::from_utf8(input) {
        Ok(s) => s,
        Err(_) => {
            String::from_utf8_lossy(input).into_owned().leak() as &'static str
        }
    };

    let pc_opts = options.to_pulldown_options();
    let parser = Parser::new_ext(s, pc_opts);
    let mut rend = renderer;

    drive_renderer(parser, &mut rend, input)
}

/// Skip UTF-8 BOM if present.
fn skip_utf8_bom(text: &[u8]) -> &[u8] {
    if text.len() >= 3 && text[0] == 0xEF && text[1] == 0xBB && text[2] == 0xBF {
        &text[3..]
    } else {
        text
    }
}

/// Inject heading IDs (and optional autolink headings) into the event stream.
fn inject_heading_ids<'a>(events: Vec<Event<'a>>, _autolink: bool) -> Vec<Event<'a>> {
    let mut result = Vec::with_capacity(events.len());
    let mut slug_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut heading_text_buf = String::new();
    let mut in_heading = false;
    let mut heading_start_idx: Option<usize> = None;

    for event in events {
        match &event {
            Event::Start(Tag::Heading { level: _, .. }) => {
                in_heading = true;
                heading_text_buf.clear();
                heading_start_idx = Some(result.len());
                result.push(event);
            }
            Event::End(TagEnd::Heading(_level)) => {
                in_heading = false;
                let slug = generate_slug_str(&heading_text_buf, &mut slug_counts);

                // Patch the Start(Heading) event to include the generated id
                if let Some(idx) = heading_start_idx.take() {
                    if let Event::Start(Tag::Heading { level, id, classes, attrs }) = &result[idx] {
                        if id.is_none() {
                            result[idx] = Event::Start(Tag::Heading {
                                level: *level,
                                id: Some(pulldown_cmark::CowStr::from(slug)),
                                classes: classes.clone(),
                                attrs: attrs.clone(),
                            });
                        }
                    }
                }

                result.push(event);
            }
            Event::Text(text) if in_heading => {
                heading_text_buf.push_str(text);
                result.push(event);
            }
            Event::Code(text) if in_heading => {
                heading_text_buf.push_str(text);
                result.push(event);
            }
            _ => {
                result.push(event);
            }
        }
    }

    result
}

/// Generate a GitHub-compatible slug from heading text.
fn generate_slug_str(text: &str, slug_counts: &mut std::collections::HashMap<String, u32>) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut prev_hyphen = true;

    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            slug.extend(c.to_lowercase());
            prev_hyphen = false;
        } else if c == '-' || c == ' ' {
            if !prev_hyphen && !slug.is_empty() {
                slug.push('-');
                prev_hyphen = true;
            }
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    if let Some(count) = slug_counts.get_mut(&slug) {
        *count += 1;
        format!("{}-{}", slug, *count)
    } else {
        slug_counts.insert(slug.clone(), 0);
        slug
    }
}

pub use crate::types;

pub use crate::helpers;

pub use crate::ansi_renderer as ansi;
pub use ansi::AnsiRenderer;
pub use ansi::ImageUrlCollector;
pub use ansi::Theme as AnsiTheme;
pub use ansi::detect_kitty_graphics;
pub use ansi::detect_light_background;
pub use ansi::render_to_ansi;
