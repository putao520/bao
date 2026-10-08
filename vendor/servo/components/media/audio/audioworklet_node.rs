/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! AudioWorkletNode media face: the real-time bridge between the audio render
//! thread and the AudioWorklet (JS) thread.
//!
//! # Threading model
//!
//! Servo's audio graph renders one render quantum (128 frames) per block on
//! the audio render thread ([`AudioRenderThread`]). SpiderMonkey is *never*
//! driven from that thread: a GC pause on the render thread is audible as a
//! glitch, so this face keeps the two worlds apart with a bounded handoff:
//!
//! ```text
//! render thread (this module's AudioWorkletNode)          worklet thread (script face)
//! ───────────────────────────────────────────             ─────────────────────────────
//! 1. fill a pooled quantum (inputs + param values)  ──▶  2. pending ring ──▶ handler
//!                                                            runs the JS process() and
//!                                                            fills quantum.outputs
//! 4. ready ring ◀── 3. push the completed quantum  ◀──
//!    (underrun ⇒ silence output, count it)
//! ```
//!
//! Every arrow is a [`SpscRing`] operation: bounded, wait-free, allocation
//! free. The quantum frames themselves are preallocated at node creation and
//! shuttle between the two rings plus a render-thread-local pool, so the
//! render thread's block path performs no allocation on the steady state. The
//! only per-quantum allocation left is the destination `Block` storage the
//! graph gives every node (same as all existing nodes: an empty `Block`
//! becomes a real buffer on first write).
//!
//! # Faces
//!
//! * *Node face* — [`AudioWorkletNode`] is a regular graph node: it consumes
//!   its input ports and produces its output ports, with underruns rendered
//!   as silence (spec behaviour when a processor produces no output).
//! * *Param face* — one [`Param`] per declared parameter, keyed by
//!   [`ParamType::WorkletParam(index)`] so the existing automation timeline
//!   (`SetValueAtTime`, ramps, connect() into a parameter port, k-rate/a-rate
//!   switching) is reused wholesale. The DOM-side `AudioParamMap` name→index
//!   mapping belongs to the script face.
//! * *Port face* — [`PortMessageRing`] is the bounded conduit primitive for
//!   main-thread ↔ processor `MessagePort` payloads; the routing and the
//!   structured-clone encoding belong to the script face.
//! * *onprocessorerror face* — a processor exception (raised on the worklet
//!   thread) is signalled through [`AudioWorkletBridge::signal_processor_error`];
//!   the node outputs silence from the next block on, and the script face
//!   observes the sticky flag with [`AudioWorkletBridge::processor_failed`]
//!   to fire the DOM event once.
//!
//! [`AudioRenderThread`]: crate::render_thread::AudioRenderThread

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use malloc_size_of_derive::MallocSizeOf;

use crate::audio_node::{AudioNodeEngine, AudioNodeType, BlockInfo, ChannelInfo};
use crate::block::{Chunk, FRAMES_PER_BLOCK_USIZE, Tick};
use crate::param::{Param, ParamRate, ParamType};
use crate::ring::{SpscRing, SpscRingError};

/// Number of quantum frames that can be in flight between the render thread
/// and the worklet thread.
///
/// 8 quanta ≈ 21 ms at 48 kHz: enough to absorb scheduling jitter of the
/// worklet (JS) thread without letting a stalled worklet thread strand an
/// unbounded amount of render-thread work. Each side degrades honestly when
/// the ring is full: the render side first paces itself against the pump
/// with bounded cooperative yields (see [`SATURATION_PACE_YIELDS`]) and,
/// once that cap is spent, drops the freshest input quantum (the worklet
/// keeps the last good state instead of a stale backlog); the worklet side
/// stages its output until the render side catches up.
pub const DEFAULT_BRIDGE_CAPACITY: usize = 8;

/// Spec upper bound for a node port's channel count.
const MAX_PORT_CHANNELS: u8 = 32;

/// How many cooperative yields the render thread spends trying to hand a
/// saturated pending ring to the worklet thread before falling back to
/// dropping the freshest input quantum (see `AudioWorkletNode::process`).
///
/// Each `yield_now()` cedes the CPU once: on a machine where the render
/// thread would otherwise sprint past the pump (the offline fast-forward
/// render produces blocks 10-20x faster than the JS `process()` call
/// consumes them), the first yield is already a full handoff — the worklet
/// thread drains its pending ring to idle and frees many slots. The cap is
/// a wedge guard for the window where no drainer exists yet (processor
/// instantiation) or the worklet thread died: those yields are near-free
/// (nothing else runnable ⇒ the yield returns immediately), and once the
/// cap is spent the honest drop degradation stands.
const SATURATION_PACE_YIELDS: usize = 64;

/// Validation error for [`AudioWorkletNodeOptions`].
///
/// The script face maps these to `NotSupportedError` / `RangeError` DOM
/// exceptions when constructing an `AudioWorkletNode`.
#[derive(Debug, PartialEq)]
pub enum AudioWorkletNodeError {
    /// `output_channel_count` was given and its length does not match
    /// `number_of_outputs` (spec: NotSupportedError).
    OutputChannelCountMismatch,
    /// A port channel count is zero or above [`MAX_PORT_CHANNELS`]
    /// (spec: RangeError).
    InvalidChannelCount(u8),
}

impl fmt::Display for AudioWorkletNodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioWorkletNodeError::OutputChannelCountMismatch => write!(
                f,
                "output_channel_count length must match number_of_outputs"
            ),
            AudioWorkletNodeError::InvalidChannelCount(c) => {
                write!(f, "channel count {c} outside the range 1..={MAX_PORT_CHANNELS}")
            },
        }
    }
}

impl std::error::Error for AudioWorkletNodeError {}

/// Initial value and automation rate for one worklet AudioParam.
///
/// The parameter *name* lives in the script face's `AudioParamMap`; the media
/// face addresses parameters by index ([`ParamType::WorkletParam`]). Min/max
/// clamping is the script face's responsibility, matching how built-in node
/// options carry already-validated defaults.
#[derive(Clone, Copy, Debug, MallocSizeOf)]
pub struct WorkletParamInit {
    pub default_value: f32,
    pub rate: ParamRate,
}

/// Shape of an `AudioWorkletNode`, minus the bridge.
///
/// The script face fills this from the `AudioWorkletNodeOptions` dictionary;
/// `output_channel_count` entries fall back to `input_channel_count` when the
/// vector is empty (the spec computes the same default).
#[derive(Clone, Debug, Default, MallocSizeOf)]
pub struct AudioWorkletNodeOptions {
    pub number_of_inputs: u32,
    pub number_of_outputs: u32,
    /// Per-output-port channel counts; empty means "use `input_channel_count`
    /// for every port".
    pub output_channel_count: Vec<u8>,
    /// Channel count of every input-port buffer shipped to the processor.
    pub input_channel_count: u8,
    /// Declared parameters, in `AudioParamMap` declaration order.
    pub params: Vec<WorkletParamInit>,
}

impl AudioWorkletNodeOptions {
    /// Validate the port/channel shape.
    pub fn validate(&self) -> Result<(), AudioWorkletNodeError> {
        if !self.output_channel_count.is_empty() &&
            self.output_channel_count.len() != self.number_of_outputs as usize
        {
            return Err(AudioWorkletNodeError::OutputChannelCountMismatch);
        }
        if self.input_channel_count == 0 || self.input_channel_count > MAX_PORT_CHANNELS {
            return Err(AudioWorkletNodeError::InvalidChannelCount(
                self.input_channel_count,
            ));
        }
        for count in self
            .output_channel_count
            .iter()
            .copied()
            .chain(std::iter::repeat(self.input_channel_count))
            .take(self.number_of_outputs as usize)
        {
            if count == 0 || count > MAX_PORT_CHANNELS {
                return Err(AudioWorkletNodeError::InvalidChannelCount(count));
            }
        }
        Ok(())
    }

    fn output_channels(&self, port: u32) -> u8 {
        self.output_channel_count
            .get(port as usize)
            .copied()
            .unwrap_or(self.input_channel_count)
    }

    /// Build the render↔worklet bridge with [`DEFAULT_BRIDGE_CAPACITY`].
    pub fn make_bridge(&self) -> AudioWorkletBridge {
        self.make_bridge_with_capacity(DEFAULT_BRIDGE_CAPACITY)
    }

    /// Build the render↔worklet bridge with an explicit in-flight quantum
    /// budget (rounded up to a power of two).
    pub fn make_bridge_with_capacity(&self, capacity: usize) -> AudioWorkletBridge {
        AudioWorkletBridge::new(
            QuantumShape {
                inputs: self.number_of_inputs,
                input_channels: self.input_channel_count,
                output_channels: (0..self.number_of_outputs)
                    .map(|port| self.output_channels(port))
                    .collect(),
                output_computed: (0..self.number_of_outputs)
                    .map(|port| self.output_channel_count.get(port as usize).is_none())
                    .collect(),
                params: self.params.len(),
            },
            capacity,
        )
    }
}

/// Everything [`crate::render_thread`] needs to construct the node: the shape
/// plus the bridge half that the worklet thread will hold.
#[derive(Debug, MallocSizeOf)]
pub struct AudioWorkletNodeInit {
    #[ignore_malloc_size_of = "measured through options"]
    pub options: AudioWorkletNodeOptions,
    #[ignore_malloc_size_of = "lock-free bridge, no heap-owned payload"]
    pub bridge: Arc<AudioWorkletBridge>,
}

/// Channel-plane sample buffer of one port (or one parameter) inside a
/// [`WorkletQuantum`].
///
/// Layout matches servo-media's `Block`: channel-major, 128 frames per
/// channel. Preallocated once per pooled frame; writes never reallocate.
#[derive(Debug)]
pub struct WorkletBuffer {
    data: Vec<f32>,
    channels: u8,
}

impl WorkletBuffer {
    fn new(channels: u8) -> Self {
        let channels = channels.max(1);
        Self {
            data: vec![0.; FRAMES_PER_BLOCK_USIZE * channels as usize],
            channels,
        }
    }

    /// Channel count of this buffer.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// The whole channel-plane buffer, `channels * 128` samples.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// The whole channel-plane buffer, `channels * 128` samples.
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// One channel's 128 frames.
    pub fn chan(&self, channel: u8) -> &[f32] {
        let start = channel as usize * FRAMES_PER_BLOCK_USIZE;
        &self.data[start..start + FRAMES_PER_BLOCK_USIZE]
    }

    /// One channel's 128 frames.
    pub fn chan_mut(&mut self, channel: u8) -> &mut [f32] {
        let start = channel as usize * FRAMES_PER_BLOCK_USIZE;
        &mut self.data[start..start + FRAMES_PER_BLOCK_USIZE]
    }

    fn zero(&mut self) {
        self.data.fill(0.);
    }
}

/// One render quantum exchanged between the render thread and the worklet
/// thread.
///
/// The same frame object shuttles render → worklet (with `inputs` and
/// `params` freshly written) → render (with `outputs` written) → back into
/// the render thread's pool, so no frame is allocated on the block path.
///
/// `inputs` and `params` are read-only from the processor's point of view;
/// the handler fills `outputs`.
#[derive(Debug)]
pub struct WorkletQuantum {
    /// Context sample rate for this quantum.
    pub sample_rate: f32,
    /// Absolute tick of the quantum's first frame.
    pub frame: u64,
    /// Context time (seconds) at the quantum start.
    pub time: f64,
    /// One buffer per input port.
    pub inputs: Box<[WorkletBuffer]>,
    /// One buffer per output port; the handler writes the processor output
    /// here.
    pub outputs: Box<[WorkletBuffer]>,
    /// One 128-value buffer per declared parameter. k-rate parameters carry
    /// the same value in every slot; a-rate parameters carry the per-frame
    /// timeline values.
    pub params: Box<[WorkletBuffer]>,
    /// (e147) Per-input-port live channel count for this quantum: the
    /// channel count of the bus actually connected to the port, `0` when
    /// the port has no connection this block. The spec's `process()`
    /// argument shape (`inputs[p]` is empty for an unconnected port) is
    /// derived from this on the script side; the buffers above stay laid
    /// out at the declared channel count.
    pub input_live: Box<[u8]>,
    /// (e147) Per-output-port live channel count for this quantum: the
    /// explicit `outputChannelCount` entry where given, else the spec's
    /// `computedNumberOfChannels` (max of the connected inputs' channels, 1
    /// with no connections), capped at the buffer layout. Same script-side
    /// consumption as `input_live`.
    pub output_live: Box<[u8]>,
}

/// Buffer layout shared by a bridge and its pooled quanta.
#[derive(Clone, Debug)]
pub(crate) struct QuantumShape {
    inputs: u32,
    input_channels: u8,
    output_channels: Vec<u8>,
    /// (e147) Per-output-port "computed" marker: `true` when the port has no
    /// explicit `outputChannelCount` entry and its live channel count is the
    /// spec's `computedNumberOfChannels` (max of the connected inputs'
    /// channels, 1 with no connections) — recomputed every quantum. The
    /// `output_channels` layout of such a port is the capacity
    /// (`input_channel_count`), the live count travels on the quantum.
    output_computed: Vec<bool>,
    params: usize,
}

/// The bounded, allocation-free handoff between the render thread and the
/// worklet thread.
///
/// Held as `Arc<AudioWorkletBridge>` by both the render-side
/// [`AudioWorkletNode`] (inside the audio graph) and the worklet-side
/// [`AudioWorkletPump`] (inside the script face).
pub struct AudioWorkletBridge {
    shape: QuantumShape,
    /// render → worklet: quanta with fresh inputs and parameter values.
    pending: SpscRing<WorkletQuantum>,
    /// worklet → render: quanta with processor output.
    ready: SpscRing<WorkletQuantum>,
    /// The processor threw (`onprocessorerror`); outputs silence from now on.
    failed: AtomicBool,
    /// The processor's `process()` returned false; outputs silence from now
    /// on (spec: "no more output will be produced").
    finished: AtomicBool,
    /// The render side found no completed quantum and output silence.
    underruns: AtomicU64,
    /// The render side had no pooled frame left to ship inputs with.
    frame_starved: AtomicU64,
    /// Worklet-thread wake hook, installed by the script face when the pump is
    /// registered (BAO 段(3) wiring). The render side never blocks and never
    /// calls into SpiderMonkey; it only notifies through this closure, which
    /// the script face binds to its own event loop (a worklet task post —
    /// `WorkletExecutor::schedule_a_worklet_task`), so `process()` runs on the
    /// worklet thread within micro of the render side publishing inputs.
    /// Without it the pump would only be driven by external worklet activity;
    /// with it, offline (faster-than-real-time) renders stay fed and real-time
    /// latency stays at one quantum.
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl fmt::Debug for AudioWorkletBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioWorkletBridge")
            .field("shape", &self.shape)
            .field("capacity", &self.pending.capacity())
            .field("pending", &self.pending.len())
            .field("ready", &self.ready.len())
            .field("failed", &self.failed.load(Ordering::Relaxed))
            .field("finished", &self.finished.load(Ordering::Relaxed))
            .field("underruns", &self.underruns.load(Ordering::Relaxed))
            .field("frame_starved", &self.frame_starved.load(Ordering::Relaxed))
            .finish()
    }
}

impl AudioWorkletBridge {
    fn new(shape: QuantumShape, capacity: usize) -> Self {
        Self {
            shape,
            pending: SpscRing::with_capacity(capacity),
            ready: SpscRing::with_capacity(capacity),
            failed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            frame_starved: AtomicU64::new(0),
            wake: Mutex::new(None),
        }
    }

    /// The buffer layout of every quantum this bridge exchanges.
    pub(crate) fn shape(&self) -> &QuantumShape {
        &self.shape
    }

    /// In-flight quantum budget of each direction.
    pub fn capacity(&self) -> usize {
        self.pending.capacity()
    }

    // --- render thread side ---

    /// Publish a quantum (fresh inputs + parameter values) to the worklet
    /// thread. Wait-free. `Err` returns the frame for pooling when the
    /// worklet thread is more than `capacity` quanta behind; the inputs of
    /// that quantum are dropped (real-time discipline: never block, never
    /// buffer unboundedly).
    pub fn push_pending(&self, quantum: WorkletQuantum) -> Result<(), SpscRingError<WorkletQuantum>> {
        let result = self.pending.push(quantum);
        if result.is_ok() {
            self.wake_worklet();
        }
        result
    }

    /// Install the worklet-side wake hook (BAO 段(3) wiring). Called once by
    /// the script face when the pump is registered; later calls overwrite the
    /// previous hook (same worklet thread in practice).
    pub fn set_wake_fn(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock().expect("Locking the worklet wake hook") = Some(wake);
    }

    /// Notify the worklet thread that a fresh quantum is available. The
    /// closure is script-face-owned; this side never drives SpiderMonkey and
    /// never blocks the render thread on the notification.
    fn wake_worklet(&self) {
        if let Some(wake) = self.wake.lock().expect("Locking the worklet wake hook").as_ref() {
            wake();
        }
    }

    /// Take the oldest completed output quantum, or `None` on underrun.
    pub fn pop_ready(&self) -> Option<WorkletQuantum> {
        self.ready.pop()
    }

    /// Number of quanta waiting for the worklet thread.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Number of completed quanta waiting for the render thread.
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// Count a render-side underrun (no completed quantum available).
    pub fn record_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a render-side pool starvation (no pooled frame left).
    pub fn record_frame_starved(&self) {
        self.frame_starved.fetch_add(1, Ordering::Relaxed);
    }

    // --- worklet thread side ---

    /// Take the oldest quantum with fresh inputs, or `None` when the render
    /// thread has not published a new quantum yet (the pending stream *is*
    /// the block-rate clock of the worklet side).
    pub fn pop_pending(&self) -> Option<WorkletQuantum> {
        self.pending.pop()
    }

    /// Publish a completed output quantum to the render thread. Wait-free.
    /// `Err` returns the frame when the render thread is more than `capacity`
    /// quanta behind; the pump stages it and retries on the next block.
    pub fn push_ready(&self, quantum: WorkletQuantum) -> Result<(), SpscRingError<WorkletQuantum>> {
        self.ready.push(quantum)
    }

    // --- processor state (onprocessorerror / process() == false) ---

    /// Mark the processor as failed (a JS exception escaped `process()`).
    /// The node outputs silence from the next block on; the script face
    /// observes the latch with [`Self::take_processor_failed`] to fire
    /// `onprocessorerror`.
    pub fn signal_processor_error(&self) {
        self.failed.store(true, Ordering::Release);
    }

    /// Mark the processor as finished (`process()` returned false).
    pub fn signal_processor_finished(&self) {
        self.finished.store(true, Ordering::Release);
    }

    /// Whether a processor exception was signalled. The flag is sticky for
    /// the node's lifetime (a failed processor stays muted); the script face
    /// fires the `processorerror` DOM event once on its first `true`
    /// observation.
    pub fn processor_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Whether the processor stopped producing output (error or `false`
    /// return), i.e. the node must render silence.
    pub fn processor_halted(&self) -> bool {
        self.failed.load(Ordering::Acquire) | self.finished.load(Ordering::Acquire)
    }

    /// Render-side underrun count (diagnostics).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Render-side pool-starvation count (diagnostics).
    pub fn frame_starved(&self) -> u64 {
        self.frame_starved.load(Ordering::Relaxed)
    }
}

/// Bounded conduit for main-thread ↔ processor `MessagePort` payloads.
///
/// `Vec<u8>` slots are the encoded (structured-clone) message bytes; the
/// routing between the DOM thread and the worklet thread belongs to the
/// script face. `push` failures are explicit backpressure
/// ([`SpscRingError::Full`]) so a flooding sender is throttled instead of
/// growing the queue without bound.
pub type PortMessageRing = SpscRing<Vec<u8>>;

/// Whether the processor wants to keep being called after this quantum.
///
/// Mirrors the `process()` boolean return of the spec: `Finish` means "no
/// more output will be produced" and mutes the node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProcessorControl {
    /// Keep calling the processor.
    Continue,
    /// The processor produced its last output.
    Finish,
}

/// Worklet-thread-side callback contract for one `AudioWorkletProcessor`
/// instance.
///
/// Implemented by the script face (the side that calls into SpiderMonkey —
/// that call site is segment 1's wiring, not the media face). The media face
/// only defines the seam and drives it at block rate through
/// [`AudioWorkletPump`].
pub trait AudioWorkletProcessorHandler: Send {
    /// Process one quantum: read `quantum.inputs` / `quantum.params`, fill
    /// `quantum.outputs`, and report whether the processor keeps running.
    fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl;
}

/// Outcome of one [`AudioWorkletPump::pump_once`] call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PumpOutcome {
    /// A quantum was processed and handed to the render thread.
    Delivered,
    /// The render thread has not consumed the previous output; the processed
    /// quantum is staged and will be delivered by a later pump call.
    Staged,
    /// No pending quantum from the render thread (it has not rendered a new
    /// block yet).
    Idle,
    /// The processor halted (error or `Finish`); no further processing.
    Halted,
}

/// Worklet-thread-side driver that ties the bridge to a processor handler.
///
/// One pump per node, owned by the worklet thread (the script face calls
/// [`pump_once`](Self::pump_once) from its block-rate pump loop).
pub struct AudioWorkletPump {
    bridge: Arc<AudioWorkletBridge>,
    handler: Box<dyn AudioWorkletProcessorHandler>,
    /// Output quantum accepted by the handler but not yet by the render side.
    staged: Option<WorkletQuantum>,
}

impl fmt::Debug for AudioWorkletPump {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioWorkletPump")
            .field("bridge", &self.bridge)
            .field("staged", &self.staged.is_some())
            .finish()
    }
}

impl AudioWorkletPump {
    pub fn new(bridge: Arc<AudioWorkletBridge>, handler: Box<dyn AudioWorkletProcessorHandler>) -> Self {
        Self {
            bridge,
            handler,
            staged: None,
        }
    }

    /// The shared bridge (also lets the script face reach the port-message
    /// conduits and the state latches).
    pub fn bridge(&self) -> &Arc<AudioWorkletBridge> {
        &self.bridge
    }

    /// Report a processor exception (`onprocessorerror`): latch the failure
    /// and drop any staged output.
    pub fn signal_processor_error(&mut self) {
        self.staged = None;
        self.bridge.signal_processor_error();
    }

    fn recycle_halted(&mut self, quantum: WorkletQuantum) {
        // Keep frames circulating after a halt: hand the frame back to the
        // render thread without touching its contents.
        let _ = self.bridge.push_ready(quantum);
    }

    /// Run one block-rate step: take the oldest pending quantum, run the
    /// handler, deliver the output. Wait-free; never blocks the render side.
    pub fn pump_once(&mut self) -> PumpOutcome {
        if self.bridge.processor_halted() {
            if let Some(staged) = self.staged.take() {
                self.recycle_halted(staged);
            }
            if let Some(quantum) = self.bridge.pop_pending() {
                self.recycle_halted(quantum);
            }
            return PumpOutcome::Halted;
        }
        // Deliver a previously staged quantum before producing new output.
        if let Some(quantum) = self.staged.take() {
            match self.bridge.push_ready(quantum) {
                Ok(()) => return PumpOutcome::Delivered,
                Err(SpscRingError::Full(quantum)) => {
                    self.staged = Some(quantum);
                    return PumpOutcome::Staged;
                },
            }
        }
        let Some(mut quantum) = self.bridge.pop_pending() else {
            return PumpOutcome::Idle;
        };
        match self.handler.process_quantum(&mut quantum) {
            ProcessorControl::Continue => match self.bridge.push_ready(quantum) {
                Ok(()) => PumpOutcome::Delivered,
                Err(SpscRingError::Full(quantum)) => {
                    self.staged = Some(quantum);
                    PumpOutcome::Staged
                },
            },
            ProcessorControl::Finish => {
                self.bridge.signal_processor_finished();
                self.recycle_halted(quantum);
                PumpOutcome::Halted
            },
        }
    }
}

/// The render-side graph node.
///
/// Created on the render thread through
/// [`AudioNodeInit::AudioWorkletNode`](crate::audio_node::AudioNodeInit::AudioWorkletNode);
/// consumes completed quanta from the bridge and publishes fresh inputs.
#[derive(AudioNodeCommon)]
pub(crate) struct AudioWorkletNode {
    channel_info: ChannelInfo,
    number_of_inputs: u32,
    number_of_outputs: u32,
    params: Vec<Param>,
    bridge: Arc<AudioWorkletBridge>,
    /// Recycled quantum frames. Render-thread-local: only this node pushes
    /// and pops it, so it is a plain `Vec`.
    pool: Vec<WorkletQuantum>,
}

impl AudioWorkletNode {
    pub(crate) fn new(init: AudioWorkletNodeInit, channel_info: ChannelInfo) -> Self {
        let AudioWorkletNodeInit { options, bridge } = init;
        debug_assert!(
            options.validate().is_ok(),
            "script face must validate AudioWorkletNodeOptions before CreateNode"
        );
        let params = options
            .params
            .iter()
            .map(|param| match param.rate {
                ParamRate::KRate => Param::new_krate(param.default_value),
                ParamRate::ARate => Param::new(param.default_value),
            })
            .collect::<Vec<_>>();
        // Frames in flight are bounded by pending + ready ring capacity, so
        // a pool of `2 * capacity` frames never starves the render path in
        // steady state (both rings full at once is the only way to strand
        // every frame, and a full pending ring rejects the frame back into
        // the pool before that can matter).
        let pool = (0..bridge.capacity() * 2)
            .map(|_| bridge.shape().make_quantum())
            .collect();
        Self {
            channel_info,
            number_of_inputs: options.number_of_inputs,
            number_of_outputs: options.number_of_outputs,
            params,
            bridge,
            pool,
        }
    }

    /// Copy the node's inputs and the parameter timelines into a pooled
    /// quantum. Allocation-free: every destination is preallocated.
    fn fill_quantum(&mut self, quantum: &mut WorkletQuantum, inputs: &Chunk, info: &BlockInfo) {
        quantum.sample_rate = info.sample_rate;
        quantum.frame = info.frame.0;
        quantum.time = info.time;
        for (port, buffer) in quantum.inputs.iter_mut().enumerate() {
            let Some(block) = inputs.blocks.get(port) else {
                quantum.input_live[port] = 0;
                buffer.zero();
                continue;
            };
            // (e147) The spec exposes the connected bus's channel count: an
            // unconnected input port keeps the graph's default placeholder
            // block (a 1-channel silent `Block::default()`), which reads as
            // "no connection" here. A connected-but-silent multi-channel bus
            // keeps its channel count. The count is capped at the buffer
            // layout (a wider bus is truncated by the copy below anyway, as
            // it has always been).
            quantum.input_live[port] = if block.is_silence() && block.chan_count() <= 1 {
                0
            } else {
                block.chan_count().min(buffer.channels())
            };
            if block.is_silence() {
                buffer.zero();
                continue;
            }
            for channel in 0..buffer.channels() {
                if channel < block.chan_count() {
                    buffer.chan_mut(channel).copy_from_slice(block.data_chan(channel));
                } else {
                    buffer.chan_mut(channel).fill(0.);
                }
            }
        }
        for (index, param) in self.params.iter_mut().enumerate() {
            let buffer = &mut quantum.params[index];
            if param.rate() == ParamRate::KRate {
                param.update(info, Tick(0));
                // Uniform fill: the spec exposes k-rate values as a
                // single-element array, the script face reads slot 0.
                buffer.data_mut().fill(param.value());
            } else {
                for (frame, slot) in buffer.data_mut().iter_mut().enumerate() {
                    param.update(info, Tick(frame as u64));
                    *slot = param.value();
                }
            }
        }

        // (e147) The output face's live channel counts: explicit
        // `outputChannelCount` ports keep their entry; computed ports get
        // the spec's `computedNumberOfChannels` — the max of the connected
        // inputs' channel counts (capped at the buffer capacity), 1 with no
        // connections.
        let computed = quantum
            .input_live
            .iter()
            .copied()
            .fold(1u8, |acc, live| acc.max(live));
        let output_computed = &self.bridge.shape().output_computed;
        for (port, buffer) in quantum.outputs.iter().enumerate() {
            quantum.output_live[port] = if output_computed.get(port) == Some(&true) {
                computed.min(buffer.channels())
            } else {
                buffer.channels()
            };
        }
    }

    /// Write a completed quantum's outputs into the chunk the graph expects.
    /// (e147) Each output block takes the port's live channel count
    /// (explicit `outputChannelCount` or the computed value), not the buffer
    /// layout.
    fn take_outputs(&mut self, quantum: &mut WorkletQuantum, outputs: &mut Chunk) {
        for (port, buffer) in quantum.outputs.iter().enumerate() {
            let Some(block) = outputs.blocks.get_mut(port) else {
                break;
            };
            let live = quantum.output_live[port];
            block.resize_silence(live);
            for channel in 0..live {
                block
                    .data_chan_mut(channel)
                    .copy_from_slice(buffer.chan(channel));
            }
        }
    }

    /// Return a frame to the render-side pool with spec-clean output
    /// buffers: each `process()` call must observe all-zero `outputs`
    /// (<https://webaudio.github.io/web-audio-api/#process-call>: the
    /// outputs are zero-filled on entry). Inputs and params are always
    /// fully rewritten by [`Self::fill_quantum`], so only the outputs need
    /// clearing; every path that hands a frame back to the pool goes
    /// through here, and freshly built pool frames start zeroed, so the
    /// handler can never observe a previous cycle's samples.
    fn pool_recycle(&mut self, mut quantum: WorkletQuantum) {
        for buffer in quantum.outputs.iter_mut() {
            buffer.zero();
        }
        self.pool.push(quantum);
    }
}

impl AudioNodeEngine for AudioWorkletNode {
    fn node_type(&self) -> AudioNodeType {
        AudioNodeType::AudioWorkletNode
    }

    fn input_count(&self) -> u32 {
        self.number_of_inputs
    }

    fn output_count(&self) -> u32 {
        self.number_of_outputs
    }

    fn always_process(&self) -> bool {
        // A live processor must be driven every quantum even when the node
        // has no destination path (unconnected, or zero outputs): spec
        // semantics call `process()` until it returns false. A halted
        // processor stays cheap in the always-process set — `process`
        // short-circuits on the bridge latch (one acquire-load, no JS).
        true
    }

    fn process(&mut self, inputs: Chunk, info: &BlockInfo) -> Chunk {
        let mut outputs = Chunk::default();
        outputs
            .blocks
            .resize(self.number_of_outputs as usize, Default::default());

        if self.bridge.processor_halted() {
            // onprocessorerror, or the processor produced its last output:
            // silence from here on. Recycle whatever is in flight so the
            // pool is conserved.
            while let Some(quantum) = self.bridge.pop_ready() {
                self.pool_recycle(quantum);
            }
            return outputs;
        }

        // Publish this block's inputs and parameter timeline to the worklet
        // thread. A saturated pending ring means the producer is outrunning
        // the pump: pace it first with bounded cooperative yields — every
        // yield hands the CPU to the worklet thread, whose drain-to-idle
        // task frees ring slots, so the ready ring gets contiguous batches
        // instead of one quantum at a time interleaved with silent
        // underruns. Only after the cap (no drainer yet — instantiation —
        // or a wedged worklet thread) does the honest degradation stand:
        // drop the freshest quantum; the processor keeps its last state
        // instead of chasing a stale backlog. The pacing lives exclusively
        // in this saturated path: the healthy steady state stays wait-free,
        // lock-free and syscall-free.
        if let Some(mut quantum) = self.pool.pop() {
            self.fill_quantum(&mut quantum, &inputs, info);
            let mut yields_left = SATURATION_PACE_YIELDS;
            loop {
                match self.bridge.push_pending(quantum) {
                    Ok(()) => break,
                    Err(SpscRingError::Full(returned)) => {
                        quantum = returned;
                        if yields_left == 0 {
                            self.pool_recycle(quantum);
                            break;
                        }
                        yields_left -= 1;
                        std::thread::yield_now();
                    },
                }
            }
        } else {
            self.bridge.record_frame_starved();
        }

        // Consume the most recent completed output quantum. Underruns (the
        // worklet thread has not produced this block's audio in time) render
        // as silence, which is what the spec requires of a processor that
        // produces no output.
        match self.bridge.pop_ready() {
            Some(mut quantum) => {
                self.take_outputs(&mut quantum, &mut outputs);
                self.pool_recycle(quantum);
            },
            None => self.bridge.record_underrun(),
        }
        outputs
    }

    fn get_param(&mut self, id: ParamType) -> &mut Param {
        match id {
            ParamType::WorkletParam(index) => self
                .params
                .get_mut(index as usize)
                .unwrap_or_else(|| panic!("Unknown worklet param index {index}")),
            _ => panic!("Unknown param {id:?} for AudioWorkletNode"),
        }
    }
}

impl QuantumShape {
    /// Allocate one quantum frame with this layout.
    fn make_quantum(&self) -> WorkletQuantum {
        WorkletQuantum {
            sample_rate: 0.,
            frame: 0,
            time: 0.,
            inputs: (0..self.inputs)
                .map(|_| WorkletBuffer::new(self.input_channels))
                .collect(),
            input_live: (0..self.inputs).map(|_| self.input_channels).collect(),
            output_live: self
                .output_channels
                .iter()
                .copied()
                .collect(),
            outputs: self
                .output_channels
                .iter()
                .copied()
                .map(WorkletBuffer::new)
                .collect(),
            params: (0..self.params)
                .map(|_| WorkletBuffer::new(1))
                .collect(),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_node::{AudioNodeInit, AudioNodeMessage, AudioScheduledSourceNodeMessage};
    use crate::block::Block;
    use crate::constant_source_node::ConstantSourceNodeOptions;
    use crate::context::AudioContextOptions;
    use crate::decoder::AudioDecoder;
    use crate::graph::{AudioGraph, InputPort, PortId};
    use crate::param::{RampKind, UserAutomationEvent};
    use crate::render_thread::{AudioRenderThread, AudioRenderThreadMsg};
    use crate::sink::{AudioSink, AudioSinkError};
    use servo_media_streams::{MediaSocket, MediaStreamId};
    use std::sync::mpsc::{self, Sender};
    use std::sync::{Arc as StdArc, Condvar, Mutex, OnceLock};
    use std::thread;
    use std::time::Duration;

    const SAMPLE_RATE: f32 = 44100.;
    const TEST_BRIDGE_CAPACITY: usize = 2;

    fn options(
        inputs: u32,
        outputs: u32,
        channels: u8,
        params: &[WorkletParamInit],
    ) -> AudioWorkletNodeOptions {
        AudioWorkletNodeOptions {
            number_of_inputs: inputs,
            number_of_outputs: outputs,
            output_channel_count: Vec::new(),
            input_channel_count: channels,
            params: params.to_vec(),
        }
    }

    fn make_node(options: AudioWorkletNodeOptions) -> AudioWorkletNode {
        let bridge = StdArc::new(options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options,
                bridge,
            },
            ChannelInfo::default(),
        )
    }

    fn filled_chunk(values: [f32; FRAMES_PER_BLOCK_USIZE], channels: u8) -> Chunk {
        let mut chunk = Chunk::default();
        chunk.blocks.resize(1, Default::default());
        chunk.blocks[0] = Block::for_channels_explicit(channels);
        for channel in 0..channels {
            chunk.blocks[0].data_chan_mut(channel).copy_from_slice(&values);
        }
        chunk
    }

    fn block_info(frame: u64) -> BlockInfo {
        BlockInfo {
            sample_rate: SAMPLE_RATE,
            frame: Tick(frame),
            time: frame as f64 / SAMPLE_RATE as f64,
        }
    }

    /// Handler that echoes input channel 0 into every output channel, or a
    /// 0.25 constant when the node has no input ports.
    struct EchoHandler;

    impl AudioWorkletProcessorHandler for EchoHandler {
        fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
            let input = quantum.inputs.first().map(|buffer| buffer.chan(0).to_vec());
            for buffer in quantum.outputs.iter_mut() {
                match &input {
                    Some(input) => buffer.chan_mut(0).copy_from_slice(input),
                    None => buffer.data_mut().fill(0.25),
                }
            }
            ProcessorControl::Continue
        }
    }

    /// Handler filling each output with an increasing value, so every
    /// delivered quantum is distinguishable by content.
    struct CountingHandler(u32);

    impl AudioWorkletProcessorHandler for CountingHandler {
        fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
            let value = self.0 as f32;
            self.0 += 1;
            for buffer in quantum.outputs.iter_mut() {
                buffer.data_mut().fill(value);
            }
            ProcessorControl::Continue
        }
    }

    #[test]
    fn options_validation() {
        let mut valid = options(1, 2, 2, &[]);
        assert_eq!(valid.validate(), Ok(()));
        // Per-port channel counts must match the output port count.
        valid.output_channel_count = vec![2];
        assert_eq!(
            valid.validate(),
            Err(AudioWorkletNodeError::OutputChannelCountMismatch)
        );
        valid.output_channel_count = vec![2, 1];
        assert_eq!(valid.validate(), Ok(()));
        // Channel counts are 1..=32.
        valid.output_channel_count.clear();
        valid.input_channel_count = 0;
        assert_eq!(
            valid.validate(),
            Err(AudioWorkletNodeError::InvalidChannelCount(0))
        );
        valid.input_channel_count = 33;
        assert_eq!(
            valid.validate(),
            Err(AudioWorkletNodeError::InvalidChannelCount(33))
        );
        valid.input_channel_count = 2;
        valid.output_channel_count = vec![0, 2];
        assert_eq!(
            valid.validate(),
            Err(AudioWorkletNodeError::InvalidChannelCount(0))
        );
    }

    #[test]
    fn node_creation_shape_and_param_face() {
        let mut node = make_node(options(
            2,
            1,
            2,
            &[
                WorkletParamInit {
                    default_value: 0.5,
                    rate: ParamRate::KRate,
                },
                WorkletParamInit {
                    default_value: -1.,
                    rate: ParamRate::ARate,
                },
            ],
        ));
        assert_eq!(node.node_type(), AudioNodeType::AudioWorkletNode);
        assert_eq!(node.input_count(), 2);
        assert_eq!(node.output_count(), 1);

        // Param face: defaults, indexed access, automation events and rate
        // switching all reuse the existing Param machinery.
        assert_eq!(node.get_param(ParamType::WorkletParam(0)).value(), 0.5);
        assert_eq!(node.get_param(ParamType::WorkletParam(1)).value(), -1.);
        node.message(
            AudioNodeMessage::SetParam(
                ParamType::WorkletParam(0),
                UserAutomationEvent::SetValue(0.25),
            ),
            SAMPLE_RATE,
        );
        assert_eq!(node.get_param(ParamType::WorkletParam(0)).value(), 0.25);
        node.message(
            AudioNodeMessage::SetParamRate(ParamType::WorkletParam(0), ParamRate::ARate),
            SAMPLE_RATE,
        );
        assert_eq!(
            node.get_param(ParamType::WorkletParam(0)).rate(),
            ParamRate::ARate
        );
    }

    #[test]
    fn process_underrun_outputs_silence_and_counts() {
        let mut node = make_node(options(1, 1, 2, &[]));
        let chunk = filled_chunk([0.; FRAMES_PER_BLOCK_USIZE], 2);
        let outputs = node.process(chunk, &block_info(0));
        assert_eq!(outputs.len(), 1);
        assert!(outputs.blocks[0].is_silence());
        assert_eq!(node.bridge.underruns(), 1);
        assert_eq!(node.bridge.frame_starved(), 0);
    }

    #[test]
    fn process_ships_inputs_and_consumes_outputs() {
        let node_options = options(
            1,
            1,
            2,
            &[WorkletParamInit {
                default_value: 0.125,
                rate: ParamRate::KRate,
            }],
        );
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(EchoHandler));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );

        let mut values = [0.; FRAMES_PER_BLOCK_USIZE];
        for (frame, value) in values.iter_mut().enumerate() {
            *value = frame as f32 * 0.001;
        }

        // Block 1: inputs are shipped to the worklet thread, outputs underrun.
        let outputs = node.process(filled_chunk(values, 2), &block_info(128));
        assert!(outputs.blocks[0].is_silence());
        assert_eq!(bridge.underruns(), 1);

        // Worklet thread: exactly one pending quantum to process.
        assert_eq!(bridge.pending_len(), 1);
        assert_eq!(pump.pump_once(), PumpOutcome::Delivered);
        assert_eq!(bridge.pending_len(), 0);
        // The completed quantum waits for the next render block.
        assert_eq!(bridge.ready_len(), 1);

        // Block 2: the completed quantum is consumed, sample-exact.
        let outputs = node.process(filled_chunk(values, 2), &block_info(256));
        assert!(!outputs.blocks[0].is_silence());
        for (frame, value) in outputs.blocks[0].data_chan(0).iter().enumerate() {
            assert_eq!(value, &(frame as f32 * 0.001));
        }
        assert_eq!(bridge.underruns(), 1);
        // Frames are conserved: the pool never starves in steady state.
        assert_eq!(bridge.frame_starved(), 0);
    }

    #[test]
    fn quantum_delivers_params_and_timeline() {
        let node_options = options(
            0,
            1,
            2,
            &[
                WorkletParamInit {
                    default_value: 0.75,
                    rate: ParamRate::KRate,
                },
                WorkletParamInit {
                    default_value: 0.5,
                    rate: ParamRate::ARate,
                },
            ],
        );
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        struct Recorder(StdArc<Mutex<Vec<(u64, f64, Vec<f32>)>>>);
        impl AudioWorkletProcessorHandler for Recorder {
            fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
                let mut all_params = Vec::new();
                for buffer in &quantum.params {
                    all_params.extend_from_slice(buffer.data());
                }
                self.0
                    .lock()
                    .unwrap()
                    .push((quantum.frame, quantum.time, all_params));
                for buffer in quantum.outputs.iter_mut() {
                    buffer.data_mut().fill(0.25);
                }
                ProcessorControl::Continue
            }
        }
        let recorded: StdArc<Mutex<Vec<(u64, f64, Vec<f32>)>>> = Default::default();
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(Recorder(recorded.clone())));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );

        // Source-role node (zero inputs): the handler generates the signal.
        let outputs = node.process(Chunk::default(), &block_info(256));
        assert!(outputs.blocks[0].is_silence());
        assert_eq!(pump.pump_once(), PumpOutcome::Delivered);

        let entries = recorded.lock().unwrap();
        let (frame, time, params) = &entries[0];
        assert_eq!(*frame, 256);
        assert!((*time - 256. / SAMPLE_RATE as f64).abs() < 1e-9);
        // k-rate parameter: uniform value across the quantum.
        assert!(params[..FRAMES_PER_BLOCK_USIZE]
            .iter()
            .all(|value| (*value - 0.75).abs() < 1e-6));
        // a-rate parameter: per-frame values, constant for a constant param.
        assert!(params[FRAMES_PER_BLOCK_USIZE..]
            .iter()
            .all(|value| (*value - 0.5).abs() < 1e-6));
        drop(entries);

        // The handler's output travels back to the render thread.
        let outputs = node.process(Chunk::default(), &block_info(384));
        assert!(outputs.blocks[0]
            .data_chan(0)
            .iter()
            .all(|value| (*value - 0.25).abs() < 1e-6));
        // Output channel count falls back to input_channel_count (2).
        assert_eq!(bridge.shape().output_channels, [2u8]);
    }

    #[test]
    fn arate_param_timeline_reaches_processor() {
        let node_options = options(
            0,
            1,
            1,
            &[WorkletParamInit {
                default_value: 0.,
                rate: ParamRate::ARate,
            }],
        );
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        struct Capture(StdArc<Mutex<Vec<f32>>>);
        impl AudioWorkletProcessorHandler for Capture {
            fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
                *self.0.lock().unwrap() = quantum.params[0].data().to_vec();
                ProcessorControl::Continue
            }
        }
        let captured: StdArc<Mutex<Vec<f32>>> = Default::default();
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(Capture(captured.clone())));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );
        // Linear ramp 0.25 → 1. across one quantum's worth of time.
        node.message(
            AudioNodeMessage::SetParam(
                ParamType::WorkletParam(0),
                UserAutomationEvent::SetValue(0.25),
            ),
            SAMPLE_RATE,
        );
        node.message(
            AudioNodeMessage::SetParam(
                ParamType::WorkletParam(0),
                UserAutomationEvent::RampToValueAtTime(
                    RampKind::Linear,
                    1.,
                    FRAMES_PER_BLOCK_USIZE as f64 / SAMPLE_RATE as f64,
                ),
            ),
            SAMPLE_RATE,
        );

        let _ = node.process(Chunk::default(), &block_info(0));
        assert_eq!(pump.pump_once(), PumpOutcome::Delivered);
        let values = captured.lock().unwrap();
        assert_eq!(values.len(), FRAMES_PER_BLOCK_USIZE);
        assert!(
            (values[0] - 0.25).abs() < 1e-3,
            "start of ramp, got {}",
            values[0]
        );
        assert!(
            (*values.last().unwrap() - 1.).abs() < 0.05,
            "end of ramp, got {}",
            values.last().unwrap()
        );
    }

    /// Handler recording whether the outputs it observes are all-zero on
    /// entry, then dirtying them for the next cycle.
    struct OutputZeroProbe(StdArc<Mutex<Vec<bool>>>);

    impl AudioWorkletProcessorHandler for OutputZeroProbe {
        fn process_quantum(&mut self, quantum: &mut WorkletQuantum) -> ProcessorControl {
            let all_zero = quantum
                .outputs
                .iter()
                .all(|buffer| buffer.data().iter().all(|value| *value == 0.));
            self.0.lock().unwrap().push(all_zero);
            for buffer in quantum.outputs.iter_mut() {
                buffer.data_mut().fill(7.);
            }
            ProcessorControl::Continue
        }
    }

    /// Spec (`#process-call`): every `process()` call observes zero-filled
    /// outputs. Frames cycle through the render-side pool, so the pool
    /// entry point must clear the previous cycle's samples.
    #[test]
    fn outputs_are_zeroed_between_cycles() {
        let node_options = options(0, 1, 1, &[]);
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        let observed: StdArc<Mutex<Vec<bool>>> = Default::default();
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(OutputZeroProbe(observed.clone())));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );

        // Block 1 ships a fresh (zeroed) pool frame; the handler dirties it.
        // Block 2 consumes the dirty output and recycles the frame; its own
        // shipped frame is that recycled one, which must be zeroed at pool
        // entry. Without the pool-entry clear, cycle 2 observes the 7.0
        // samples written in cycle 1.
        for block in 0..3 {
            let _ = node.process(Chunk::default(), &block_info(block * 128));
            let _ = pump.pump_once();
        }
        assert_eq!(*observed.lock().unwrap(), [true, true, true]);
    }

    #[test]
    fn finish_control_halts_node() {
        let node_options = options(0, 1, 1, &[]);
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        struct StopAfter(u32);
        impl AudioWorkletProcessorHandler for StopAfter {
            fn process_quantum(&mut self, _: &mut WorkletQuantum) -> ProcessorControl {
                self.0 -= 1;
                if self.0 == 0 {
                    ProcessorControl::Finish
                } else {
                    ProcessorControl::Continue
                }
            }
        }
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(StopAfter(2)));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );

        assert_eq!(pump.pump_once(), PumpOutcome::Idle);
        let _ = node.process(Chunk::default(), &block_info(0));
        assert_eq!(pump.pump_once(), PumpOutcome::Delivered);
        let _ = node.process(Chunk::default(), &block_info(128));
        // Second quantum: the processor returns Finish.
        assert_eq!(pump.pump_once(), PumpOutcome::Halted);
        assert!(bridge.processor_halted());
        assert!(!bridge.processor_failed());

        // From here on the node renders silence and recycles in-flight
        // frames instead of consuming them.
        let outputs = node.process(Chunk::default(), &block_info(256));
        assert!(outputs.blocks[0].is_silence());
        assert_eq!(pump.pump_once(), PumpOutcome::Halted);
    }

    #[test]
    fn onprocessorerror_face() {
        let node_options = options(0, 1, 1, &[]);
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(CountingHandler(0)));
        let mut node = AudioWorkletNode::new(
            AudioWorkletNodeInit {
                options: node_options,
                bridge: StdArc::clone(&bridge),
            },
            ChannelInfo::default(),
        );

        // A processor exception is signalled by the worklet side.
        pump.signal_processor_error();
        assert!(bridge.processor_failed());
        assert!(bridge.processor_halted());
        // The flag is sticky: the script face fires the event once on its
        // first observation, the node stays muted for its lifetime.
        assert!(bridge.processor_failed());

        let _ = node.process(Chunk::default(), &block_info(0));
        let outputs = node.process(Chunk::default(), &block_info(128));
        assert!(outputs.blocks[0].is_silence());
        // A failed processor stops consuming render work entirely.
        assert_eq!(pump.pump_once(), PumpOutcome::Halted);
    }

    #[test]
    fn bridge_backpressure_stages_and_recovers() {
        let node_options = options(0, 1, 1, &[]);
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        // Saturate the ready ring directly (a stalled render thread).
        for _ in 0..TEST_BRIDGE_CAPACITY {
            bridge.push_ready(bridge.shape().make_quantum()).unwrap();
        }
        // One quantum of work for the pump.
        bridge.push_pending(bridge.shape().make_quantum()).unwrap();
        let mut pump =
            AudioWorkletPump::new(StdArc::clone(&bridge), Box::new(CountingHandler(9)));
        assert_eq!(bridge.ready_len(), TEST_BRIDGE_CAPACITY);

        // The output has nowhere to go: the processed quantum is staged.
        assert_eq!(pump.pump_once(), PumpOutcome::Staged);
        // Still no room: the staged quantum stays put.
        assert_eq!(pump.pump_once(), PumpOutcome::Staged);

        // The render thread consumes one quantum; the staged output is
        // delivered on the next pump, behind the remaining pre-filled one.
        assert!(bridge.pop_ready().is_some());
        assert_eq!(pump.pump_once(), PumpOutcome::Delivered);
        let delivered = bridge.pop_ready().expect("staged quantum delivered");
        assert_eq!(delivered.outputs[0].data()[0], 0., "pre-filled frame");
        let delivered = bridge.pop_ready().expect("staged quantum delivered");
        assert_eq!(delivered.outputs[0].data()[0], 9., "staged frame content");
        // Every frame is accounted for: nothing was lost to the backpressure.
        assert_eq!(bridge.pending_len(), 0);
        assert_eq!(bridge.ready_len(), 0);
    }

    #[test]
    fn port_message_ring_backpressure() {
        let port: PortMessageRing = SpscRing::with_capacity(2);
        assert_eq!(port.push(vec![1, 2, 3]), Ok(()));
        assert_eq!(port.push(vec![4]), Ok(()));
        // A flooding sender is throttled explicitly instead of growing the
        // queue without bound.
        assert_eq!(port.push(vec![5]), Err(SpscRingError::Full(vec![5])));
        assert_eq!(port.pop().as_deref(), Some(&[1, 2, 3][..]));
        assert_eq!(port.pop().as_deref(), Some(&[4][..]));
        assert_eq!(port.pop(), None);
    }

    // --- real render-thread integration ---

    /// Sink that paces the render thread one block at a time so the test is
    /// fully deterministic: `has_enough_data()` gates the event loop, the
    /// test thread releases exactly one block per step and runs the pump
    /// between steps.
    struct LockstepSink {
        state: StdArc<LockstepState>,
        inner: Mutex<Option<Sender<AudioRenderThreadMsg>>>,
    }

    struct LockstepState {
        /// When true the render thread must wait for the test to release the
        /// next block.
        gate: Mutex<bool>,
        signal: Condvar,
        /// Channel 0 of every block the destination produced.
        collected: Mutex<Vec<f32>>,
    }

    impl LockstepState {
        fn release_next_block(&self) {
            *self.gate.lock().unwrap() = false;
            self.signal.notify_all();
        }

        fn wait_for_blocks(&self, count: usize) {
            let mut collected = self.collected.lock().unwrap();
            while collected.len() < count {
                let (guard, timeout) = self
                    .signal
                    .wait_timeout(collected, Duration::from_secs(5))
                    .unwrap();
                assert!(
                    !timeout.timed_out(),
                    "timed out waiting for block {count}"
                );
                collected = guard;
            }
        }
    }

    static LOCKSTEP: OnceLock<StdArc<LockstepState>> = OnceLock::new();

    impl AudioSink for LockstepSink {
        fn init(&self, _: f32, sender: Sender<AudioRenderThreadMsg>) -> Result<(), AudioSinkError> {
            *self.inner.lock().unwrap() = Some(sender);
            Ok(())
        }
        fn init_stream(
            &self,
            _: u8,
            _: f32,
            _: Box<dyn MediaSocket>,
        ) -> Result<(), AudioSinkError> {
            Ok(())
        }
        fn play(&self) -> Result<(), AudioSinkError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), AudioSinkError> {
            Ok(())
        }
        fn has_enough_data(&self) -> bool {
            *self.state.gate.lock().unwrap()
        }
        fn push_data(&self, chunk: Chunk) -> Result<(), AudioSinkError> {
            let mut collected = self.state.collected.lock().unwrap();
            let block = chunk.blocks.first().expect("destination block");
            if block.is_silence() {
                collected.extend(std::iter::repeat(0.).take(FRAMES_PER_BLOCK_USIZE));
            } else {
                collected.extend_from_slice(block.data_chan(0));
            }
            *self.state.gate.lock().unwrap() = true;
            drop(collected);
            self.state.signal.notify_all();
            Ok(())
        }
        fn set_eos_callback(
            &self,
            _: Box<dyn Fn(Box<dyn AsRef<[f32]>>) + Send + Sync + 'static>,
        ) {
        }
    }

    struct LockstepBackend;

    impl crate::AudioBackend for LockstepBackend {
        type Sink = LockstepSink;
        fn make_decoder() -> Box<dyn AudioDecoder> {
            panic!("decoder is not exercised by this test")
        }
        fn make_sink() -> Result<Self::Sink, AudioSinkError> {
            let state = LOCKSTEP
                .get_or_init(|| {
                    StdArc::new(LockstepState {
                        gate: Mutex::new(true),
                        signal: Condvar::new(),
                        collected: Mutex::new(Vec::new()),
                    })
                })
                .clone();
            Ok(LockstepSink {
                state,
                inner: Mutex::new(None),
            })
        }
        fn make_streamreader(
            _: MediaStreamId,
            _: f32,
        ) -> Result<Box<dyn crate::AudioStreamReader + Send>, AudioSinkError> {
            panic!("stream reader is not exercised by this test")
        }
    }

    #[test]
    fn render_thread_create_node_end_to_end() {
        let state = LOCKSTEP
            .get_or_init(|| {
                StdArc::new(LockstepState {
                    gate: Mutex::new(true),
                    signal: Condvar::new(),
                    collected: Mutex::new(Vec::new()),
                })
            })
            .clone();
        state.collected.lock().unwrap().clear();

        let (sender, receiver) = mpsc::channel();
        let (init_sender, init_receiver) = mpsc::channel();
        let graph = AudioGraph::new(2);
        let dest_input: PortId<InputPort> = graph.dest_id().input(0);
        let render_sender = sender.clone();
        let handle = thread::spawn(move || {
            AudioRenderThread::start::<LockstepBackend>(
                receiver,
                render_sender,
                SAMPLE_RATE,
                graph,
                AudioContextOptions::RealTimeAudioContext(Default::default()),
                init_sender,
            );
        });
        init_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("render thread init");

        // Constant source → AudioWorkletNode (echo) → destination, wired
        // through the real render-thread CreateNode path.
        let (created, created_rx) = mpsc::channel();
        sender
            .send(AudioRenderThreadMsg::CreateNode(
                AudioNodeInit::ConstantSourceNode(ConstantSourceNodeOptions { offset: 0.5 }),
                created.clone(),
                ChannelInfo::default(),
            ))
            .unwrap();
        let source = created_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("source created");

        let node_options = options(1, 1, 2, &[]);
        let bridge = StdArc::new(node_options.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        sender
            .send(AudioRenderThreadMsg::CreateNode(
                AudioNodeInit::AudioWorkletNode(AudioWorkletNodeInit {
                    options: node_options,
                    bridge: StdArc::clone(&bridge),
                }),
                created,
                ChannelInfo::default(),
            ))
            .unwrap();
        let worklet = created_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("worklet node created");

        sender
            .send(AudioRenderThreadMsg::ConnectPorts(
                source.output(0),
                worklet.input(0),
            ))
            .unwrap();
        sender
            .send(AudioRenderThreadMsg::ConnectPorts(worklet.output(0), dest_input))
            .unwrap();
        sender
            .send(AudioRenderThreadMsg::MessageNode(
                source,
                AudioNodeMessage::AudioScheduledSourceNode(
                    AudioScheduledSourceNodeMessage::Start(0.),
                ),
            ))
            .unwrap();

        // The pump runs on the worklet (JS) thread in production; here it is
        // the test thread, stepped between rendered blocks.
        let mut pump = AudioWorkletPump::new(bridge, Box::new(EchoHandler));

        let (resumed, resumed_rx) = mpsc::channel();
        sender.send(AudioRenderThreadMsg::Resume(resumed)).unwrap();
        resumed_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        const BLOCKS: usize = 5;
        for handled in 0..BLOCKS {
            state.release_next_block();
            sender.send(AudioRenderThreadMsg::SinkNeedData).unwrap();
            state.wait_for_blocks((handled + 1) * FRAMES_PER_BLOCK_USIZE);
            let _ = pump.pump_once();
        }

        let (closed, closed_rx) = mpsc::channel();
        sender.send(AudioRenderThreadMsg::Close(closed)).unwrap();
        closed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().expect("render thread exits on Close");

        let collected = state.collected.lock().unwrap();
        assert_eq!(collected.len(), BLOCKS * FRAMES_PER_BLOCK_USIZE);
        // Block 1 underruns (the pump has not produced yet); every later
        // block is the echo of the constant source.
        assert!(collected[..FRAMES_PER_BLOCK_USIZE]
            .iter()
            .all(|value| *value == 0.));
        for (index, value) in collected[FRAMES_PER_BLOCK_USIZE..].iter().enumerate() {
            assert!(
                (*value - 0.5).abs() < 1e-6,
                "block {} sample {} = {}",
                index / FRAMES_PER_BLOCK_USIZE + 2,
                index % FRAMES_PER_BLOCK_USIZE,
                value
            );
        }
    }

    /// Handler that only counts quanta into a shared counter, mirroring a
    /// processor whose observable side effect is "process() got called".
    struct SharedCounter(StdArc<Mutex<u32>>);

    impl AudioWorkletProcessorHandler for SharedCounter {
        fn process_quantum(&mut self, _: &mut WorkletQuantum) -> ProcessorControl {
            *self.0.lock().unwrap() += 1;
            ProcessorControl::Continue
        }
    }

    /// RED pin for the always-process semantics (e145): a worklet node with
    /// no path to the destination — free outputs connected to nothing, or
    /// zero outputs at all — must still be processed every render quantum.
    /// Its processor is called until it returns false regardless of
    /// connections (spec/Chromium semantics; the pull-graph topsort that
    /// only visits the destination's upstream closure starves it —
    /// .plans/wpt-webaudio-ledger.md §F2 root cause F).
    #[test]
    fn unconnected_worklet_nodes_are_always_processed() {
        let state = LOCKSTEP
            .get_or_init(|| {
                StdArc::new(LockstepState {
                    gate: Mutex::new(true),
                    signal: Condvar::new(),
                    collected: Mutex::new(Vec::new()),
                })
            })
            .clone();
        state.collected.lock().unwrap().clear();

        let (sender, receiver) = mpsc::channel();
        let (init_sender, init_receiver) = mpsc::channel();
        let graph = AudioGraph::new(2);
        let render_sender = sender.clone();
        let handle = thread::spawn(move || {
            AudioRenderThread::start::<LockstepBackend>(
                receiver,
                render_sender,
                SAMPLE_RATE,
                graph,
                AudioContextOptions::RealTimeAudioContext(Default::default()),
                init_sender,
            );
        });
        init_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("render thread init");

        // Two unconnected shapes: node A has a free output port nobody
        // reads; node B has no output ports at all (the
        // automatic-pull / zero-outputs WPT shapes). Neither has a path to
        // the destination.
        let (created, created_rx) = mpsc::channel();
        let options_a = options(1, 1, 2, &[]);
        let bridge_a = StdArc::new(options_a.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        sender
            .send(AudioRenderThreadMsg::CreateNode(
                AudioNodeInit::AudioWorkletNode(AudioWorkletNodeInit {
                    options: options_a,
                    bridge: StdArc::clone(&bridge_a),
                }),
                created.clone(),
                ChannelInfo::default(),
            ))
            .unwrap();
        let worklet_a = created_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("free-output worklet node created");

        let options_b = options(1, 0, 2, &[]);
        let bridge_b = StdArc::new(options_b.make_bridge_with_capacity(TEST_BRIDGE_CAPACITY));
        sender
            .send(AudioRenderThreadMsg::CreateNode(
                AudioNodeInit::AudioWorkletNode(AudioWorkletNodeInit {
                    options: options_b,
                    bridge: StdArc::clone(&bridge_b),
                }),
                created,
                ChannelInfo::default(),
            ))
            .unwrap();
        let worklet_b = created_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("zero-output worklet node created");
        // Nothing is connected to either node; silencing the "unused" lint
        // the conventional way would hide the shapes under test.
        let _ = (worklet_a, worklet_b);

        let count_a: StdArc<Mutex<u32>> = Default::default();
        let count_b: StdArc<Mutex<u32>> = Default::default();
        let mut pump_a =
            AudioWorkletPump::new(bridge_a, Box::new(SharedCounter(count_a.clone())));
        let mut pump_b =
            AudioWorkletPump::new(bridge_b, Box::new(SharedCounter(count_b.clone())));

        let (resumed, resumed_rx) = mpsc::channel();
        sender.send(AudioRenderThreadMsg::Resume(resumed)).unwrap();
        resumed_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        const BLOCKS: usize = 5;
        for handled in 0..BLOCKS {
            state.release_next_block();
            sender.send(AudioRenderThreadMsg::SinkNeedData).unwrap();
            state.wait_for_blocks((handled + 1) * FRAMES_PER_BLOCK_USIZE);
            let _ = pump_a.pump_once();
            let _ = pump_b.pump_once();
        }

        let (closed, closed_rx) = mpsc::channel();
        sender.send(AudioRenderThreadMsg::Close(closed)).unwrap();
        closed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().expect("render thread exits on Close");

        // The pull model leaves both counts at 0 (processors starve);
        // always-processing drives both shapes continuously.
        assert!(
            *count_a.lock().unwrap() >= 2,
            "free-output unconnected node processor called {} times",
            *count_a.lock().unwrap()
        );
        assert!(
            *count_b.lock().unwrap() >= 2,
            "zero-output node processor called {} times",
            *count_b.lock().unwrap()
        );
    }
}
