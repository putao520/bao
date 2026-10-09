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
use servo_media::audio::param::{
    ParamRate, ParamTimelineMirror, ParamType, RampKind, UserAutomationEvent,
};

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
    /// (e147/e162, REQ-BRW-002) Script-side mirror of the scheduled
    /// automation timeline, for the spec's *synchronous*
    /// `NotSupportedError` faces: a scheduling call during the interval of
    /// an existing `setValueCurve` event throws before anything reaches the
    /// render thread (servo-media keeps the real timeline — this mirror
    /// only answers the throw guards). (e162) The guard/pruning RULES live
    /// single-source in `servo_media::audio::param::ParamTimelineMirror`,
    /// next to the real timeline (DUP-AUDIO-TIMELINE audit); this field is
    /// the per-param state only. Plain `f64` data — no GC payload
    /// (`#[no_trace]`).
    #[no_trace = "Plain f64 scheduling data — no GC payload to trace"]
    #[ignore_malloc_size_of = "plain f64 scheduling data"]
    timeline: RefCell<ParamTimelineMirror>,
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
            timeline: RefCell::default(),
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
            self.timeline.borrow().curve_covers(now)
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
        if self.timeline.borrow().curve_covers(*start_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetValueAtTime(*value, *start_time),
        ));
        self.timeline.borrow_mut().record_event(*start_time);
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
        if self.timeline.borrow().curve_covers(*end_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::RampToValueAtTime(RampKind::Linear, *value, *end_time),
        ));
        self.timeline.borrow_mut().record_event(*end_time);
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
        if self.timeline.borrow().curve_covers(*end_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::RampToValueAtTime(RampKind::Exponential, *value, *end_time),
        ));
        self.timeline.borrow_mut().record_event(*end_time);
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
        if self.timeline.borrow().curve_covers(*start_time) {
            return Err(Error::NotSupported(None));
        }
        self.message_node(AudioNodeMessage::SetParam(
            self.param,
            UserAutomationEvent::SetTargetAtTime(*target, *start_time, (*time_constant).into()),
        ));
        self.timeline.borrow_mut().record_event(*start_time);
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
        if self.timeline.borrow().curve_conflicts(*start_time, *end_time) {
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
        self.timeline.borrow_mut().record_curve(*start_time, *end_time);
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
        self.timeline.borrow_mut().cancel_from(*cancel_time, true);
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
        self.timeline.borrow_mut().cancel_from(*cancel_time, false);
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
