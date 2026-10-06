/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

pub mod async_runtime;
pub mod connector;
pub mod cookie;
pub mod cookie_storage;
mod decoder;
mod devtools;
mod disk_cache;
pub mod embedder;
pub mod filemanager_thread;
mod hosts;
pub mod hsts;
pub mod http_cache;
pub mod http_loader;
pub mod image_cache;
pub mod local_directory_listing;
pub mod protocols;
pub mod request_interceptor;
pub mod resource_thread;
pub mod subresource_integrity;
#[cfg(feature = "test-util")]
pub mod test_util;
mod websocket_loader;

/// An implementation of the [Fetch specification](https://fetch.spec.whatwg.org/)
pub mod fetch {
    pub mod cors_cache;
    // Bao fusion (U2 phase 0): raw-pointer ownership protocol with the
    // bun_http AsyncHTTP heap boxes — same discipline as fetch_async.rs.
    #[allow(unsafe_code)]
    pub mod bun_bridge;
    pub mod fetch_params;
    pub mod headers;
    pub mod methods;
}

/// A module for re-exports of items used in unit tests.
pub mod test {
    pub use crate::decoder::{BodyStreamError, DECODER_BUFFER_SIZE, map_decode_error};
    pub use crate::hosts::parse_hostsfile;
    pub use crate::http_loader::HttpState;
}
