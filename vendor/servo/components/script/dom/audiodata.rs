/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `AudioData` (REQ-BRW-051, Bao fork self-build; contract =
//! <https://w3c.github.io/webcodecs/#audiodata-interface>).

use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};
use std::cell::Cell;

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
use crate::dom::bindings::codegen::Bindings::AudioDataBinding::{
    AudioDataCopyToOptions, AudioDataInit, AudioDataMethods, AudioSampleFormat,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::num::Finite;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;
use crate::dom::webcodecs_pcm::{copy_samples, is_planar, sample_size};
use webcodecs::audio_data::{AudioSampleFormat as AudioSampleFormatComponent, DecodedAudioFrame};

/// Immutable sample capture handed from `AudioData` to the encode plane
/// (`AudioEncoder::encode`). Plain data; no JS pointers, so it can be queued
/// across a `flush()` microtask without rooting the source object.
pub(crate) struct AudioDataSnapshot {
    pub format: AudioSampleFormat,
    pub sample_rate: f32,
    pub number_of_frames: u32,
    pub number_of_channels: u32,
    pub timestamp: i64,
    pub data: Rc<[u8]>,
}

/// <https://www.w3.org/TR/webcodecs/#audiodata>
#[dom_struct]
pub(crate) struct AudioData {
    reflector_: Reflector,

    /// The in-memory sample data, `None` once the `AudioData` is closed.
    /// Plain bytes (no JS-managed pointers), so it never participates in GC
    /// tracing; heap accounting rides the per-sample bookkeeping of the
    /// webcodecs pipeline.
    #[no_trace]
    #[ignore_malloc_size_of = "Plain sample bytes owned by the webcodecs pipeline"]
    data: DomRefCell<Option<Rc<[u8]>>>,
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-format>
    format: DomRefCell<Option<AudioSampleFormat>>,
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-samplerate>
    sample_rate: Cell<f32>,
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-numberofframes>
    number_of_frames: Cell<u32>,
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-numberofchannels>
    number_of_channels: Cell<u32>,
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-timestamp>
    timestamp: Cell<i64>,
}

impl AudioData {
    fn new_inherited(
        format: AudioSampleFormat,
        sample_rate: f32,
        number_of_frames: u32,
        number_of_channels: u32,
        timestamp: i64,
        data: Rc<[u8]>,
    ) -> AudioData {
        AudioData {
            reflector_: Reflector::new(),
            data: DomRefCell::new(Some(data)),
            format: DomRefCell::new(Some(format)),
            sample_rate: Cell::new(sample_rate),
            number_of_frames: Cell::new(number_of_frames),
            number_of_channels: Cell::new(number_of_channels),
            timestamp: Cell::new(timestamp),
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioDataInit,
        data: Rc<[u8]>,
    ) -> DomRoot<AudioData> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(AudioData::new_inherited(
                init.format,
                *init.sampleRate,
                init.numberOfFrames,
                init.numberOfChannels,
                init.timestamp,
                data,
            )),
            global,
            proto,
        )
    }

    /// Construct an `AudioData` from a plain decoded frame emitted by the
    /// decode plane (REQ-BRW-051 `AudioDecoder` output delivery path;
    /// main-session adjudication 2026-10-21: minimal `pub(crate)` opening of
    /// the constructor surface, not a rewrite of the JS-facing constructor).
    ///
    /// Planar frames are stored plane-concatenated, matching the
    /// `channel_plane_base` layout the `webcodecs_pcm` copy helpers address.
    /// Returns `None` when the frame's frame/channel counts do not fit the
    /// `u32` attribute surface (unreachable for buffer-backed decode output:
    /// a `BufferSource` payload is `u32`-bounded, so a PCM frame re-iterated
    /// from one can never exceed it).
    pub(crate) fn from_decoded_frame(
        cx: &mut JSContext,
        global: &GlobalScope,
        frame: &DecodedAudioFrame,
    ) -> Option<DomRoot<AudioData>> {
        let format = match frame.format {
            AudioSampleFormatComponent::U8 => AudioSampleFormat::U8,
            AudioSampleFormatComponent::S16 => AudioSampleFormat::S16,
            AudioSampleFormatComponent::S32 => AudioSampleFormat::S32,
            AudioSampleFormatComponent::F32 => AudioSampleFormat::F32,
            AudioSampleFormatComponent::U8Planar => AudioSampleFormat::U8_planar,
            AudioSampleFormatComponent::S16Planar => AudioSampleFormat::S16_planar,
            AudioSampleFormatComponent::S32Planar => AudioSampleFormat::S32_planar,
            AudioSampleFormatComponent::F32Planar => AudioSampleFormat::F32_planar,
        };
        let number_of_frames = u32::try_from(frame.number_of_frames).ok()?;
        let number_of_channels = u32::try_from(frame.number_of_channels).ok()?;
        // Interleaved frames carry one buffer; planar frames carry one per
        // channel — concatenate in plane order (AudioData owns a single
        // contiguous sample buffer).
        let data: Rc<[u8]> = frame.data.concat().into();
        Some(reflect_dom_object_with_proto(
            cx,
            Box::new(AudioData::new_inherited(
                format,
                frame.sample_rate as f32,
                number_of_frames,
                number_of_channels,
                frame.timestamp,
                data,
            )),
            global,
            None,
        ))
    }

    /// Plain-data snapshot of the sample buffer + geometry, for the encode
    /// plane (`AudioEncoder::encode`). `None` once the `AudioData` is closed.
    ///
    /// The snapshot is taken at `encode()` time: WebCodecs "AudioData is
    /// immutable" means the bytes cannot change under us, so a shared `Rc`
    /// reference is a copy-free capture (matching the `clone()` semantics in
    /// this module).
    pub(crate) fn encode_snapshot(&self) -> Option<AudioDataSnapshot> {
        Some(AudioDataSnapshot {
            format: (*self.format.borrow())?,
            sample_rate: self.sample_rate.get(),
            number_of_frames: self.number_of_frames.get(),
            number_of_channels: self.number_of_channels.get(),
            timestamp: self.timestamp.get(),
            // `Ref` itself is `Clone`, so deref before cloning the option.
            data: (*self.data.borrow()).clone()?,
        })
    }

    /// <https://www.w3.org/TR/webcodecs/#compute-copy-element-count>
    fn compute_copy_element_count(
        &self,
        options: &AudioDataCopyToOptions,
        destination_format: AudioSampleFormat,
    ) -> Fallible<usize> {
        let frames = self.number_of_frames.get() as usize;
        let frame_offset = options.frameOffset as usize;
        // Steps 2-3. A frameOffset at or past the last frame throws.
        if frame_offset >= frames {
            return Err(Error::Range(c"frameOffset is out of range".to_owned()));
        }
        let Some(source_format) = *self.format.borrow() else {
            return Err(Error::InvalidState(None));
        };
        let channels = self.number_of_channels.get() as usize;
        let plane_index = options.planeIndex as usize;
        // The plane must exist, and interleaved data has a single plane;
        // both are RangeErrors so allocationSize gets the same validation.
        if plane_index >= channels {
            return Err(Error::Range(c"planeIndex is out of range".to_owned()));
        }
        if !is_planar(source_format) && plane_index != 0 {
            return Err(Error::Range(
                c"copyTo from interleaved data with non-zero planeIndex throws".to_owned(),
            ));
        }
        // Steps 4-5. frameCount defaults to, and is clamped to, the frames
        // remaining after frameOffset. The AudioData's allocation is
        // guaranteed to hold `frames` frames (the constructor rejects
        // undersized data), so no further source-side clamp exists.
        let frame_count = match options.frameCount {
            Some(count) => count as usize,
            None => frames - frame_offset,
        }
        .min(frames - frame_offset);
        // Step 6. The element count is expressed in destination samples:
        // one per frame for a planar destination, one per frame per channel
        // for an interleaved one.
        Ok(frame_count
            * if is_planar(destination_format) {
                1
            } else {
                channels
            })
    }
}

impl AudioDataMethods<crate::DomTypeHolder> for AudioData {
    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-audiodata>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &AudioDataInit,
    ) -> Fallible<DomRoot<AudioData>> {
        // Step 2. If init.numberOfFrames is 0, throw a TypeError.
        if init.numberOfFrames == 0 {
            return Err(Error::Type(c"The numberOfFrames member is 0".to_owned()));
        }
        // Step 3. If init.numberOfChannels is 0, throw a TypeError.
        if init.numberOfChannels == 0 {
            return Err(Error::Type(c"The numberOfChannels member is 0".to_owned()));
        }
        // Step 6. Let resourceSize be the result of computing the number of
        // samples required for init.data. If the format describes a planar
        // format, the number of samples is init.numberOfFrames *
        // init.numberOfChannels, otherwise init.numberOfFrames.
        let no_gc = cx.no_gc();
        let bytes = get_buffer_source_slice(&init.data, no_gc);
        let channels = init.numberOfChannels as usize;
        let frames = init.numberOfFrames as usize;
        let required = if is_planar(init.format) {
            frames.checked_mul(channels)
        } else {
            Some(frames)
        }
        .and_then(|samples| samples.checked_mul(sample_size(init.format)))
        .ok_or_else(|| Error::Type(c"AudioDataInit.data needs to be big enough".to_owned()))?;
        if bytes.len() < required {
            return Err(Error::Type(
                c"AudioDataInit.data needs to be big enough".to_owned(),
            ));
        }
        // Steps 8-9. Detach and copy: the AudioData owns an immutable copy of
        // the samples ("input array is copied on construction" WPT clause).
        let owned: Rc<[u8]> = bytes[..required].to_vec().into();
        Ok(AudioData::new(cx, global, proto, init, owned))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-format>
    fn GetFormat(&self) -> Option<AudioSampleFormat> {
        *self.format.borrow()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-samplerate>
    fn SampleRate(&self) -> Finite<f32> {
        Finite::wrap(self.sample_rate.get())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-numberofframes>
    fn NumberOfFrames(&self) -> u32 {
        self.number_of_frames.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-numberofchannels>
    fn NumberOfChannels(&self) -> u32 {
        self.number_of_channels.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-duration>
    fn Duration(&self) -> u64 {
        // duration = frames / sampleRate * 1_000_000 µs, 0 when closed.
        let frames = self.number_of_frames.get();
        let rate = self.sample_rate.get();
        if rate <= 0.0 {
            return 0;
        }
        ((frames as f64) / (rate as f64) * 1_000_000.0) as u64
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-timestamp>
    fn Timestamp(&self) -> i64 {
        self.timestamp.get()
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-allocationsize>
    fn AllocationSize(&self, options: &AudioDataCopyToOptions) -> Fallible<u32> {
        let Some(source_format) = *self.format.borrow() else {
            return Err(Error::InvalidState(None));
        };
        let destination_format = options.format.unwrap_or(source_format);
        let element_count = self.compute_copy_element_count(options, destination_format)?;
        Ok((element_count * sample_size(destination_format)) as u32)
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-copyto>
    #[allow(unsafe_code)]
    fn CopyTo(
        &self,
        mut destination: ArrayBufferViewOrArrayBuffer,
        options: &AudioDataCopyToOptions,
    ) -> Fallible<()> {
        let Some(source_format) = *self.format.borrow() else {
            return Err(Error::InvalidState(None));
        };
        let destination_format = options.format.unwrap_or(source_format);
        let element_count = self.compute_copy_element_count(options, destination_format)?;

        let channels = self.number_of_channels.get() as usize;
        let frames = self.number_of_frames.get() as usize;
        let plane_index = options.planeIndex as usize;
        // `Ref` itself is `Clone`, so deref before cloning the option.
        let source_data = (*self.data.borrow()).clone().unwrap_or_default();

        // Take the destination bytes. `copyTo` never grows the destination:
        // a buffer smaller than the expected byte count throws RangeError
        // (spec: "if bytesExpected is greater than destination.byteLength").
        // A detached buffer has zero bytes and fails the same check unless
        // the requested copy is empty (a zero-element copy is a no-op,
        // detached or not).
        let cx = unsafe { JSContext::get_from_thread() }.expect("no active JS context");
        let destination_slice = match &mut destination {
            ArrayBufferViewOrArrayBuffer::ArrayBufferView(view) => {
                view.as_mut_slice_safe(cx.no_gc())
            },
            ArrayBufferViewOrArrayBuffer::ArrayBuffer(buffer) => {
                buffer.as_mut_slice_safe(cx.no_gc())
            },
        }
        .unwrap_or(&mut []);
        if element_count * sample_size(destination_format) > destination_slice.len() {
            return Err(Error::Range(c"destination is too small".to_owned()));
        }

        // Write the converted samples through the shared PCM copy arithmetic
        // (element_count counts destination samples; the copy loop is
        // framed). plane_index is already validated by
        // compute_copy_element_count (plane exists; single plane for
        // interleaved sources).
        let frame_count = element_count
            / if is_planar(destination_format) {
                1
            } else {
                channels
            };
        copy_samples(
            source_format,
            &source_data,
            channels,
            frames,
            options.frameOffset as usize,
            frame_count,
            destination_format,
            destination_slice,
            plane_index,
        );
        Ok(())
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-clone>
    #[allow(unsafe_code)]
    fn Clone(&self) -> Fallible<DomRoot<AudioData>> {
        // `Ref` itself is `Clone`, so deref before cloning the option (a bare
        // `.clone()` here would clone the guard, not the buffer).
        let Some(data) = (*self.data.borrow()).clone() else {
            return Err(Error::InvalidState(None));
        };
        let Some(format) = *self.format.borrow() else {
            return Err(Error::InvalidState(None));
        };
        let global = self.global();
        let cloned = AudioData::new_inherited(
            format,
            self.sample_rate.get(),
            self.number_of_frames.get(),
            self.number_of_channels.get(),
            self.timestamp.get(),
            data,
        );
        // `reflect_dom_object_with_proto` is generic over `&U: DerivedFrom`,
        // so it needs the concrete `&GlobalScope`; deref the `DomRoot` root
        // explicitly (DomRoot does not implement DerivedFrom).
        let global: &GlobalScope = &global;
        let mut cx = unsafe { JSContext::get_from_thread() }.expect("no active JS context");
        Ok(reflect_dom_object_with_proto(
            &mut cx,
            Box::new(cloned),
            global,
            None,
        ))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-audiodata-close>
    fn Close(&self) {
        *self.data.borrow_mut() = None;
        *self.format.borrow_mut() = None;
        self.sample_rate.set(0.0);
        self.number_of_frames.set(0);
        self.number_of_channels.set(0);
        self.timestamp.set(0);
    }
}
