use crate::mal_prelude::*;
use std::io::Write as _;

use bstr::BStr;

use bun_ast::Log;
use bun_ast::{ImportKind, ImportRecord, ImportRecordFlags};
use bun_core::strings;
use bun_lolhtml_sys::lol_html as lol;
use bun_threading::thread_pool::Task as ThreadPoolLibTask;

use crate::HTMLScanner::{HTMLProcessor, HTMLProcessorHandler};
use crate::linker_context_mod::{GenerateChunkCtx, LinkerContext, debug};
use crate::options::Loader;
use crate::{Chunk, CompileResult};

/// Rewrite the HTML with the following transforms:
/// 1. Remove all <script> and <link> tags which were not marked as
///    external. This is defined by the source_index on the ImportRecord,
///    when it's not Index.invalid then we update it accordingly. This will
///    need to be a reference to the chunk or asset.
/// 2. For all other non-external URLs, update the "src" or "href"
///    attribute to point to the asset's unique key. Later, when joining
///    chunks, we will rewrite these to their final URL or pathname,
///    including the public_path.
/// 3. If a JavaScript chunk exists, add a <script type="module" crossorigin> tag that contains
///    the JavaScript for the entry point which uses the "src" attribute
///    to point to the JavaScript chunk's unique key.
/// 4. If a CSS chunk exists, add a <link rel="stylesheet" href="..." crossorigin> tag that contains
///    the CSS for the entry point which uses the "href" attribute to point to the
///    CSS chunk's unique key.
/// 5. For each imported module or chunk within the JavaScript code, add
///    a <link rel="modulepreload" href="..." crossorigin> tag that
///    points to the module or chunk's unique key so that we tell the
///    browser to preload the user's code.
// CONCURRENCY: thread-pool callback — runs on worker threads, one task per
// HTML `PendingPartRange` (exactly one per HTML chunk). Writes:
// `chunk.compile_results_for_chunk[0]` (per-chunk disjoint). Reads
// `c.parse_graph.input_files` / `c.graph` / `ctx.chunks` shared. Never forms
// `&mut LinkerContext` — `c_ptr` stays raw; the HTML rewriter takes
// `&LinkerContext`. See `generate_compile_result_for_js_chunk` for the
// `PendingPartRange: Send` justification.
//
/// # Safety
///
/// `task` must be the intrusive `task` field of a live `PendingPartRange`
/// scheduled by `generate_chunks_in_parallel`; see
/// [`pending_part_range_prologue`](crate::linker_context_mod::pending_part_range_prologue)
/// for the full contract. Matches the `Task::callback: unsafe fn(*mut Task)`
/// contract.
pub unsafe fn generate_compile_result_for_html_chunk(task: *mut ThreadPoolLibTask) {
    // SAFETY: `task` is the intrusive `task` field of a `PendingPartRange`
    // scheduled by `generate_chunks_in_parallel`; see the helper's contract.
    let (part_range, _c_ptr, chunk_ptr, _worker) =
        unsafe { crate::linker_context_mod::pending_part_range_prologue(task) };
    let i = part_range.i as usize;
    let ctx: &GenerateChunkCtx = part_range.ctx;

    // `ctx.chunks` is a `BackRef<[Chunk]>` constructed via `new_mut` (write
    // provenance); recover the raw `*mut [Chunk]` for the HTML loader, which
    // still needs `&mut [Chunk]` for `get_{js,css}_chunk_for_html`.
    let chunks: *mut [Chunk] = ctx.chunks.as_ptr();
    // `ctx.c` is `ParentRef<LinkerContext>` and `ctx.chunk` is `BackRef<Chunk>`
    // — both yield safe shared borrows via `.get()`. `chunk` is this task's
    // exclusively-owned HTML chunk for the duration of the compile step.
    let c_ref: &LinkerContext = ctx.c.get();
    let chunk_ref: &Chunk = ctx.chunk.get();
    let result = generate_compile_result_for_html_chunk_impl(c_ref, chunk_ref, chunks);
    // SAFETY: HTML chunks have exactly one part-range (i == 0); see
    // `Chunk::write_compile_result_slot` for the disjoint-slot contract.
    unsafe { Chunk::write_compile_result_slot(chunk_ptr, i, result) };
}

#[derive(Default)]
struct EndTagIndices {
    head: Option<u32>,
    body: Option<u32>,
    html: Option<u32>,
}

struct HTMLLoader<'a> {
    linker: &'a LinkerContext<'a>,
    import_records: &'a [ImportRecord],
    current_import_record_index: u32,
    /// Backref to this task's HTML chunk (an element of `*chunks`). The chunk
    /// outlives this `HTMLLoader` (link-step duration), so `BackRef`'s
    /// owner-outlives-holder invariant holds and reads go through safe `Deref`.
    chunk: bun_ptr::BackRef<Chunk>,
    chunks: *mut [Chunk],
    compile_to_standalone_html: bool,
    output: Vec<u8>,
    end_tag_indices: EndTagIndices,
    added_head_tags: bool,
    added_body_script: bool,
}

impl<'a> HTMLProcessorHandler for HTMLLoader<'a> {
    fn on_write_html(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn on_html_parse_error(&mut self, err: &[u8]) {
        bun_core::Output::panic(format_args!(
            "Parsing HTML during replacement phase errored, which should never happen since the first pass succeeded: {}",
            BStr::new(err)
        ));
    }

    fn on_tag(
        &mut self,
        element: &mut lol::html_content::Element,
        _path: &[u8],
        url_attribute: &[u8],
        _kind: ImportKind,
    ) {
        if self.current_import_record_index as usize >= self.import_records.len() {
            bun_core::Output::panic(format_args!(
                "Assertion failure in HTMLLoader.onTag: current_import_record_index ({}) >= import_records.len ({})",
                self.current_import_record_index,
                self.import_records.len()
            ));
        }

        let import_record: &ImportRecord =
            &self.import_records[self.current_import_record_index as usize];
        self.current_import_record_index += 1;

        let parse_graph = self.linker.parse_graph();
        let unique_key_for_additional_files: &[u8] = if import_record.source_index.is_valid() {
            &parse_graph
                .input_files
                .items_unique_key_for_additional_file()[import_record.source_index.get() as usize]
        } else {
            b""
        };
        let loader: Loader = if import_record.source_index.is_valid() {
            parse_graph.input_files.items_loader()[import_record.source_index.get() as usize]
        } else {
            Loader::File
        };

        if import_record
            .flags
            .contains(ImportRecordFlags::IS_EXTERNAL_WITHOUT_SIDE_EFFECTS)
        {
            debug!(
                "Leaving external import: {}",
                BStr::new(import_record.path.text)
            );
            return;
        }

        // Helper: in lol_html 3.0, attribute names and values are &str, not &[u8].
        let url_attr_str = std::str::from_utf8(url_attribute).unwrap_or("");

        if self.linker.dev_server.is_some() {
            if !unique_key_for_additional_files.is_empty() {
                let val = std::str::from_utf8(unique_key_for_additional_files).unwrap_or("");
                element
                    .set_attribute(url_attr_str, val)
                    .unwrap_or_else(|_| panic!("unexpected error from Element.setAttribute"));
            } else if import_record.path.is_disabled
                || loader.is_javascript_like()
                || loader.is_css()
            {
                element.remove();
            } else {
                let val = std::str::from_utf8(import_record.path.pretty).unwrap_or("");
                element
                    .set_attribute(url_attr_str, val)
                    .unwrap_or_else(|_| panic!("unexpected error from Element.setAttribute"));
            }
            return;
        }

        if import_record.source_index.is_invalid() {
            debug!(
                "Leaving import with invalid source index: {}",
                BStr::new(import_record.path.text)
            );
            return;
        }

        if loader.is_javascript_like() || loader.is_css() {
            // Remove the original non-external tags
            element.remove();
            return;
        }

        if self.compile_to_standalone_html && import_record.source_index.is_valid() {
            // In standalone HTML mode, inline assets as data: URIs
            let url_for_css =
                parse_graph.ast.items_url_for_css()[import_record.source_index.get() as usize];
            if !url_for_css.is_empty() {
                let val = std::str::from_utf8(url_for_css).unwrap_or("");
                element
                    .set_attribute(url_attr_str, val)
                    .unwrap_or_else(|_| panic!("unexpected error from Element.setAttribute"));
                return;
            }
        }

        if !unique_key_for_additional_files.is_empty() {
            // Replace the external href/src with the unique key so that we later will rewrite it to the final URL or pathname
            let val = std::str::from_utf8(unique_key_for_additional_files).unwrap_or("");
            element
                .set_attribute(url_attr_str, val)
                .unwrap_or_else(|_| panic!("unexpected error from Element.setAttribute"));
            return;
        }
    }

    fn on_head_tag(&mut self, element: &mut lol::html_content::Element) -> bool {
        // In lol_html 3.0, on_end_tag takes EndTagHandler<'static> which is
        // Box<dyn FnOnce(&mut EndTag<'_>) -> HandlerResult + 'static>.
        // We erase the pointer type to c_void to make the closure 'static
        // (the raw pointer doesn't capture lifetime info, but the rewriter
        // runs synchronously so the pointer remains valid).
        let this_ptr = core::ptr::from_mut::<Self>(self).cast::<core::ffi::c_void>();
        let handler: lol::EndTagHandler<'static> = Box::new(move |end_tag: &mut lol::html_content::EndTag| {
            // SAFETY: `this_ptr` was set from `&mut Self` and is valid for the
            // lifetime of the rewriter.
            let this = unsafe { &mut *this_ptr.cast::<Self>() };
            Self::end_head_tag_handler(this, end_tag)
        });
        element.on_end_tag(handler).is_err()
    }

    fn on_html_tag(&mut self, element: &mut lol::html_content::Element) -> bool {
        let this_ptr = core::ptr::from_mut::<Self>(self).cast::<core::ffi::c_void>();
        let handler: lol::EndTagHandler<'static> = Box::new(move |end_tag: &mut lol::html_content::EndTag| {
            let this = unsafe { &mut *this_ptr.cast::<Self>() };
            Self::end_html_tag_handler(this, end_tag)
        });
        element.on_end_tag(handler).is_err()
    }

    fn on_body_tag(&mut self, element: &mut lol::html_content::Element) -> bool {
        let this_ptr = core::ptr::from_mut::<Self>(self).cast::<core::ffi::c_void>();
        let handler: lol::EndTagHandler<'static> = Box::new(move |end_tag: &mut lol::html_content::EndTag| {
            let this = unsafe { &mut *this_ptr.cast::<Self>() };
            Self::end_body_tag_handler(this, end_tag)
        });
        element.on_end_tag(handler).is_err()
    }
}

impl<'a> HTMLLoader<'a> {
    /// This is called for head, body, and html; whichever ends up coming first.
    fn add_head_tags(&mut self, end_tag: &mut lol::html_content::EndTag) {
        if self.added_head_tags {
            return;
        }
        self.added_head_tags = true;

        // PERF(port): was stack-fallback (std.heap.stackFallback(256))
        let slices = self.get_head_tags();
        for slice in slices.as_slice() {
            // In lol_html 3.0, before() takes (&str, ContentType) and returns ().
            let content_str = std::str::from_utf8(slice).unwrap_or("");
            end_tag.before(content_str, lol::html_content::ContentType::Html);
        }
    }

    /// Insert inline script before </body> so DOM elements are available.
    fn add_body_tags(&mut self, end_tag: &mut lol::html_content::EndTag) {
        if self.added_body_script {
            return;
        }
        self.added_body_script = true;

        // PERF(port): was stack-fallback (std.heap.stackFallback(256))
        // SAFETY: `self.chunks` raw `*mut [Chunk]` valid for the link step; sole live `&mut`.
        let chunks = unsafe { &mut *self.chunks };
        if let Some(js_chunk) = self.chunk.get_js_chunk_for_html(chunks) {
            let mut script = Vec::new();
            write!(
                &mut script,
                "<script type=\"module\">{}</script>",
                BStr::new(js_chunk.unique_key)
            )
            .unwrap();
            let content_str = std::str::from_utf8(&script).unwrap_or("");
            end_tag.before(content_str, lol::html_content::ContentType::Html);
        }
    }

    fn get_head_tags(&self) -> Vec<Vec<u8>> {
        // PERF(port): was stack-fallback arena; now heap Vec<u8>
        let mut array: Vec<Vec<u8>> = Vec::with_capacity(2);
        // `self.chunk` is a `BackRef` (safe `Deref`).
        let chunk: &Chunk = &self.chunk;
        // SAFETY: `chunks` raw pointer valid for the link step; sole live `&mut`.
        let chunks = unsafe { &mut *self.chunks };
        if self.compile_to_standalone_html {
            // In standalone HTML mode, only put CSS in <head>; JS goes before </body>
            if let Some(css_chunk) = chunk.get_css_chunk_for_html(chunks) {
                let mut style_tag = Vec::new();
                write!(
                    &mut style_tag,
                    "<style>{}</style>",
                    BStr::new(css_chunk.unique_key)
                )
                .unwrap();
                array.push(style_tag);
            }
        } else {
            // Put CSS before JS to reduce chances of flash of unstyled content
            if let Some(css_chunk) = chunk.get_css_chunk_for_html(chunks) {
                let mut link_tag = Vec::new();
                write!(
                    &mut link_tag,
                    "<link rel=\"stylesheet\" crossorigin href=\"{}\">",
                    BStr::new(css_chunk.unique_key)
                )
                .unwrap();
                array.push(link_tag);
            }
            if let Some(js_chunk) = chunk.get_js_chunk_for_html(chunks) {
                // type="module" scripts do not block rendering, so it is okay to put them in head
                let mut script = Vec::new();
                write!(
                    &mut script,
                    "<script type=\"module\" crossorigin src=\"{}\"></script>",
                    BStr::new(js_chunk.unique_key)
                )
                .unwrap();
                array.push(script);
            }
        }
        array
    }

    /// Handle end head tag — called as a closure from on_head_tag.
    fn end_head_tag_handler(
        this: &mut Self,
        _end_tag: &mut lol::html_content::EndTag,
    ) -> lol::HandlerResult {
        if this.linker.dev_server.is_none() {
            this.add_head_tags(_end_tag);
        } else {
            this.end_tag_indices.head = Some(u32::try_from(this.output.len()).expect("int cast"));
        }
        Ok(())
    }

    /// Handle end body tag — called as a closure from on_body_tag.
    fn end_body_tag_handler(
        this: &mut Self,
        _end_tag: &mut lol::html_content::EndTag,
    ) -> lol::HandlerResult {
        if this.linker.dev_server.is_none() {
            if this.compile_to_standalone_html {
                // In standalone mode, insert JS before </body> so DOM is available
                this.add_body_tags(_end_tag);
            } else {
                this.add_head_tags(_end_tag);
            }
        } else {
            this.end_tag_indices.body = Some(u32::try_from(this.output.len()).expect("int cast"));
        }
        Ok(())
    }

    /// Handle end html tag — called as a closure from on_html_tag.
    fn end_html_tag_handler(
        this: &mut Self,
        _end_tag: &mut lol::html_content::EndTag,
    ) -> lol::HandlerResult {
        if this.linker.dev_server.is_none() {
            if this.compile_to_standalone_html {
                // Fallback: if no </body> was found, insert both CSS and JS before </html>
                this.add_head_tags(_end_tag);
                this.add_body_tags(_end_tag);
            } else {
                this.add_head_tags(_end_tag);
            }
        } else {
            this.end_tag_indices.html = Some(u32::try_from(this.output.len()).expect("int cast"));
        }
        Ok(())
    }
}

/// `chunk` is the HTML chunk being compiled (held read-only via `BackRef`
/// inside `HTMLLoader`). `chunks` is the raw `*mut [Chunk]` from
/// `GenerateChunkCtx.chunks` (write provenance, valid for the link step) —
/// stored as-is and only deref'd at the guarded `&mut *` sites in
/// `HTMLLoader::{on_tag,get_head_tags}` and the standalone-HTML branch below.
fn generate_compile_result_for_html_chunk_impl<'a>(
    c: &'a LinkerContext<'a>,
    chunk: &Chunk,
    chunks: *mut [Chunk],
) -> CompileResult {
    let parse_graph = c.parse_graph();
    let sources = parse_graph.input_files.items_source();
    let import_records = c.graph.ast.items_import_records();
    let source_index = chunk.entry_point.source_index();

    // HTML bundles for dev server must be allocated to it, as it must outlive
    // the bundle task. See `DevServer.RouteBundle.HTML.bundled_html_text`
    // TODO(port): Zig used `dev.arena()` vs `worker.arena` to control output ownership.
    // In Rust with global mimalloc this distinction collapses; verify DevServer ownership.

    // `c.log` is now `*mut Log` (raw backref); copy directly. The HTMLLoader.log
    // field is currently dead_code, so no write actually occurs through this
    // pointer today.
    let log: *mut Log = c.log;
    let minify_whitespace = c.options.minify_whitespace;
    let compile_to_standalone_html = c.options.compile_to_standalone_html;
    let has_dev_server = c.dev_server.is_some();
    let contents: &[u8] = &sources[source_index as usize].contents;
    let records = import_records[source_index as usize].as_slice();

    let _ = (source_index, log, minify_whitespace);
    let mut html_loader = HTMLLoader {
        linker: c,
        import_records: records,
        current_import_record_index: 0,
        compile_to_standalone_html,
        chunk: bun_ptr::BackRef::new(chunk),
        chunks,
        output: Vec::new(),
        end_tag_indices: EndTagIndices {
            html: None,
            body: None,
            head: None,
        },
        added_head_tags: false,
        added_body_script: false,
    };

    HTMLProcessor::<HTMLLoader, true>::run(&mut html_loader, contents)
        .unwrap_or_else(|_| panic!("unexpected error from HTMLProcessor.run"));

    // There are some cases where invalid HTML will make it so </head> is
    // never emitted, even if the literal text DOES appear. These cases are
    // along the lines of having a self-closing tag for a non-self closing
    // element. In this case, head_end_tag_index will be 0, and a simple
    // search through the page is done to find the "</head>"
    // See https://github.com/oven-sh/bun/issues/17554
    let script_injection_offset: u32 = if has_dev_server {
        'brk: {
            if let Some(head) = html_loader.end_tag_indices.head {
                break 'brk head;
            }
            if let Some(head) = strings::index_of(&html_loader.output, b"</head>") {
                break 'brk u32::try_from(head).expect("int cast");
            }
            if let Some(body) = html_loader.end_tag_indices.body {
                break 'brk body;
            }
            if let Some(html) = html_loader.end_tag_indices.html {
                break 'brk html;
            }
            u32::try_from(html_loader.output.len()).expect("int cast") // inject at end of file.
        }
    } else {
        'brk: {
            if !html_loader.added_head_tags || !html_loader.added_body_script {
                // PERF(port): @branchHint(.cold) — this is if the document is missing all head, body, and html elements.
                // PERF(port): was stack-fallback (std.heap.stackFallback(256))
                if !html_loader.added_head_tags {
                    let slices = html_loader.get_head_tags();
                    for slice in slices.as_slice() {
                        html_loader.output.extend_from_slice(slice);
                    }
                    html_loader.added_head_tags = true;
                }
                if !html_loader.added_body_script {
                    if html_loader.compile_to_standalone_html {
                        // SAFETY: `html_loader.chunks` raw `*mut [Chunk]` valid for the link step; sole live `&mut`.
                        let chunks = unsafe { &mut *html_loader.chunks };
                        if let Some(js_chunk) = html_loader.chunk.get_js_chunk_for_html(chunks) {
                            let mut script = Vec::new();
                            write!(
                                &mut script,
                                "<script type=\"module\">{}</script>",
                                BStr::new(js_chunk.unique_key)
                            )
                            .unwrap();
                            html_loader.output.extend_from_slice(&script);
                        }
                    }
                    html_loader.added_body_script = true;
                }
            }
            // value is ignored. fail loud if hit in debug
            // TODO(port): Zig returned `undefined` in debug to fail loud; Rust has no direct equivalent.
            break 'brk 0;
        }
    };

    CompileResult::Html {
        // TODO(port): Zig returned `output.items` (slice into the ArrayList). Here we hand over the Vec.
        code: html_loader.output.into_boxed_slice(),
        source_index,
        script_injection_offset,
    }
}

pub use crate::{DeferredBatchTask, ParseTask, ThreadPool};

// ported from: src/bundler/linker_context/generateCompileResultForHtmlChunk.zig
