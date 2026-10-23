/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::reflector::reflect_dom_object;

use super::long_animation_frame_recorder::ScriptTimingInfo;
use super::performance::ToDOMHighResTimeStamp;
use super::performanceentry::{EntryType, PerformanceEntry};
use crate::dom::bindings::codegen::Bindings::PerformanceBinding::DOMHighResTimeStamp;
use crate::dom::bindings::codegen::Bindings::PerformanceScriptTimingBinding::{
    PerformanceScriptTimingMethods, ScriptInvokerType, ScriptWindowAttribution,
};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;
use crate::dom::window::Window;

/// <https://w3c.github.io/long-animation-frames/#the-performancescripttiming-interface>
///
/// Script timing entries are materialized together with the
/// [`super::performancelonganimationframetiming::PerformanceLongAnimationFrameTiming`]
/// entry that reports them; the frame entry roots each of them.
#[dom_struct]
pub(crate) struct PerformanceScriptTiming {
    entry: PerformanceEntry,
    #[no_trace]
    info: ScriptTimingInfo,
    attribution: ScriptWindowAttribution,
}

impl PerformanceScriptTiming {
    fn new_inherited(
        info: ScriptTimingInfo,
        attribution: ScriptWindowAttribution,
    ) -> PerformanceScriptTiming {
        PerformanceScriptTiming {
            entry: PerformanceEntry::new_inherited(
                DOMString::from("script"),
                EntryType::Script,
                Some(info.start),
                info.duration,
            ),
            info,
            attribution,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        info: ScriptTimingInfo,
        attribution: ScriptWindowAttribution,
    ) -> DomRoot<PerformanceScriptTiming> {
        reflect_dom_object(
            cx,
            Box::new(PerformanceScriptTiming::new_inherited(info, attribution)),
            global,
        )
    }
}

impl PerformanceScriptTimingMethods<crate::DomTypeHolder> for PerformanceScriptTiming {
    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-invokertype>
    fn InvokerType(&self) -> ScriptInvokerType {
        self.info.invoker_type
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-invoker>
    fn Invoker(&self) -> DOMString {
        DOMString::from(self.info.invoker.clone())
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-executionstart>
    fn ExecutionStart(&self, cx: &mut JSContext) -> DOMHighResTimeStamp {
        // Servo observes script entry points as a single synchronous run, so
        // the start of the entry point is also the start of its execution.
        self.global()
            .performance(cx)
            .to_dom_high_res_time_stamp(self.info.start)
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-sourceurl>
    fn SourceURL(&self) -> DOMString {
        DOMString::from(self.info.source_url.clone())
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-sourcefunctionname>
    fn SourceFunctionName(&self) -> DOMString {
        DOMString::from(self.info.source_function_name.clone())
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-sourcecharposition>
    fn SourceCharPosition(&self) -> i64 {
        self.info.source_char_position
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-sourceline>
    fn SourceLine(&self) -> i64 {
        self.info.source_line
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-sourcecolumn>
    fn SourceColumn(&self) -> i64 {
        self.info.source_column
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-pauseduration>
    fn PauseDuration(&self) -> DOMHighResTimeStamp {
        time::Duration::ZERO.to_dom_high_res_time_stamp()
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-forcedstyleandlayoutduration>
    ///
    /// Servo does not instrument synchronous style and layout calls, so this
    /// implementation-defined value is reported as zero.
    fn ForcedStyleAndLayoutDuration(&self) -> DOMHighResTimeStamp {
        time::Duration::ZERO.to_dom_high_res_time_stamp()
    }

    fn ForcedStyleDuration(&self) -> DOMHighResTimeStamp {
        time::Duration::ZERO.to_dom_high_res_time_stamp()
    }

    fn ForcedLayoutDuration(&self) -> DOMHighResTimeStamp {
        time::Duration::ZERO.to_dom_high_res_time_stamp()
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-window>
    fn GetWindow(&self) -> Option<DomRoot<Window>> {
        match self.attribution {
            ScriptWindowAttribution::Other => None,
            _ => DomRoot::downcast::<Window>(self.global()),
        }
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancescripttiming-windowattribution>
    fn WindowAttribution(&self) -> ScriptWindowAttribution {
        self.attribution
    }
}
