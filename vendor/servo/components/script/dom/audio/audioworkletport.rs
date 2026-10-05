/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletNode-port
//
// (Bao 段(3) wiring, user ruling 2026-10-05 "自研吧"): the node↔processor
// MessagePort pair conduit. The two `MessagePort` DOM objects live on
// different threads (the node's port on the script thread, the processor's
// port on the audio worklet thread), and upstream servo routes port traffic
// through the constellation — a path that never reaches worklet event loops
// (upstream worklets have no ports at all). The routing therefore lives in
// this face: a pair of bounded [`SpscRing`]s (the e90 media-face primitive,
// consumed per its "routing is script-side" contract) moving
// `StructuredSerializedData` payloads, plus two thread hooks:
//
//   * `to_processor` ring: produced by the script thread (the redirect hook
//     in `MessagePort::post_message_impl`), consumed by the audio pump drain
//     on the worklet thread. Pushes wake the pump through `wake_worklet`
//     (installed at pump registration as a `schedule_a_worklet_task` post),
//     so messages are dispatched on the worklet thread promptly.
//   * `to_main` ring: produced on the worklet thread (processor
//     `this.port.postMessage`), consumed on the script thread through
//     `notify_main` (a `port_message_queue` task post installed at node
//     construction), which dispatches `message` events on the node's port.
//
// Backpressure is explicit (e90 real-time discipline): each ring holds
// [`PORT_RING_CAPACITY`] payloads; an overflowing `postMessage` drops the
// message, counts it and logs a loud warning instead of growing the queue
// without bound. This is a documented deviation from the spec's unbounded
// port message queue; a processor that stops draining its port cannot strand
// the posting thread's memory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use js::context::JSContext;
use js::jsval::UndefinedValue;
use servo_constellation_traits::StructuredSerializedData;
use servo_media::audio::ring::{SpscRing, SpscRingError};

use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::structuredclone;
use crate::dom::event::messageevent::MessageEvent;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::globalscope::messageport::MessagePort;

/// Bounded payload budget of each ring direction (rounded up to a power of
/// two by `SpscRing::with_capacity`).
const PORT_RING_CAPACITY: usize = 1024;

/// Which ring a redirected [`MessagePort`] feeds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PortDirection {
    /// The node's port (script thread): payloads travel to the processor.
    ToProcessor,
    /// The processor's port (worklet thread): payloads travel to the node.
    ToMain,
}

/// One payload in flight: the sender's origin (spec `PortMessageTask`
/// fidelity) plus the structured-clone record (its `ports` side table is
/// empty by construction — the redirect hook rejects port transfers with a
/// `DataCloneError` before serialization consumes them).
pub(crate) struct PortPayload {
    pub(crate) origin: String,
    pub(crate) data: StructuredSerializedData,
}

/// The node↔processor port conduit for one `AudioWorkletNode`.
pub(crate) struct AudioWorkletPortConduit {
    /// node port → processor port (produced on the script thread).
    to_processor: SpscRing<PortPayload>,
    /// processor port → node port (produced on the worklet thread).
    to_main: SpscRing<PortPayload>,
    /// Worklet-thread wake hook, installed at pump registration (a
    /// `schedule_a_worklet_task` post). Called after every `to_processor`
    /// push so the pump drain dispatches promptly.
    wake_worklet: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Script-thread notify hook, installed at node construction (a
    /// `port_message_queue` task post). Called after every `to_main` push so
    /// `message` events fire promptly.
    notify_main: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Payloads dropped because the target ring was full (diagnostics for
    /// the documented bounded-queue deviation).
    dropped: AtomicU64,
}

impl AudioWorkletPortConduit {
    pub(crate) fn new() -> Arc<AudioWorkletPortConduit> {
        Arc::new(AudioWorkletPortConduit {
            to_processor: SpscRing::with_capacity(PORT_RING_CAPACITY),
            to_main: SpscRing::with_capacity(PORT_RING_CAPACITY),
            wake_worklet: Mutex::new(None),
            notify_main: Mutex::new(None),
            dropped: AtomicU64::new(0),
        })
    }

    /// Install the worklet wake hook (worklet thread, at pump registration).
    pub(crate) fn set_wake_worklet(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake_worklet.lock().expect("Locking the port wake hook") = Some(wake);
    }

    /// Install the main-thread notify hook (script thread, at node
    /// construction).
    pub(crate) fn set_notify_main(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *self.notify_main.lock().expect("Locking the port notify hook") = Some(notify);
    }

    /// Push one payload into `direction`'s ring. Overflow drops the payload,
    /// counts it and warns loudly (documented bounded-queue deviation); the
    /// target thread is notified on success.
    pub(crate) fn send(&self, direction: PortDirection, payload: PortPayload) {
        let ring = match direction {
            PortDirection::ToProcessor => &self.to_processor,
            PortDirection::ToMain => &self.to_main,
        };
        if let Err(SpscRingError::Full(_)) = ring.push(payload) {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            log::warn!(
                "AudioWorklet port ring full ({direction:?}): dropping message \
                 ({} dropped in total for this node)",
                dropped
            );
            return;
        }
        match direction {
            PortDirection::ToProcessor => {
                if let Some(wake) =
                    self.wake_worklet.lock().expect("Locking the port wake hook").as_ref()
                {
                    wake();
                }
            },
            PortDirection::ToMain => {
                if let Some(notify) =
                    self.notify_main.lock().expect("Locking the port notify hook").as_ref()
                {
                    notify();
                }
            },
        }
    }

    /// Take the oldest payload waiting for the processor (worklet thread).
    pub(crate) fn pop_for_processor(&self) -> Option<PortPayload> {
        self.to_processor.pop()
    }

    /// Take the oldest payload waiting for the node (script thread).
    pub(crate) fn pop_for_main(&self) -> Option<PortPayload> {
        self.to_main.pop()
    }
}

/// The redirect installed on a redirected `MessagePort`: where its
/// `postMessage` payloads travel. Stored on the port itself (`MessagePort`'s
/// `bao_port_redirect` field) so the lifetime is the port's own — no global
/// registry, nothing to clean up on node teardown.
#[derive(Clone)]
pub(crate) struct PortRedirect {
    pub(crate) conduit: Arc<AudioWorkletPortConduit>,
    pub(crate) direction: PortDirection,
}

/// Dispatch one drained payload as a `message` event on `port` (the tail of
/// the spec's port message queue steps — deserialize in the receiver's realm,
/// fire, and turn a failed deserialization into a `messageerror` event).
/// Mirrors `GlobalScope::route_task_to_port`'s dispatch tail.
pub(crate) fn dispatch_port_payload(
    cx: &mut JSContext,
    port: &MessagePort,
    global: &GlobalScope,
    payload: PortPayload,
) {
    let origin = payload.origin;
    rooted!(&in(cx) let mut message_clone = UndefinedValue());
    let ports = match structuredclone::read(cx, global, payload.data, message_clone.handle_mut())
    {
        Ok(ports) => {
            ports
        },
        Err(_) => {
            MessageEvent::dispatch_error(cx, port.upcast::<EventTarget>(), global);
            return;
        },
    };
    MessageEvent::dispatch_jsval(
        cx,
        port.upcast::<EventTarget>(),
        global,
        message_clone.handle(),
        Some(origin.as_str()),
        None,
        ports,
    );
}
