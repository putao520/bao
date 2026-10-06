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
//
// (Bao e114) The conduit is a lane bundle: lane 0 is the node's own pair
// above; each `MessagePort` substituted inside `processorOptions` gets its
// own lane (main-side endpoint = the page's port object with a redirect,
// worklet-side endpoint = a freshly minted port wired at processor
// instantiation). This is the conduit form's expression of the Transferable
// subface: the constellation port-router path cannot deliver to worklet
// event loops (the documented limitation this whole module exists around),
// so a `processorOptions` port's traffic rides the same in-process rings.

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

/// Which ring a redirected [`MessagePort`] feeds. Lane `0` is the node's own
/// port pair; lanes `1..` are the e114 `processorOptions` port lanes (one per
/// `MessagePort` substituted inside `processorOptions` at node construction —
/// the main-side endpoint is the page's port object, the worklet-side
/// endpoint is minted at processor instantiation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PortDirection {
    /// Script-thread producer: payloads travel to the consumer on the
    /// worklet thread.
    ToProcessor { lane: u32 },
    /// Worklet-thread producer: payloads travel to the consumer on the
    /// script thread.
    ToMain { lane: u32 },
}

/// One payload in flight: the sender's origin (spec `PortMessageTask`
/// fidelity) plus the structured-clone record (its `ports` side table is
/// empty by construction — the redirect hook rejects port transfers with a
/// `DataCloneError` before serialization consumes them).
pub(crate) struct PortPayload {
    pub(crate) origin: String,
    pub(crate) data: StructuredSerializedData,
}

/// Both rings of one lane.
struct PortLanePair {
    to_processor: SpscRing<PortPayload>,
    to_main: SpscRing<PortPayload>,
}

/// The node↔processor port conduit for one `AudioWorkletNode`.
///
/// (Bao e114) The conduit is a lane bundle: lane 0 is the node's own
/// port↔processor-port pair, and each `MessagePort` found inside
/// `processorOptions` at construction gets its own lane. All lanes are
/// created up front (before the `Arc` is shared across threads) and never
/// added or removed afterwards, so ring access is lock-free indexing.
pub(crate) struct AudioWorkletPortConduit {
    /// One entry per lane; index == the lane number in [`PortDirection`].
    lanes: Vec<PortLanePair>,
    /// Worklet-thread wake hook, installed at pump registration (a
    /// `schedule_a_worklet_task` post). Called after every `to_processor`
    /// push (any lane) so the pump drain dispatches promptly.
    wake_worklet: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Script-thread notify hook, installed at node construction (a
    /// `port_message_queue` task post). Called after every `to_main` push
    /// (any lane) so `message` events fire promptly.
    notify_main: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Payloads dropped because the target ring was full (diagnostics for
    /// the documented bounded-queue deviation).
    dropped: AtomicU64,
}

impl AudioWorkletPortConduit {
    /// A conduit with `count` lanes total (lane 0 = the node's own port
    /// pair, lanes `1..count` = the `processorOptions` port lanes).
    pub(crate) fn with_lane_count(count: u32) -> Arc<AudioWorkletPortConduit> {
        let make_lane = || PortLanePair {
            to_processor: SpscRing::with_capacity(PORT_RING_CAPACITY),
            to_main: SpscRing::with_capacity(PORT_RING_CAPACITY),
        };
        Arc::new(AudioWorkletPortConduit {
            lanes: (0..count.max(1)).map(|_| make_lane()).collect(),
            wake_worklet: Mutex::new(None),
            notify_main: Mutex::new(None),
            dropped: AtomicU64::new(0),
        })
    }

    /// The number of lanes this conduit was created with.
    pub(crate) fn lane_count(&self) -> u32 {
        self.lanes.len() as u32
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

    /// Push one payload into `direction`'s lane ring. Overflow drops the
    /// payload, counts it and warns loudly (documented bounded-queue
    /// deviation); the target thread is notified on success.
    pub(crate) fn send(&self, direction: PortDirection, payload: PortPayload) {
        let (ring, to_worklet) = match direction {
            PortDirection::ToProcessor { lane } => {
                let Some(pair) = self.lanes.get(lane as usize) else {
                    log::warn!("AudioWorklet port send on unknown lane {lane}");
                    return;
                };
                (&pair.to_processor, true)
            },
            PortDirection::ToMain { lane } => {
                let Some(pair) = self.lanes.get(lane as usize) else {
                    log::warn!("AudioWorklet port send on unknown lane {lane}");
                    return;
                };
                (&pair.to_main, false)
            },
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
        if to_worklet {
            if let Some(wake) =
                self.wake_worklet.lock().expect("Locking the port wake hook").as_ref()
            {
                wake();
            }
        } else if let Some(notify) = self
            .notify_main
            .lock()
            .expect("Locking the port notify hook")
            .as_ref()
        {
            notify();
        }
    }

    /// Take the oldest payload waiting for the processor (worklet thread).
    pub(crate) fn pop_for_processor(&self) -> Option<PortPayload> {
        self.pop_for_processor_lane(0)
    }

    /// Take the oldest payload waiting for the node (script thread).
    pub(crate) fn pop_for_main(&self) -> Option<PortPayload> {
        self.pop_for_main_lane(0)
    }

    /// Take the oldest payload of `lane` waiting for its worklet-thread
    /// consumer (the processor's own port for lane 0, the minted
    /// `processorOptions` counterpart port for lanes `1..`).
    pub(crate) fn pop_for_processor_lane(&self, lane: u32) -> Option<PortPayload> {
        self.lanes.get(lane as usize)?.to_processor.pop()
    }

    /// Take the oldest payload of `lane` waiting for its script-thread
    /// consumer (the node's own port for lane 0, the page's
    /// `processorOptions` port for lanes `1..`).
    pub(crate) fn pop_for_main_lane(&self, lane: u32) -> Option<PortPayload> {
        self.lanes.get(lane as usize)?.to_main.pop()
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
