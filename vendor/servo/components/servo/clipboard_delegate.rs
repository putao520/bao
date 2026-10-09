/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use servo_base::generic_channel::GenericCallback;

use crate::WebView;

pub struct StringRequest {
    pub(crate) result_sender: GenericCallback<Result<String, String>>,
    response_sent: bool,
}

impl StringRequest {
    pub fn success(mut self, string: String) {
        let _ = self.result_sender.send(Ok(string));
        self.response_sent = true;
    }

    pub fn failure(mut self, message: String) {
        let _ = self.result_sender.send(Err(message));
        self.response_sent = true;
    }
}

impl From<GenericCallback<Result<String, String>>> for StringRequest {
    fn from(result_sender: GenericCallback<Result<String, String>>) -> Self {
        Self {
            result_sender,
            response_sent: false,
        }
    }
}

impl Drop for StringRequest {
    fn drop(&mut self) {
        if !self.response_sent {
            let _ = self
                .result_sender
                .send(Err("No response sent to request.".into()));
        }
    }
}

/// A delegate that is responsible for accessing the system clipboard. On Mac, Windows, and
/// Linux if the `clipboard` feature is enabled, a default delegate is automatically used
/// that implements clipboard support. An embedding application can override this delegate
/// by using this trait.
pub trait ClipboardDelegate {
    /// A request to clear all contents of the system clipboard.
    fn clear(&self, _webview: WebView) {}

    /// A request to get the text contents of the system clipboard. Once the contents are
    /// retrieved the embedder should call [`StringRequest::success`] with the text or
    /// [`StringRequest::failure`] with a failure message.
    fn get_text(&self, _webview: WebView, _request: StringRequest) {}

    /// A request to set the text contents of the system clipboard to `new_contents`.
    fn set_text(&self, _webview: WebView, _new_contents: String) {}

    /// A request to get the HTML contents of the system clipboard. Once the contents are
    /// retrieved the embedder should call [`StringRequest::success`] with the HTML or
    /// with an empty string when the clipboard has no HTML representation.
    fn get_html(&self, _webview: WebView, _request: StringRequest) {}

    /// A request to set the HTML contents of the system clipboard to `new_contents`.
    fn set_html(&self, _webview: WebView, _new_contents: String) {}
}

pub(crate) struct DefaultClipboardDelegate;

impl ClipboardDelegate for DefaultClipboardDelegate {
    fn clear(&self, _webview: WebView) {
        clipboard::clear();
    }

    fn get_text(&self, _webview: WebView, request: StringRequest) {
        clipboard::get_text(request);
    }

    fn set_text(&self, _webview: WebView, new_contents: String) {
        clipboard::set_text(new_contents);
    }

    fn get_html(&self, _webview: WebView, request: StringRequest) {
        clipboard::get_html(request);
    }

    fn set_html(&self, _webview: WebView, new_contents: String) {
        clipboard::set_html(new_contents);
    }
}

mod fallback_clipboard {
    use std::sync::{LockResult, Mutex, OnceLock};

    use crate::clipboard_delegate::StringRequest;

    /// If the clipboard cannot be accessed, we fall back to a simple `String` to store
    /// text for the clipboard. This obviously does not work across processes.
    static SHARED_FALLBACK_CLIPBOARD: OnceLock<Mutex<String>> = OnceLock::new();

    /// The fallback clipboard's HTML representation. Like the text slot above this is
    /// in-memory only; `None` means the clipboard has no HTML representation. Setting
    /// plain text replaces the whole clipboard, so it clears this slot.
    static SHARED_FALLBACK_CLIPBOARD_HTML: OnceLock<Mutex<Option<String>>> = OnceLock::new();

    fn with_shared_clipboard(callback: impl FnOnce(&mut String)) {
        let clipboard_mutex =
            SHARED_FALLBACK_CLIPBOARD.get_or_init(|| Mutex::new(Default::default()));
        if let LockResult::Ok(mut string) = clipboard_mutex.lock() {
            callback(&mut string)
        }
    }

    fn with_shared_clipboard_html(callback: impl FnOnce(&mut Option<String>)) {
        let clipboard_mutex =
            SHARED_FALLBACK_CLIPBOARD_HTML.get_or_init(|| Mutex::new(Default::default()));
        if let LockResult::Ok(mut string) = clipboard_mutex.lock() {
            callback(&mut string)
        }
    }

    pub(super) fn clear() {
        with_shared_clipboard(|clipboard_string| {
            clipboard_string.clear();
        });
        with_shared_clipboard_html(|clipboard_html| {
            *clipboard_html = None;
        });
    }

    pub(super) fn get_text(request: StringRequest) {
        with_shared_clipboard(move |clipboard_string| request.success(clipboard_string.clone()));
    }

    pub(super) fn set_text(new_contents: String) {
        with_shared_clipboard(move |clipboard_string| {
            *clipboard_string = new_contents;
        });
        // Setting the text replaces the whole clipboard, dropping any HTML
        // representation of the previous contents.
        with_shared_clipboard_html(|clipboard_html| {
            *clipboard_html = None;
        });
    }

    pub(super) fn get_html(request: StringRequest) {
        with_shared_clipboard_html(move |clipboard_html| {
            request.success(clipboard_html.clone().unwrap_or_default())
        });
    }

    pub(super) fn set_html(new_contents: String) {
        with_shared_clipboard_html(move |clipboard_html| {
            *clipboard_html = Some(new_contents);
        });
    }
}

#[cfg(all(
    feature = "clipboard",
    not(any(target_os = "android", target_env = "ohos"))
))]
mod clipboard {
    use std::sync::OnceLock;

    use arboard::Clipboard;
    use parking_lot::Mutex;

    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    /// A shared clipboard for use by the [`DefaultClipboardDelegate`](super::DefaultClipboardDelegate).
    /// This is protected by a mutex so that it can only be used by one thread at a time.
    /// The `arboard` documentation suggests that more than one thread shouldn't try to access
    /// the Windows clipboard at a time. See <https://docs.rs/arboard/latest/arboard/struct.Clipboard.html>.
    static SHARED_CLIPBOARD: OnceLock<Option<Mutex<Clipboard>>> = OnceLock::new();

    /// The last text written through [`set_text`], kept as the alternate text of the
    /// HTML representation: `arboard::Clipboard::set_html` replaces the whole
    /// clipboard, so the plain-text representation of an engine copy that writes both
    /// formats must be passed along as the alt text to survive.
    static LAST_TEXT_FOR_HTML_ALT: OnceLock<Mutex<String>> = OnceLock::new();

    fn with_shared_clipboard<ResultType>(
        callback: impl FnOnce(&mut Clipboard) -> Result<ResultType, arboard::Error>,
    ) -> Result<ResultType, arboard::Error> {
        match SHARED_CLIPBOARD.get_or_init(|| Clipboard::new().ok().map(Mutex::new)) {
            Some(clipboard_mutex) => callback(&mut clipboard_mutex.lock()),
            None => Err(arboard::Error::ClipboardNotSupported),
        }
    }

    pub(super) fn clear() {
        if with_shared_clipboard(|clipboard| clipboard.clear()).is_err() {
            fallback_clipboard::clear();
        }
        if let Some(last_text) = LAST_TEXT_FOR_HTML_ALT.get() {
            last_text.lock().clear();
        }
    }

    pub(super) fn get_text(request: StringRequest) {
        if let Ok(text) = with_shared_clipboard(|clipboard| clipboard.get_text()) {
            request.success(text);
            return;
        };
        fallback_clipboard::get_text(request);
    }

    pub(super) fn set_text(new_contents: String) {
        if with_shared_clipboard(|clipboard| clipboard.set_text(&new_contents)).is_err() {
            fallback_clipboard::set_text(new_contents);
            return;
        }
        *LAST_TEXT_FOR_HTML_ALT
            .get_or_init(|| Mutex::new(String::new()))
            .lock() = new_contents;
    }

    pub(super) fn get_html(request: StringRequest) {
        // arboard 3.6.1 has no API to read the OS clipboard's HTML representation
        // back (`get_html` is not available at this dependency version), so the HTML
        // format is mirrored into the in-memory fallback store by `set_html` and
        // read back from there. Only HTML written by this process is visible until
        // arboard grows an HTML read API.
        fallback_clipboard::get_html(request);
    }

    pub(super) fn set_html(new_contents: String) {
        let alternate_text = LAST_TEXT_FOR_HTML_ALT
            .get_or_init(|| Mutex::new(String::new()))
            .lock()
            .clone();
        let mirrored = new_contents.clone();
        if with_shared_clipboard(|clipboard| clipboard.set_html(&new_contents, Some(&alternate_text)))
            .is_err()
        {
            log::warn!(
                "arboard set_html failed; keeping the HTML format in the in-memory \
                 fallback_clipboard only"
            );
        }
        fallback_clipboard::set_html(mirrored);
    }
}

#[cfg(all(feature = "clipboard", target_env = "ohos"))]
mod clipboard {
    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    pub(super) fn clear() {
        if let Err(error) = ohos_pasteboard::clear() {
            log::warn!(
                "OHOS pasteboard clear failed ({error}); using in-memory fallback_clipboard"
            );
            fallback_clipboard::clear();
        }
    }

    pub(super) fn get_text(request: StringRequest) {
        match ohos_pasteboard::get_text() {
            Ok(text) => request.success(text),
            Err(ohos_pasteboard::Error::NoText) => request.success(String::new()),
            Err(error) => {
                log::warn!(
                    "OHOS pasteboard get_text failed ({error}); using in-memory fallback_clipboard"
                );
                fallback_clipboard::get_text(request);
            },
        }
    }

    pub(super) fn set_text(new_contents: String) {
        if let Err(error) = ohos_pasteboard::set_text(&new_contents) {
            log::warn!(
                "OHOS pasteboard set_text failed ({error}); using in-memory fallback_clipboard"
            );
            fallback_clipboard::set_text(new_contents);
        }
    }

    pub(super) fn get_html(request: StringRequest) {
        // The OHOS pasteboard API has no HTML representation; keep the HTML format
        // in the in-memory fallback store only.
        fallback_clipboard::get_html(request);
    }

    pub(super) fn set_html(new_contents: String) {
        fallback_clipboard::set_html(new_contents);
    }
}

#[cfg(any(
    not(feature = "clipboard"),
    all(feature = "clipboard", target_os = "android")
))]
mod clipboard {
    use super::StringRequest;
    use crate::clipboard_delegate::fallback_clipboard;

    pub(super) fn clear() {
        fallback_clipboard::clear();
    }

    pub(super) fn get_text(request: StringRequest) {
        fallback_clipboard::get_text(request);
    }

    pub(super) fn set_text(new_contents: String) {
        fallback_clipboard::set_text(new_contents);
    }

    pub(super) fn get_html(request: StringRequest) {
        fallback_clipboard::get_html(request);
    }

    pub(super) fn set_html(new_contents: String) {
        fallback_clipboard::set_html(new_contents);
    }
}
