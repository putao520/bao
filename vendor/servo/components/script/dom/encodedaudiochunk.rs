/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `EncodedAudioChunk` (REQ-BRW-051, Bao fork self-build; contract
//! = <https://w3c.github.io/webcodecs/#encodedaudiochunk-interface>). The
//! `transfer` member of `EncodedAudioChunkInit` is omitted per the webidl
//! header (buffer detaching is the transfer integration slice): construction
//! copies `data` instead.

use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
use crate::dom::bindings::codegen::Bindings::EncodedAudioChunkBinding::{
    EncodedAudioChunkInit, EncodedAudioChunkMethods, EncodedAudioChunkType,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;

/// <https://www.w3.org/TR/webcodecs/#encodedaudiochunk>
#[dom_struct]
pub(crate) struct EncodedAudioChunk {
    reflector_: Reflector,
    type_: EncodedAudioChunkType,
    timestamp: i64,
    duration: Option<u64>,
    /// Immutable payload; plain bytes, never JS-managed.
    #[no_trace]
    #[ignore_malloc_size_of = "Plain encoded bytes owned by the webcodecs pipeline"]
    data: Rc<[u8]>,
}

impl EncodedAudioChunk {
    fn new_inherited(
        type_: EncodedAudioChunkType,
        timestamp: i64,
        duration: Option<u64>,
        data: Rc<[u8]>,
    ) -> EncodedAudioChunk {
        EncodedAudioChunk {
            reflector_: Reflector::new(),
            type_,
            timestamp,
            duration,
            data,
        }
    }

    fn new(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        type_: EncodedAudioChunkType,
        timestamp: i64,
        duration: Option<u64>,
        data: Rc<[u8]>,
    ) -> DomRoot<EncodedAudioChunk> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(EncodedAudioChunk::new_inherited(
                type_, timestamp, duration, data,
            )),
            global,
            proto,
        )
    }

    /// Build a chunk from encode-plane output (Wave B `AudioEncoder`). Audio
    /// has no inter-frame prediction, so every encoded chunk is a key chunk
    /// (WebCodecs `EncodedAudioChunk.type` is always `"key"` for audio).
    pub(crate) fn from_encoded(
        cx: &mut JSContext,
        global: &GlobalScope,
        timestamp: i64,
        duration: Option<u64>,
        data: Rc<[u8]>,
    ) -> DomRoot<EncodedAudioChunk> {
        EncodedAudioChunk::new(
            cx,
            global,
            None,
            EncodedAudioChunkType::Key,
            timestamp,
            duration,
            data,
        )
    }

    /// Byte payload for the decoder control plane (`AudioDecoder::decode`).
    pub(crate) fn payload(&self) -> Rc<[u8]> {
        self.data.clone()
    }

    /// Microsecond timestamp for the decoder control plane.
    pub(crate) fn chunk_timestamp(&self) -> i64 {
        self.timestamp
    }

    /// Microsecond duration for the decoder control plane (`u64` → `i64`
    /// narrowing: the spec surfaces chunk durations as `long long`; values
    /// beyond the `i64` range cannot arise from a `[EnforceRange]` JS number
    /// the binding layer accepts for this attribute width — and an absent
    /// duration stays absent).
    pub(crate) fn chunk_duration(&self) -> Option<i64> {
        self.duration.map(i64::try_from).and_then(Result::ok)
    }
}

impl EncodedAudioChunkMethods<crate::DomTypeHolder> for EncodedAudioChunk {
    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-encodedaudiochunk>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &EncodedAudioChunkInit,
    ) -> Fallible<DomRoot<EncodedAudioChunk>> {
        // "input array is copied on construction" (the init's `transfer`
        // member is a registered boundary — see module doc).
        let no_gc = cx.no_gc();
        let bytes = get_buffer_source_slice(&init.data, no_gc);
        let owned: Rc<[u8]> = bytes.to_vec().into();
        Ok(EncodedAudioChunk::new(
            cx,
            global,
            proto,
            init.type_,
            init.timestamp,
            init.duration,
            owned,
        ))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-type>
    fn Type(&self) -> EncodedAudioChunkType {
        self.type_
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-timestamp>
    fn Timestamp(&self) -> i64 {
        self.timestamp
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-duration>
    fn GetDuration(&self) -> Option<u64> {
        self.duration
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-bytelength>
    fn ByteLength(&self) -> u32 {
        self.data.len() as u32
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedaudiochunk-copyto>
    #[allow(unsafe_code)]
    fn CopyTo(&self, mut destination: ArrayBufferViewOrArrayBuffer) -> Fallible<()> {
        // `copyTo` never grows the destination: a buffer smaller than the
        // chunk throws RangeError. A detached destination has zero bytes and
        // fails the same check unless the chunk is empty.
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
        if destination_slice.len() < self.data.len() {
            return Err(Error::Range(c"destination is too small".to_owned()));
        }
        destination_slice[..self.data.len()].copy_from_slice(&self.data);
        Ok(())
    }
}
