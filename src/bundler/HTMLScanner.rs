use bun_alloc::AstAlloc;
use bun_alloc::AstBox;
use bun_alloc::core_alloc::{AllocVec, Global};
use bun_ast::{ImportKind, ImportRecord, ImportRecordFlags, ImportRecordTag, Index as AstIndex};
use bun_ast::{Loc, Log, Range, Source};
use bun_core::Error;
use bun_lolhtml_sys::lol_html as lol;
use bun_paths::fs::Path as FsPath;
use bun_paths::{platform, resolve_path};
use bun_sys as sys;

use crate::bun_fs as fs;

bun_core::declare_scope!(HTMLScanner, hidden);

// TODO(port): lifetime — `log`/`source` are borrowed for the scanner's lifetime
// (LIFETIMES.tsv had no row for this file; classified locally as BORROW_PARAM).
pub(crate) struct HTMLScanner<'a> {
    // arena field dropped — global mimalloc (see PORTING.md §Allocators).
    // Facade-typed to match `css::BundlerStyleSheet::parse_bundler`'s
    // `PlainVec2<ImportRecord>` contract (`AllocVec<T, Global>`).
    pub import_records: AllocVec<ImportRecord, Global>, // Zig: ImportRecord.List
    pub log: &'a mut Log,
    pub source: &'a Source,
}

impl<'a> HTMLScanner<'a> {
    pub(crate) fn init(log: &'a mut Log, source: &'a Source) -> HTMLScanner<'a> {
        HTMLScanner {
            import_records: AllocVec::new(),
            log,
            source,
        }
    }
}

impl<'a> HTMLScanner<'a> {
    fn create_import_record(&mut self, input_path: &[u8], kind: ImportKind) -> Result<(), Error> {
        // In HTML, sometimes people do /src/index.js
        // In that case, we don't want to use the absolute filesystem path, we want to use the path relative to the project root
        let path_to_use: &[u8] = if input_path.len() > 1 && input_path[0] == b'/' {
            resolve_path::join_abs_string::<platform::Auto>(
                fs::FileSystem::instance().top_level_dir,
                &[&input_path[1..]],
            )
        }
        // Check if imports to (e.g) "App.tsx" are actually relative imoprts w/o the "./"
        else if input_path.len() > 2 && input_path[0] != b'.' && input_path[1] != b'/' {
            'blk: {
                let Some(index_of_dot) = input_path.iter().rposition(|&b| b == b'.') else {
                    break 'blk input_path;
                };
                let ext = &input_path[index_of_dot..];
                if ext.len() > 4 {
                    break 'blk input_path;
                }
                // /foo/bar/index.html -> /foo/bar
                let dirname = resolve_path::dirname::<platform::Auto>(self.source.path.text());
                if dirname.is_empty() {
                    break 'blk input_path;
                }
                let resolved =
                    resolve_path::join_abs_string_z::<platform::Auto>(dirname, &[input_path]);
                if sys::exists_z(resolved) {
                    resolved.as_bytes()
                } else {
                    input_path
                }
            }
        } else {
            input_path
        };

        let owned: &'static [u8] =
            AstBox::leak(AstAlloc::vec_from_slice(path_to_use).into_boxed_slice());
        let record = ImportRecord {
            path: FsPath::init(owned),
            kind,
            range: Range::NONE,
            tag: ImportRecordTag::default(),
            loader: None,
            source_index: AstIndex::default(),
            module_id: 0,
            original_path: b"",
            flags: ImportRecordFlags::default(),
        };

        self.import_records.push(record);
        Ok(())
    }

    pub(crate) fn on_write_html(&mut self, bytes: &[u8]) {
        let _ = bytes; // bytes are not written in scan phase
    }

    pub(crate) fn on_html_parse_error(&mut self, message: &[u8]) {
        // bun.handleOom -> Rust Vec/Box allocations abort on OOM; just call.
        // Zig `Log.addError` dupes via `log.msgs.allocator`; here `IntoText for
        // Vec<u8>` -> `Cow::Owned`, so the Log owns and drops the copy.
        let _ = self
            .log
            .add_error(Some(self.source), Loc::EMPTY, message.to_vec());
    }

    pub(crate) fn on_tag(
        &mut self,
        _element: &mut lol::html_content::Element,
        path: &[u8],
        url_attribute: &[u8],
        kind: ImportKind,
    ) {
        let _ = url_attribute;
        let _ = self.create_import_record(path, kind);
    }

    pub(crate) fn scan(&mut self, input: &[u8]) -> Result<(), Error> {
        Processor::run(self, input)
    }
}

// Zig: const processor = HTMLProcessor(HTMLScanner, false);
type Processor<'a> = HTMLProcessor<HTMLScanner<'a>, false>;

// ---------------------------------------------------------------------------
// HTMLProcessor — generic over visitor `T` and `VISIT_DOCUMENT_TAGS`
//
// Rewritten for lol_html 3.0: the old C-FFI-style DirectiveCallback /
// HTMLRewriterBuilder / HTMLSelector / OutputSink layer has been replaced
// with the idiomatic Rust API: Settings + HtmlRewriter + element! macro.
// ---------------------------------------------------------------------------

/// Trait capturing the duck-typed methods Zig's `HTMLProcessor` calls on `T`.
/// Zig used `anytype`-style structural calls; Rust needs an explicit bound.
pub(crate) trait HTMLProcessorHandler {
    fn on_tag(
        &mut self,
        element: &mut lol::html_content::Element,
        path: &[u8],
        url_attribute: &[u8],
        kind: ImportKind,
    );
    fn on_write_html(&mut self, bytes: &[u8]);
    fn on_html_parse_error(&mut self, message: &[u8]);

    // Only required when VISIT_DOCUMENT_TAGS == true.
    fn on_body_tag(&mut self, _element: &mut lol::html_content::Element) -> bool {
        unreachable!()
    }
    fn on_head_tag(&mut self, _element: &mut lol::html_content::Element) -> bool {
        unreachable!()
    }
    fn on_html_tag(&mut self, _element: &mut lol::html_content::Element) -> bool {
        unreachable!()
    }
}

impl<'a> HTMLProcessorHandler for HTMLScanner<'a> {
    fn on_tag(
        &mut self,
        element: &mut lol::html_content::Element,
        path: &[u8],
        url_attribute: &[u8],
        kind: ImportKind,
    ) {
        HTMLScanner::on_tag(self, element, path, url_attribute, kind)
    }
    fn on_write_html(&mut self, bytes: &[u8]) {
        HTMLScanner::on_write_html(self, bytes)
    }
    fn on_html_parse_error(&mut self, message: &[u8]) {
        HTMLScanner::on_html_parse_error(self, message)
    }
}

pub(crate) struct HTMLProcessor<T, const VISIT_DOCUMENT_TAGS: bool>(std::marker::PhantomData<T>);

#[derive(Clone, Copy)]
pub struct TagHandler {
    /// CSS selector to match elements
    pub selector: &'static [u8],
    /// Whether this tag can have text content that needs to be processed
    pub has_content: bool,
    /// The attribute to extract the URL from
    pub url_attribute: &'static [u8],
    /// The kind of import to create
    pub kind: ImportKind,

    pub is_head_or_html: bool,
}

impl TagHandler {
    const fn new(
        selector: &'static [u8],
        has_content: bool,
        url_attribute: &'static [u8],
        kind: ImportKind,
    ) -> Self {
        Self {
            selector,
            has_content,
            url_attribute,
            kind,
            is_head_or_html: false,
        }
    }
}

pub(crate) const TAG_HANDLERS: [TagHandler; 16] = [
    // Module scripts with src
    TagHandler::new(b"script[src]", false, b"src", ImportKind::Stmt),
    // CSS Stylesheets
    TagHandler::new(
        b"link[rel='stylesheet'][href]",
        false,
        b"href",
        ImportKind::At,
    ),
    // CSS Assets
    TagHandler::new(b"link[as='style'][href]", false, b"href", ImportKind::At),
    // Font files
    TagHandler::new(
        b"link[as='font'][href], link[type^='font/'][href]",
        false,
        b"href",
        ImportKind::Url,
    ),
    // Image assets
    TagHandler::new(b"link[as='image'][href]", false, b"href", ImportKind::Url),
    // Audio/Video assets
    TagHandler::new(
        b"link[as='video'][href], link[as='audio'][href]",
        false,
        b"href",
        ImportKind::Url,
    ),
    // Web Workers
    TagHandler::new(b"link[as='worker'][href]", false, b"href", ImportKind::Stmt),
    // Manifest files
    TagHandler::new(
        b"link[rel='manifest'][href]",
        false,
        b"href",
        ImportKind::Url,
    ),
    // Icons
    TagHandler::new(
        b"link[rel='icon'][href], link[rel='apple-touch-icon'][href]",
        false,
        b"href",
        ImportKind::Url,
    ),
    // Images with src
    TagHandler::new(b"img[src]", false, b"src", ImportKind::Url),
    // Images with srcset
    TagHandler::new(b"img[srcset]", false, b"srcset", ImportKind::Url),
    // Videos with src
    TagHandler::new(b"video[src]", false, b"src", ImportKind::Url),
    // Videos with poster
    TagHandler::new(b"video[poster]", false, b"poster", ImportKind::Url),
    // Audio with src
    TagHandler::new(b"audio[src]", false, b"src", ImportKind::Url),
    // Source elements with src
    TagHandler::new(b"source[src]", false, b"src", ImportKind::Url),
    // Source elements with srcset
    TagHandler::new(b"source[srcset]", false, b"srcset", ImportKind::Url),
    //     // Iframes
    //     TagHandler::new(b"iframe[src]", false, b"src", ImportKind::Url),
];

#[inline]
fn lol_err(_: lol::errors::RewritingError) -> Error {
    bun_core::err!(Fail)
}

impl<T: HTMLProcessorHandler, const VISIT_DOCUMENT_TAGS: bool>
    HTMLProcessor<T, VISIT_DOCUMENT_TAGS>
{
    pub(crate) fn run(this: &mut T, input: &[u8]) -> Result<(), Error> {
        let this_ptr: *mut T = this;

        // Build Settings with all the element content handlers.
        let mut settings = lol::Settings::new()
            .with_encoding(lol::AsciiCompatibleEncoding::utf_8())
            .with_strict(false)
            .with_memory_settings(
                lol::MemorySettings::new()
                    .with_preallocated_parsing_buffer_size((input.len() / 4).max(1024))
                    .with_max_allowed_memory_usage(1024 * 1024 * 10),
            );

        // Add handlers for each tag type using closures that capture this_ptr
        // and the tag index. The closures are boxed (as required by
        // ElementHandler<'h>) and borrow `this_ptr` for the duration of the
        // rewriter.
        for i in 0..TAG_HANDLERS.len() {
            let tag_info = &TAG_HANDLERS[i];
            let selector_str = std::str::from_utf8(tag_info.selector).unwrap_or("");
            let tag_index = i;
            let handler = move |element: &mut lol::html_content::Element| {
                let tag_info = &TAG_HANDLERS[tag_index];
                // Handle URL attribute if present
                if !tag_info.url_attribute.is_empty() {
                    let url_attr_str = std::str::from_utf8(tag_info.url_attribute).unwrap_or("");
                    if element.has_attribute(url_attr_str) {
                        if let Some(value) = element.get_attribute(url_attr_str) {
                            if !value.is_empty() {
                                bun_core::scoped_log!(
                                    HTMLScanner,
                                    "{} {}",
                                    bstr::BStr::new(tag_info.selector),
                                    bstr::BStr::new(value.as_bytes())
                                );
                                // SAFETY: `this_ptr` was set from `&mut T` in `run` and is
                                // valid for the lifetime of the rewriter.
                                unsafe {
                                    (*this_ptr).on_tag(
                                        element,
                                        value.as_bytes(),
                                        tag_info.url_attribute,
                                        tag_info.kind,
                                    );
                                }
                            }
                        }
                    }
                }
                Ok(())
            };
            settings = settings.append_element_content_handler(lol::element!(selector_str, handler));
        }

        if VISIT_DOCUMENT_TAGS {
            for (i, tag) in [b"body" as &[u8], b"head", b"html"]
                .into_iter()
                .enumerate()
            {
                let tag_str = std::str::from_utf8(tag).unwrap_or("");
                let which = i as u8;
                let handler = move |element: &mut lol::html_content::Element| {
                    // SAFETY: `this_ptr` was set from `&mut T` in `run` and is valid for the
                    // lifetime of the rewriter.
                    unsafe {
                        match which {
                            0 => (*this_ptr).on_body_tag(element),
                            1 => (*this_ptr).on_head_tag(element),
                            _ => (*this_ptr).on_html_tag(element),
                        };
                    }
                    Ok(())
                };
                settings = settings.append_element_content_handler(lol::element!(tag_str, handler));
            }
        }

        // Output sink that forwards to `on_write_html`.
        let mut output_sink = |bytes: &[u8]| {
            // SAFETY: `this_ptr` was set from `&mut T` and is valid for the
            // lifetime of the rewriter.
            unsafe {
                (*this_ptr).on_write_html(bytes);
            }
        };

        let res: Result<(), Error> = (|| {
            let mut rewriter = lol::HtmlRewriter::new(settings, &mut output_sink);
            rewriter.write(input).map_err(lol_err)?;
            rewriter.end().map_err(lol_err)?;
            Ok(())
        })();

        if res.is_err() {
            // In lol_html 3.0 there is no HTMLString::last_error().
            // Parse errors are propagated via the RewritingError returned by
            // write/end. Report a generic message.
            this.on_html_parse_error(b"HTML parsing error");
        }
        res
    }
}

// ported from: src/bundler/HTMLScanner.zig
