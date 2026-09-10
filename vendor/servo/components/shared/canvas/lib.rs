/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![crate_name = "servo_canvas_traits"]
#![crate_type = "rlib"]
#![deny(unsafe_code)]

use crossbeam_channel::Sender;
use euclid::default::Size2D;
use profile_traits::mem::ReportsChan;
use servo_base::id::WebViewId;

use crate::canvas::CanvasId;

pub mod canvas;
#[macro_use]
pub mod webgl;

pub enum ConstellationCanvasMsg {
    Create {
        sender: Sender<Option<CanvasId>>,
        size: Size2D<u64>,
        /// Bao (BUN-EVOLUTION R53-A phase 2): the owning-webview identity
        /// for the canvas being created — stamped by the script side at
        /// `CanvasState::new` (`GlobalScope::egress_webview_id`: worker/SW
        /// realms carry their host page's id), stored by the paint thread
        /// alongside the canvas, and consulted at the `GetImageData` noise
        /// choke point to resolve the per-WebViewId noise config. `None` =
        /// identity-less realm → process-global fallback (pre-R53
        /// semantics).
        webview_id: Option<WebViewId>,
    },
    CollectMemoryReport(ReportsChan),
    Exit(Sender<()>),
}
