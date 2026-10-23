/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::MutableHandleValue;
use paint_api::display_list::PaintTimingInfo;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object;
use servo_base::cross_process_instant::CrossProcessInstant;
use servo_base::id::PipelineId;
use time::Duration;

use super::long_animation_frame_recorder::{
    FinishedFrameTiming, SCRIPT_REPORTING_THRESHOLD,
};
use super::performance::ToDOMHighResTimeStamp;
use super::performanceentry::{EntryType, PerformanceEntry};
use super::performancescripttiming::PerformanceScriptTiming;
use crate::dom::bindings::codegen::Bindings::PerformanceBinding::DOMHighResTimeStamp;
use crate::dom::bindings::codegen::Bindings::PerformanceLongAnimationFrameTimingBinding::PerformanceLongAnimationFrameTimingMethods;
use crate::dom::bindings::codegen::Bindings::PerformanceScriptTimingBinding::ScriptWindowAttribution;
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::utils::to_frozen_array;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::bindings::str::DOMString;
use crate::dom::globalscope::GlobalScope;

/// <https://w3c.github.io/long-animation-frames/#the-performancelonganimationframetiming-interface>
#[dom_struct]
pub(crate) struct PerformanceLongAnimationFrameTiming {
    entry: PerformanceEntry,
    #[no_trace]
    render_start: Option<CrossProcessInstant>,
    #[no_trace]
    style_and_layout_start: Option<CrossProcessInstant>,
    #[no_trace]
    #[ignore_malloc_size_of = "No MallocSizeOf support for `time` crate"]
    blocking_duration: Duration,
    #[no_trace]
    first_ui_event_timestamp: Option<CrossProcessInstant>,
    #[no_trace]
    #[ignore_malloc_size_of = "No MallocSizeOf support for `time` crate"]
    style_duration: Duration,
    #[no_trace]
    #[ignore_malloc_size_of = "No MallocSizeOf support for `time` crate"]
    layout_duration: Duration,
    script_count: u32,
    scripts: DomRefCell<Vec<Dom<PerformanceScriptTiming>>>,
    #[no_trace]
    presentation_time: Option<CrossProcessInstant>,
}

impl PerformanceLongAnimationFrameTiming {
    /// <https://w3c.github.io/long-animation-frames/#dfn-blocking-duration>
    ///
    /// Sum the durations of tasks over 50ms, subtracting 50ms from each. When a
    /// rendering update happened, its duration is added to the longest task
    /// first, because that task is what delayed the start of the rendering
    /// update.
    fn compute_blocking_duration(frame: &FinishedFrameTiming) -> Duration {
        let mut durations: Vec<Duration> = frame.tasks.iter().map(|(_, d)| *d).collect();
        if let Some(render_start) = frame.render_start {
            let render_duration = frame.end - render_start;
            if durations.is_empty() {
                durations.push(render_duration);
            } else {
                let longest = durations
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, duration)| **duration)
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                durations[longest] = durations[longest] + render_duration;
            }
        }

        durations
            .into_iter()
            .filter(|duration| *duration > super::long_animation_frame_recorder::LONG_FRAME_THRESHOLD)
            .map(|duration| {
                duration - super::long_animation_frame_recorder::LONG_FRAME_THRESHOLD
            })
            .fold(Duration::ZERO, |total, duration| total + duration)
    }

    fn new_inherited(
        frame: FinishedFrameTiming,
        scripts: Vec<Dom<PerformanceScriptTiming>>,
        script_count: u32,
        paint_timing_info: PaintTimingInfo,
    ) -> PerformanceLongAnimationFrameTiming {
        let blocking_duration = Self::compute_blocking_duration(&frame);
        PerformanceLongAnimationFrameTiming {
            entry: PerformanceEntry::new_inherited(
                DOMString::from("long-animation-frame"),
                EntryType::LongAnimationFrame,
                Some(frame.start),
                frame.end - frame.start,
            ),
            render_start: frame.render_start,
            style_and_layout_start: frame.style_and_layout_start,
            blocking_duration,
            first_ui_event_timestamp: frame.first_ui_event_timestamp,
            style_duration: frame.style_duration,
            layout_duration: frame.layout_duration,
            script_count,
            scripts: DomRefCell::new(scripts),
            presentation_time: paint_timing_info.presentation_time(),
        }
    }

    /// Materialize the entry and the script timings it reports.
    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        pipeline_id: PipelineId,
        frame: FinishedFrameTiming,
        paint_timing_info: PaintTimingInfo,
    ) -> DomRoot<PerformanceLongAnimationFrameTiming> {
        let script_count = frame.scripts.len() as u32;
        let mut scripts = Vec::new();
        for info in frame.scripts.iter() {
            if info.duration < SCRIPT_REPORTING_THRESHOLD {
                continue;
            }
            let attribution = if info.window_pipeline_id == Some(pipeline_id) {
                ScriptWindowAttribution::Self_
            } else {
                ScriptWindowAttribution::Other
            };
            let script = PerformanceScriptTiming::new(cx, global, info.clone(), attribution);
            scripts.push(Dom::from_ref(&*script));
        }

        reflect_dom_object(
            cx,
            Box::new(PerformanceLongAnimationFrameTiming::new_inherited(
                frame,
                scripts,
                script_count,
                paint_timing_info,
            )),
            global,
        )
    }
}

impl PerformanceLongAnimationFrameTimingMethods<crate::DomTypeHolder>
    for PerformanceLongAnimationFrameTiming
{
    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-renderstart>
    fn RenderStart(&self, cx: &mut JSContext) -> DOMHighResTimeStamp {
        self.global()
            .performance(cx)
            .maybe_to_dom_high_res_time_stamp(self.render_start)
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-styleandlayoutstart>
    fn StyleAndLayoutStart(&self, cx: &mut JSContext) -> DOMHighResTimeStamp {
        self.global()
            .performance(cx)
            .maybe_to_dom_high_res_time_stamp(self.style_and_layout_start)
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-blockingduration>
    fn BlockingDuration(&self) -> DOMHighResTimeStamp {
        self.blocking_duration.to_dom_high_res_time_stamp()
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-firstuieventtimestamp>
    fn FirstUIEventTimestamp(&self, cx: &mut JSContext) -> DOMHighResTimeStamp {
        self.global()
            .performance(cx)
            .maybe_to_dom_high_res_time_stamp(self.first_ui_event_timestamp)
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-styleduration>
    fn StyleDuration(&self) -> DOMHighResTimeStamp {
        self.style_duration.to_dom_high_res_time_stamp()
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-layoutduration>
    fn LayoutDuration(&self) -> DOMHighResTimeStamp {
        self.layout_duration.to_dom_high_res_time_stamp()
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-scriptcount>
    fn ScriptCount(&self) -> u32 {
        self.script_count
    }

    /// <https://w3c.github.io/long-animation-frames/#dom-performancelonganimationframetiming-scripts>
    fn Scripts(&self, cx: &mut JSContext, retval: MutableHandleValue) {
        let scripts: Vec<DomRoot<PerformanceScriptTiming>> =
            self.scripts.borrow().iter().map(|s| s.as_rooted()).collect();
        to_frozen_array(cx, &scripts, retval);
    }

    /// <https://w3c.github.io/paint-timing/#dom-painttimingmixin-painttime>
    ///
    /// The paint time of a long animation frame is the end of the frame.
    /// Compute it from the same independently-clamped components as
    /// `startTime` and `duration` so that `paintTime == startTime + duration`
    /// holds exactly.
    fn PaintTime(&self, cx: &mut JSContext) -> DOMHighResTimeStamp {
        let performance = self.global().performance(cx);
        let start = performance.maybe_to_dom_high_res_time_stamp(self.entry.start_time());
        let duration = self.entry.duration().to_dom_high_res_time_stamp();
        Finite::wrap(*start + *duration)
    }

    /// <https://w3c.github.io/paint-timing/#dom-painttimingmixin-presentationtime>
    fn GetPresentationTime(&self, cx: &mut JSContext) -> Option<DOMHighResTimeStamp> {
        Some(
            self.global()
                .performance(cx)
                .maybe_to_dom_high_res_time_stamp(self.presentation_time),
        )
    }
}
