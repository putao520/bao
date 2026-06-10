//! Public types for the bun_md crate.
//!
//! These types define the renderer interface (RendererImpl trait) and the
//! block/span/text enumerations used by downstream consumers like
//! MarkdownObject.rs, the ANSI renderer, and the bundler.

/// Result type for renderer callbacks.
pub type JsResult<T> = Result<T, crate::parser::ParserError>;

/// Offset into the input document.
pub type OFF = u32;
/// Size type.
pub type SZ = u32;

/// Block types reported via enter_block / leave_block callbacks.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum BlockType {
    Doc,
    Quote,
    Ul,
    Ol,
    Li,
    Hr,
    H,
    Code,
    Html,
    P,
    Table,
    Thead,
    Tbody,
    Tr,
    Th,
    Td,
}

/// Span (inline) types reported via enter_span / leave_span callbacks.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum SpanType {
    Em,
    Strong,
    A,
    Img,
    Code,
    Del,
    Latexmath,
    LatexmathDisplay,
    Wikilink,
    U,
}

/// Text types reported via the text callback.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum TextType {
    Normal,
    NullChar,
    Br,
    Softbr,
    Entity,
    Code,
    Html,
    Latexmath,
}

/// Table cell alignment.
#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
pub enum Align {
    #[default]
    Default,
    Left,
    Center,
    Right,
}

/// Renderer interface. The parser calls these methods to produce output.
pub struct Renderer<'a> {
    pub ptr: &'a mut dyn RendererImpl,
}

/// Trait backing the `Renderer` fat pointer (was Zig `Renderer.VTable`).
pub trait RendererImpl {
    fn enter_block(&mut self, block_type: BlockType, data: u32, flags: u32) -> JsResult<()>;
    fn leave_block(&mut self, block_type: BlockType, data: u32) -> JsResult<()>;
    fn enter_span(&mut self, span_type: SpanType, detail: SpanDetail<'_>) -> JsResult<()>;
    fn leave_span(&mut self, span_type: SpanType) -> JsResult<()>;
    fn text(&mut self, text_type: TextType, content: &[u8]) -> JsResult<()>;
}

impl<'a> Renderer<'a> {
    #[inline]
    pub fn enter_block(&mut self, block_type: BlockType, data: u32, flags: u32) -> JsResult<()> {
        self.ptr.enter_block(block_type, data, flags)
    }
    #[inline]
    pub fn leave_block(&mut self, block_type: BlockType, data: u32) -> JsResult<()> {
        self.ptr.leave_block(block_type, data)
    }
    #[inline]
    pub fn enter_span(&mut self, span_type: SpanType, detail: SpanDetail<'_>) -> JsResult<()> {
        self.ptr.enter_span(span_type, detail)
    }
    #[inline]
    pub fn leave_span(&mut self, span_type: SpanType) -> JsResult<()> {
        self.ptr.leave_span(span_type)
    }
    #[inline]
    pub fn text(&mut self, text_type: TextType, content: &[u8]) -> JsResult<()> {
        self.ptr.text(text_type, content)
    }
}

/// Detail data for span events (links, images, wikilinks).
#[derive(Copy, Clone)]
pub struct SpanDetail<'a> {
    pub href: &'a [u8],
    pub title: &'a [u8],
    /// Standard autolink (angle-bracket): use writeUrlEscaped (no entity/escape processing)
    pub autolink: bool,
    /// Standard autolink is an email: prepend "mailto:" to href
    pub autolink_email: bool,
    /// Permissive autolink: use HTML-escaping for href (not URL-escaping)
    pub permissive_autolink: bool,
    /// Permissive www autolink: prepend "http://" to href
    pub autolink_www: bool,
}

impl<'a> Default for SpanDetail<'a> {
    fn default() -> Self {
        Self {
            href: b"",
            title: b"",
            autolink: false,
            autolink_email: false,
            permissive_autolink: false,
            autolink_www: false,
        }
    }
}

impl<'a> SpanDetail<'a> {
    /// Widen `href`/`title` to `'static` for storage on the renderer stack.
    /// Field-by-field reconstruction (no bitwise reinterpret).
    ///
    /// # Safety
    /// Caller guarantees the source text the slices borrow outlives every
    /// read through the returned value.
    #[inline]
    pub unsafe fn detach_lifetime(self) -> SpanDetail<'static> {
        SpanDetail {
            // SAFETY: caller contract.
            href: unsafe { &*core::ptr::from_ref::<[u8]>(self.href) },
            // SAFETY: caller contract.
            title: unsafe { &*core::ptr::from_ref::<[u8]>(self.title) },
            autolink: self.autolink,
            autolink_email: self.autolink_email,
            permissive_autolink: self.permissive_autolink,
            autolink_www: self.autolink_www,
        }
    }
}

/// An attribute is a string that may contain embedded entities.
/// The text is split into substrings, each with a type (normal or entity).
#[derive(Copy, Clone)]
pub struct Attribute<'a> {
    /// Slices into the source text, one per substring.
    pub substr_offsets: &'a [SubstrOffset],
    pub substr_types: &'a [SubstrType],
}

#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum SubstrType {
    Normal,
    Entity,
}

#[derive(Copy, Clone)]
pub struct SubstrOffset {
    pub beg: OFF,
    pub end: OFF,
}

impl<'a> Attribute<'a> {
    pub fn text<'s>(&self, src: &'s [u8]) -> &'s [u8] {
        if self.substr_offsets.is_empty() {
            return b"";
        }
        let first = self.substr_offsets[0].beg;
        let last = self.substr_offsets[self.substr_offsets.len() - 1].end;
        &src[first as usize..last as usize]
    }
}

// ========================================
// Metadata extraction helpers
// ========================================

/// Extract table cell alignment from block data.
pub fn alignment_from_data(data: u32) -> Align {
    match data & 0b11 {
        0 => Align::Default,
        1 => Align::Left,
        2 => Align::Center,
        _ => Align::Right,
    }
}

/// Get string name for alignment, or null for default.
pub fn alignment_name(alignment: Align) -> Option<&'static [u8]> {
    match alignment {
        Align::Left => Some(b"left"),
        Align::Center => Some(b"center"),
        Align::Right => Some(b"right"),
        Align::Default => None,
    }
}

/// Extract task list item mark from block data. Returns 0 for non-task items.
pub fn task_mark_from_data(data: u32) -> u8 {
    data as u8
}

/// Check if a task mark indicates a checked box.
pub fn is_task_checked(task_mark: u8) -> bool {
    task_mark != 0 && task_mark != b' '
}

/// Block flag: fenced code block.
pub const BLOCK_FENCED_CODE: u32 = 0x10;
