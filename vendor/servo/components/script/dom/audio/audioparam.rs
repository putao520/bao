/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::{Cell, RefCell};
use std::sync::mpsc;

use dom_struct::dom_struct;
use js::context::JSContext;
use script_bindings::cformat;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use servo_media::audio::audio_node::{AudioNodeMessage, AudioNodeType};
use servo_media::audio::graph::NodeId;
use servo_media::audio::param::{ParamRate, ParamType, RampKind, UserAutomationEvent};

use crate::conversions::Convert;
use crate::dom::audio::baseaudiocontext::BaseAudioContext;
use crate::dom::bindings::codegen::Bindings::AudioParamBinding::{
    AudioParamMethods, AutomationRate,
};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::root::{Dom, DomRoot};
use crate::dom::globalscope::GlobalScope;

#[dom_struct]
pub(crate) struct AudioParam {
    reflector_: Reflector,
    context: Dom<BaseAudioContext>,
    #[no_trace]
    node: Option<NodeId>,
    #[no_trace]
    node_type: AudioNodeType,
    #[no_trace]
    param: ParamType,
    automation_rate: Cell<AutomationRate>,
    default_value: f32,
    min_value: f32,
    max_value: f32,
    /// (e147, REQ-BRW-002) Script-side mirror of the scheduled automation
    /// timeline, for the spec's *synchronous* `NotSupportedError` faces: a
    /// scheduling call during the interval of an existing `setValueCurve`
    /// event throws before anything reaches the render thread (servo-media
    /// keeps the real timeline — this mirror only answers the throw guards).
    /// Curves are `(start, start + duration)` intervals; every other event
    /// contributes its scheduling time as a point. Plain `f64` data — no GC
    /// payload (`#[no_trace]`).
    #[no_trace = "Plain f64 scheduling data — no GC payload to trace"]
    #[ignore_malloc_size_of = "plain f64 scheduling data"]
    timeline_curves: RefCell<Vec<(f64, f64)>>,
    #[no_trace = "Plain f64 scheduling data — no GC payload to trace"]
    #[ignore_malloc_size_of = "plain f64 scheduling data"]
    timeline_events: RefCell<Vec<f64>>,
}

impl AudioParam {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn new_inherited(
        context: &BaseAudioContext,
        node: Option<NodeId>,
        node_type: AudioNodeType,
        param: ParamType,
        automation_rate: AutomationRate,
        default_value: f32,
        min_value: f32,
        max_value: f32,
    ) -> AudioParam {
        AudioParam {
            reflector_: Reflector::new(),
            context: Dom::from_ref(context),
            node,
            node_type,
            param,
            automation_rate: Cell::new(automation_rate),
            default_value,
            min_value,
            max_value,
            timeline_curves: RefCell::default(),
            timeline_events: RefCell::default(),
        }
    }

    #[expect(clippy::too_many_arguments)]
    #[cfg_attr(crown, expect(crown::unrooted_must_root))]
    pub(crate) fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        context: &BaseAudioContext,
        node: Option<NodeId>,
        node_type: AudioNodeType,
        param: ParamType,
        automation_rate: AutomationRate,
        default_value: f32,
        min_value: f32,
        max_value: f32,
    ) -> DomRoot<AudioParam> {
        let audio_param = AudioParam::new_inherited(
            context,
            node,
            node_type,
            param,
            automation_rate,
            default_value,
            min_value,
            max_value,
        );
        // Update the value range
        audio_param.message_node(AudioNodeMessage::SetParamRange(
            audio_param.param,
            (min_value, max_value),
        ));
        reflect_dom_object(cx, Box::new(audio_param), global)
    }

    fn message_node(&self, message: AudioNodeMessage) {
        if let Some(node_id) = self.node {
            self.context
                .audio_context_impl()
                .lock()
                .unwrap()
                .message_node(node_id, message);
        }
    }

    pub(crate) fn context(&self) -> &BaseAudioContext {
        &self.context
    }

    pub(crate) fn node_id(&self) -> Option<NodeId> {
        self.node
    }

    pub(crate) fn param_type(&self) -> ParamType {
        self.param
    }

    /// (e147) Spec `NotSupportedError` guard for scheduling a point event:
    /// throws when `time` falls inside the interval of a scheduled
    /// `setValueCurve` event (interval `[start, start + duration)`: the
    /// start is inclusive — the value setter during a curve starting "now"
    /// throws — the end is exclusive — an event at the curve's end is the
    /// next event and is fine).
    fn timeline_curve_covers(&self, time: f64) -> bool {
        self.timeline_curves
            .borrow()
            .iter()
            .any(|&(start, end)| start <= time && time < end)
    }

    /// (e147) Record a scheduled point event (setValue/ramp end/setTarget
    /// start) on the mirror.
    fn timeline_record_event(&self, time: f64) {
        self.timeline_events.borrow_mut().push(time);
    }

    /// (e147) Spec `NotSupportedError` guard for scheduling a
    /// `setValueCurve` event: throws when the new curve's interval overlaps
    /// any scheduled event (point strictly inside — a curve may start at an
    /// event's time) or any scheduled curve (positive-length intersection —
    /// back-to-back curves at a shared endpoint are fine).
    fn timeline_curve_conflicts(&self, start: f64, duration: f64) -> bool {
        let end = start + duration;
        self.timeline_events
            .borrow()
            .iter()
            .any(|&time| start < time && time < end) ||
            self.timeline_curves
                .borrow()
                .iter()
                .any(|&(other_start, other_end)| start < other_end && other_start < end)
    }

    /// (e147) Record a scheduled `setValueCurve` interval on the mirror.
    fn timeline_record_curve(&self, start: f64, duration: f64) {
        self.timeline_curves.borrow_mut().push((start, start + duration));
    }

    /// (e147) `cancelScheduledValues`/`cancelAndHoldAtTime` prune the mirror
    /// the way the spec prunes the timeline: every event whose scheduling
    /// time is past the cancel point is removed (`>=` for
    /// `cancelScheduledValues`, `>` for the hold variant), so re-scheduling
    /// after a cancel is not blocked by removed events. A curve is dropped
    /// unless it ends at or before the cancel point — cancelling *inside*
    /// a curve removes the whole curve event (the WPT
    /// `cancel-scheduled-values` "cancel setValueCurve" face: scheduling
    /// inside the cancelled curve's interval must not throw).
    fn timeline_cancel_from(&self, cancel_time: f64, inclusive: bool) {
        self.timeline_events
            .borrow_mut()
            .retain(|&time| if inclusive { time < cancel_time } else { time <= cancel_time });
        self.timeline_curves
            .borrow_mut()
            .retain(|&(_, end)| end <= cancel_time);
    }
}

impl AudioParamMethods<crate::DomTypeHolder> for AudioParam {
    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-automationrate>
    fn AutomationRate(&self) -> AutomationRate {
        self.automation_rate.get()
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-automationrate>
    fn SetAutomationRate(&self, automation_rate: AutomationRate) -> Fallible<()> {
        // > AudioBufferSourceNode
        // > The AudioParams playbackRate and detune MUST be "k-rate". An InvalidStateError must be
        // > thrown if the rate is changed to "a-rate".
        if automation_rate == AutomationRate::A_rate &&
            self.node_type == AudioNodeType::AudioBufferSourceNode &&
            (self.param == ParamType::Detune || self.param == ParamType::PlaybackRate)
        {
            return Err(Error::InvalidState(None));
        }

        self.automation_rate.set(automation_rate);
        self.message_node(AudioNodeMessage::SetParamRate(
            self.param,
            automation_rate.convert(),
        ));

        Ok(())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-value>
    fn Value(&self) -> Finite<f32> {
        if self.node.is_none() {
            return Finite::wrap(self.default_value);
        }
        let (tx, rx) = mpsc::channel();
        self.message_node(AudioNodeMessage::GetParamValue(self.param, tx));
        Finite::wrap(rx.recv().unwrap())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-value>
    fn SetValue(&self, value: Finite<f32>) -> Fallible<()> {
        // (e147) The value setter is a `setValueAtTime` at "now": during a
        // scheduled curve it throws instead of changing the value. The
        // "now" round trip degrades to "no guard" when the render thread is
        // gone (closed context) — the message below is a no-op then anyway.
        let now = self
            .context
            .audio_context_impl()
            .lock()
            .unwrap()
            .current_time_or_default();
        if let Some(now) = now &&
            self.timeline_curve_covers(now)
        {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetValue(*value),
        ));
        Ok(())
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-defaultvalue>
    fn DefaultValue(&self) -> Finite<f32> {
        Finite::wrap(self.default_value)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-minvalue>
    fn MinValue(&self) -> Finite<f32> {
        Finite::wrap(self.min_value)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-maxvalue>
    fn MaxValue(&self) -> Finite<f32> {
        Finite::wrap(self.max_value)
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-setvalueattime>
    fn SetValueAtTime(
        &self,
        value: Finite<f32>,
        start_time: Finite<f64>,
    ) -> Fallible<DomRoot<AudioParam>> {
        if *start_time < 0. {
            return Err(Error::Range(cformat!(
                "start time {} should not be negative",
                *start_time
            )));
        }
        if self.timeline_curve_covers(*start_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetValueAtTime(*value, *start_time),
        ));
        self.timeline_record_event(*start_time);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-linearramptovalueattime>
    fn LinearRampToValueAtTime(
        &self,
        value: Finite<f32>,
        end_time: Finite<f64>,
    ) -> Fallible<DomRoot<AudioParam>> {
        if *end_time < 0. {
            return Err(Error::Range(cformat!(
                "end time {} should not be negative",
                *end_time
            )));
        }
        if self.timeline_curve_covers(*end_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::RampToValueAtTime(RampKind::Linear, *value, *end_time),
        ));
        self.timeline_record_event(*end_time);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-exponentialramptovalueattime>
    fn ExponentialRampToValueAtTime(
        &self,
        value: Finite<f32>,
        end_time: Finite<f64>,
    ) -> Fallible<DomRoot<AudioParam>> {
        if *end_time < 0. {
            return Err(Error::Range(cformat!(
                "end time {} should not be negative",
                *end_time
            )));
        }
        if *value == 0. {
            return Err(Error::Range(cformat!(
                "target value {} should not be 0",
                *value
            )));
        }
        if self.timeline_curve_covers(*end_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::RampToValueAtTime(RampKind::Exponential, *value, *end_time),
        ));
        self.timeline_record_event(*end_time);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-settargetattime>
    fn SetTargetAtTime(
        &self,
        target: Finite<f32>,
        start_time: Finite<f64>,
        time_constant: Finite<f32>,
    ) -> Fallible<DomRoot<AudioParam>> {
        if *start_time < 0. {
            return Err(Error::Range(cformat!(
                "start time {} should not be negative",
                *start_time
            )));
        }
        if *time_constant < 0. {
            return Err(Error::Range(cformat!(
                "time constant {} should not be negative",
                *time_constant
            )));
        }
        if self.timeline_curve_covers(*start_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetTargetAtTime(*target, *start_time, (*time_constant).into()),
        ));
        self.timeline_record_event(*start_time);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-setvaluecurveattime>
    fn SetValueCurveAtTime(
        &self,
        values: Vec<Finite<f32>>,
        start_time: Finite<f64>,
        end_time: Finite<f64>,
    ) -> Fallible<DomRoot<AudioParam>> {
        if *start_time < 0. {
            return Err(Error::Range(cformat!(
                "start time {} should not be negative",
                *start_time
            )));
        }
        if values.len() < 2. as usize {
            return Err(Error::InvalidState(None));
        }

        if *end_time < 0. {
            return Err(Error::Range(cformat!(
                "end time {} should not be negative",
                *end_time
            )));
        }
        if self.timeline_curve_conflicts(*start_time, *end_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetValueCurveAtTime(
                values.into_iter().map(|v| *v).collect(),
                *start_time,
                *end_time,
            ),
        ));
        self.timeline_record_curve(*start_time, *end_time);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-cancelscheduledvalues>
    fn CancelScheduledValues(&self, cancel_time: Finite<f64>) -> Fallible<DomRoot<AudioParam>> {
        if *cancel_time < 0. {
            return Err(Error::Range(cformat!(
                "cancel time {} should not be negative",
                *cancel_time
            )));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::CancelScheduledValues(*cancel_time),
        ));
        self.timeline_cancel_from(*cancel_time, true);
        Ok(DomRoot::from_ref(self))
    }

    /// <https://webaudio.github.io/web-audio-api/#dom-audioparam-cancelandholdattime>
    fn CancelAndHoldAtTime(&self, cancel_time: Finite<f64>) -> Fallible<DomRoot<AudioParam>> {
        if *cancel_time < 0. {
            return Err(Error::Range(cformat!(
                "cancel time {} should not be negative",
                *cancel_time
            )));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::CancelAndHoldAtTime(*cancel_time),
        ));
        self.timeline_cancel_from(*cancel_time, false);
        Ok(DomRoot::from_ref(self))
    }
}

// https://webaudio.github.io/web-audio-api/#enumdef-automationrate
impl Convert<ParamRate> for AutomationRate {
    fn convert(self) -> ParamRate {
        match self {
            AutomationRate::A_rate => ParamRate::ARate,
            AutomationRate::K_rate => ParamRate::KRate,
        }
    }
}
