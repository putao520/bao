/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! In-thread measurement state for the Long Animation Frames API.
//!
//! <https://w3c.github.io/long-animation-frames/#sec-processing-model>
//!
//! Long animation frames are measured on the script thread that runs the
//! relevant tasks. A frame starts when the first task that may contribute to
//! a rendering update runs and ends with either that rendering update or, when
//! no rendering update is pending, at the end of the last task. The state is
//! keyed by [`PipelineId`] because a single script thread hosts every pipeline
//! that shares its event loop (same-origin iframes and same-origin
//! navigations).
//!
//! This module deliberately stores no DOM pointers: script timings only keep
//! the pipeline identity of the realm that invoked them, which is resolved to
//! a `Window` when the entry is materialized.

use std::cell::RefCell;
use std::collections::HashMap;

use servo_base::cross_process_instant::CrossProcessInstant;
use servo_base::id::PipelineId;
use time::Duration;

use crate::dom::bindings::codegen::Bindings::PerformanceScriptTimingBinding::ScriptInvokerType;
use crate::dom::window::Window;

/// A frame is a long animation frame when it exceeds this duration.
/// <https://w3c.github.io/long-animation-frames/#dfn-long-animation-frame>
pub(crate) const LONG_FRAME_THRESHOLD: Duration = Duration::milliseconds(50);

/// Script entry points shorter than this are recorded in `scriptCount` but are
/// not exposed in the `scripts` array.
/// <https://w3c.github.io/long-animation-frames/#dfn-script-timing-info>
pub(crate) const SCRIPT_REPORTING_THRESHOLD: Duration = Duration::milliseconds(5);

/// A single script entry point observed while a frame was being built.
#[derive(Clone, MallocSizeOf)]
pub(crate) struct ScriptTimingInfo {
    pub(crate) invoker_type: ScriptInvokerType,
    pub(crate) invoker: String,
    pub(crate) start: CrossProcessInstant,
    #[ignore_malloc_size_of = "No MallocSizeOf support for `time` crate"]
    pub(crate) duration: Duration,
    pub(crate) source_url: String,
    pub(crate) source_function_name: String,
    pub(crate) source_char_position: i64,
    pub(crate) source_line: i64,
    pub(crate) source_column: i64,
    /// The pipeline of the realm that invoked the entry point. Used to compute
    /// `window`/`windowAttribution` relative to the entry's window.
    pub(crate) window_pipeline_id: Option<PipelineId>,
}

/// Measurement state for the frame currently being built for one pipeline.
struct FrameTimingInfo {
    pipeline_id: PipelineId,
    start: CrossProcessInstant,
    /// `(start, duration)` for every task attributed to this frame.
    tasks: Vec<(CrossProcessInstant, Duration)>,
    scripts: Vec<ScriptTimingInfo>,
    first_ui_event_timestamp: Option<CrossProcessInstant>,
    render_start: Option<CrossProcessInstant>,
    style_and_layout_start: Option<CrossProcessInstant>,
    style_duration: Duration,
    layout_duration: Duration,
    /// Whether any rendering work (animation frame callbacks, resize
    /// observers, style/layout or display list generation) ran for this frame.
    rendering_work: bool,
    open_task_start: Option<CrossProcessInstant>,
}

impl FrameTimingInfo {
    fn new(pipeline_id: PipelineId, start: CrossProcessInstant) -> Self {
        Self {
            pipeline_id,
            start,
            tasks: Vec::new(),
            scripts: Vec::new(),
            first_ui_event_timestamp: None,
            render_start: None,
            style_and_layout_start: None,
            style_duration: Duration::ZERO,
            layout_duration: Duration::ZERO,
            rendering_work: false,
            open_task_start: None,
        }
    }
}

/// The fully measured frame handed to the script thread for materialization.
pub(crate) struct FinishedFrameTiming {
    pub(crate) start: CrossProcessInstant,
    pub(crate) end: CrossProcessInstant,
    pub(crate) tasks: Vec<(CrossProcessInstant, Duration)>,
    pub(crate) scripts: Vec<ScriptTimingInfo>,
    pub(crate) first_ui_event_timestamp: Option<CrossProcessInstant>,
    pub(crate) render_start: Option<CrossProcessInstant>,
    pub(crate) style_and_layout_start: Option<CrossProcessInstant>,
    pub(crate) style_duration: Duration,
    pub(crate) layout_duration: Duration,
}

thread_local! {
    static FRAMES: RefCell<HashMap<PipelineId, FrameTimingInfo>> =
        RefCell::new(HashMap::new());
}

fn with_frames<R>(f: impl FnOnce(&mut HashMap<PipelineId, FrameTimingInfo>) -> R) -> R {
    FRAMES.with(|frames| f(&mut frames.borrow_mut()))
}

fn with_frame<R>(
    pipeline_id: PipelineId,
    f: impl FnOnce(&mut FrameTimingInfo) -> R,
) -> Option<R> {
    with_frames(|frames| frames.get_mut(&pipeline_id).map(f))
}

/// Start a task for `pipeline_id`, creating a frame if none is being built.
pub(crate) fn begin_task(pipeline_id: PipelineId, now: CrossProcessInstant) {
    with_frames(|frames| {
        let frame = frames
            .entry(pipeline_id)
            .or_insert_with(|| FrameTimingInfo::new(pipeline_id, now));
        frame.open_task_start = Some(now);
    });
}

/// Finish the task that is currently running for `pipeline_id`.
pub(crate) fn end_task(pipeline_id: PipelineId, now: CrossProcessInstant) {
    with_frame(pipeline_id, |frame| {
        if let Some(start) = frame.open_task_start.take() {
            frame.tasks.push((start, now - start));
        }
    });
}

/// Attribute `start..end` to a task of the frame for `pipeline_id`. Used for
/// work that the event loop performs inside "update the rendering" but which
/// the specification models as a task, such as dispatching input events.
pub(crate) fn record_task(
    pipeline_id: PipelineId,
    start: CrossProcessInstant,
    end: CrossProcessInstant,
) {
    with_frame(pipeline_id, |frame| {
        frame.tasks.push((start, end - start));
    });
}

/// Note that rendering work (for example an animation frame callback) ran in
/// the current rendering update of `pipeline_id`.
pub(crate) fn note_rendering_work(pipeline_id: PipelineId) {
    with_frame(pipeline_id, |frame| frame.rendering_work = true);
}

/// RAII guard recording a script entry point. The entry is appended to the
/// pipeline's frame (if one exists) when the guard is dropped.
pub(crate) struct ScriptEntryGuard {
    pipeline_id: PipelineId,
    script: Option<ScriptTimingInfo>,
}

impl ScriptEntryGuard {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        pipeline_id: PipelineId,
        invoker_type: ScriptInvokerType,
        invoker: String,
        source_url: String,
        source_function_name: String,
        source_char_position: i64,
        source_line: i64,
        source_column: i64,
        window_pipeline_id: Option<PipelineId>,
        now: CrossProcessInstant,
    ) -> Self {
        Self {
            pipeline_id,
            script: Some(ScriptTimingInfo {
                invoker_type,
                invoker,
                start: now,
                duration: Duration::ZERO,
                source_url,
                source_function_name,
                source_char_position,
                source_line,
                source_column,
                window_pipeline_id,
            }),
        }
    }
}

impl Drop for ScriptEntryGuard {
    fn drop(&mut self) {
        let Some(mut script) = self.script.take() else {
            return;
        };
        let now = CrossProcessInstant::now();
        script.duration = now - script.start;
        with_frame(self.pipeline_id, |frame| frame.scripts.push(script));
    }
}

/// Start recording a script entry point invoked by `window`.
pub(crate) fn begin_script_entry(
    window: &Window,
    invoker_type: ScriptInvokerType,
    invoker: String,
) -> ScriptEntryGuard {
    let pipeline_id = window.pipeline_id();
    ScriptEntryGuard::new(
        pipeline_id,
        invoker_type,
        invoker,
        String::new(),
        String::new(),
        -1,
        -1,
        -1,
        Some(pipeline_id),
        CrossProcessInstant::now(),
    )
}

/// Start recording a classic or module script block. `source_url` is the
/// script's base URL.
pub(crate) fn begin_script_block(
    window: &Window,
    invoker_type: ScriptInvokerType,
    source_url: String,
) -> ScriptEntryGuard {
    let pipeline_id = window.pipeline_id();
    ScriptEntryGuard::new(
        pipeline_id,
        invoker_type,
        source_url.clone(),
        source_url,
        String::new(),
        -1,
        -1,
        -1,
        Some(pipeline_id),
        CrossProcessInstant::now(),
    )
}

/// Normalize a script URL for the `invoker`/`sourceURL` attributes.
/// <https://w3c.github.io/long-animation-frames/#create-a-script-entry-point>
///
/// Opaque URLs that can embed arbitrary content (`data:`) only expose their
/// scheme so that script contents are not leaked through the performance
/// timeline.
pub(crate) fn normalize_script_url(url: &str) -> String {
    if url.starts_with("data:") {
        "data:".to_owned()
    } else {
        url.to_owned()
    }
}

/// Whether `event_type` is a UI event whose timestamp is reported as
/// `firstUIEventTimestamp`.
/// <https://w3c.github.io/long-animation-frames/#dfn-first-ui-event-timestamp>
pub(crate) fn is_ui_event_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "click"
            | "auxclick"
            | "dblclick"
            | "mousedown"
            | "mouseup"
            | "mousemove"
            | "mouseover"
            | "mouseout"
            | "pointerdown"
            | "pointerup"
            | "pointermove"
            | "pointerover"
            | "pointerout"
            | "pointercancel"
            | "keydown"
            | "keyup"
            | "keypress"
            | "touchstart"
            | "touchend"
            | "touchmove"
            | "touchcancel"
            | "input"
            | "beforeinput"
            | "wheel"
    )
}

/// Record the timestamp of the first trusted UI event in the current frame.
pub(crate) fn note_ui_event(pipeline_id: PipelineId, timestamp: CrossProcessInstant) {
    with_frame(pipeline_id, |frame| {
        if frame.first_ui_event_timestamp.is_none() {
            frame.first_ui_event_timestamp = Some(timestamp);
        }
    });
}

/// Mark the beginning of a rendering update for `pipeline_id`, creating a
/// frame if no task has started one yet.
pub(crate) fn begin_render(pipeline_id: PipelineId, now: CrossProcessInstant) {
    with_frames(|frames| {
        let frame = frames
            .entry(pipeline_id)
            .or_insert_with(|| FrameTimingInfo::new(pipeline_id, now));
        if frame.render_start.is_none() {
            frame.render_start = Some(now);
        }
    });
}

/// The document has started its style/layout phase.
pub(crate) fn set_style_and_layout_start(pipeline_id: PipelineId, now: CrossProcessInstant) {
    with_frame(pipeline_id, |frame| {
        if frame.style_and_layout_start.is_none() {
            frame.style_and_layout_start = Some(now);
        }
    });
}

/// A rendering update finished. `produced_frame` tells whether the update did
/// any rendering work; `style_duration`/`layout_duration` measure the
/// synchronous style and layout work performed by the document.
pub(crate) fn end_render(
    pipeline_id: PipelineId,
    style_duration: Duration,
    layout_duration: Duration,
    produced_frame: bool,
) {
    with_frame(pipeline_id, |frame| {
        frame.rendering_work = frame.rendering_work || produced_frame;
        frame.style_duration = frame.style_duration + style_duration;
        frame.layout_duration = frame.layout_duration + layout_duration;
        // Keep the end of the frame aligned with the end of the rendering
        // update; the frame is closed by the script thread afterwards.
        frame.open_task_start = None;
    });
}

/// Whether the frame for `pipeline_id` recorded rendering work since its
/// creation.
pub(crate) fn frame_rendered(pipeline_id: PipelineId) -> bool {
    with_frame(pipeline_id, |frame| frame.rendering_work).unwrap_or(false)
}

/// Whether a frame is currently being built for `pipeline_id`.
pub(crate) fn has_open_frame(pipeline_id: PipelineId) -> bool {
    with_frame(pipeline_id, |_| ()).is_some()
}

/// Close and return the frame for `pipeline_id`.
pub(crate) fn take_frame(
    pipeline_id: PipelineId,
    end: CrossProcessInstant,
) -> Option<FinishedFrameTiming> {
    with_frames(|frames| {
        let frame = frames.remove(&pipeline_id)?;
        // A task that was interrupted (for example by an early return during
        // thread shutdown) still contributes its elapsed time.
        let mut tasks = frame.tasks;
        if let Some(start) = frame.open_task_start {
            tasks.push((start, end - start));
        }
        // A frame whose rendering update produced no rendering work does not
        // report a render start; its time is attributed to the tasks and the
        // input event dispatches that ran inside the update.
        let (render_start, style_and_layout_start) = if frame.rendering_work {
            (frame.render_start, frame.style_and_layout_start)
        } else {
            (None, None)
        };
        Some(FinishedFrameTiming {
            start: frame.start,
            end,
            tasks,
            scripts: frame.scripts,
            first_ui_event_timestamp: frame.first_ui_event_timestamp,
            render_start,
            style_and_layout_start,
            style_duration: if frame.rendering_work {
                frame.style_duration
            } else {
                Duration::ZERO
            },
            layout_duration: if frame.rendering_work {
                frame.layout_duration
            } else {
                Duration::ZERO
            },
        })
    })
}

/// Drop all measurement state. Called when a script thread exits so that no
/// state outlives the pipeline that produced it.
pub(crate) fn clear() {
    with_frames(|frames| frames.clear());
}

/// Drop frames whose pipeline no longer has a document on this script thread
/// (for example after a pipeline exited mid-frame).
pub(crate) fn retain_pipelines(pipeline_ids: &[PipelineId]) {
    with_frames(|frames| {
        frames.retain(|pipeline_id, _| pipeline_ids.contains(pipeline_id));
    });
}
