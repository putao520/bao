/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051
//! WebCodecs `EncodedVideoChunk` (REQ-BRW-051, Bao fork self-build; contract
//! = <https://w3c.github.io/webcodecs/#encodedvideochunk-interface>). Same
//! `transfer` scope boundary as `EncodedAudioChunk` (webidl header):
//! construction copies `data`.

use std::rc::Rc;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::reflector::{Reflector, reflect_dom_object_with_proto};

use crate::dom::bindings::buffer_source::get_buffer_source_slice;
use crate::dom::bindings::codegen::Bindings::EncodedVideoChunkBinding::{
    EncodedVideoChunkInit, EncodedVideoChunkMethods, EncodedVideoChunkType,
};
use crate::dom::bindings::codegen::UnionTypes::ArrayBufferViewOrArrayBuffer;
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::root::DomRoot;
use crate::dom::globalscope::GlobalScope;

/// <https://www.w3.org/TR/webcodecs/#encodedvideochunk>
#[dom_struct]
pub(crate) struct EncodedVideoChunk {
    reflector_: Reflector,
    type_: EncodedVideoChunkType,
    timestamp: i64,
    duration: Option<u64>,
    /// Immutable payload; plain bytes, never JS-managed.
    #[no_trace]
    #[ignore_malloc_size_of = "Plain encoded bytes owned by the webcodecs pipeline"]
    data: Rc<[u8]>,
}

impl EncodedVideoChunk {
    fn new_inherited(
        type_: EncodedVideoChunkType,
        timestamp: i64,
        duration: Option<u64>,
        data: Rc<[u8]>,
    ) -> EncodedVideoChunk {
        EncodedVideoChunk {
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
        type_: EncodedVideoChunkType,
        timestamp: i64,
        duration: Option<u64>,
        data: Rc<[u8]>,
    ) -> DomRoot<EncodedVideoChunk> {
        reflect_dom_object_with_proto(
            cx,
            Box::new(EncodedVideoChunk::new_inherited(
                type_, timestamp, duration, data,
            )),
            global,
            proto,
        )
    }

    /// Chunk-type / timestamp / duration / byte payload for the decoder
    /// control plane (`VideoDecoder::decode`), mirroring the component
    /// `EncodedVideoChunk` struct.
    pub(crate) fn chunk_type(&self) -> EncodedVideoChunkType {
        self.type_
    }

    pub(crate) fn payload(&self) -> Rc<[u8]> {
        self.data.clone()
    }

    pub(crate) fn chunk_timestamp(&self) -> i64 {
        self.timestamp
    }

    pub(crate) fn chunk_duration(&self) -> Option<i64> {
        self.duration.map(i64::try_from).and_then(Result::ok)
    }
}

impl EncodedVideoChunkMethods<crate::DomTypeHolder> for EncodedVideoChunk {
    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-encodedvideochunk>
    fn Constructor(
        cx: &mut JSContext,
        global: &GlobalScope,
        proto: Option<HandleObject>,
        init: &EncodedVideoChunkInit,
    ) -> Fallible<DomRoot<EncodedVideoChunk>> {
        let no_gc = cx.no_gc();
        let bytes = get_buffer_source_slice(&init.data, no_gc);
        let owned: Rc<[u8]> = bytes.to_vec().into();
        Ok(EncodedVideoChunk::new(
            cx,
            global,
            proto,
            init.type_,
            init.timestamp,
            init.duration,
            owned,
        ))
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-type>
    fn Type(&self) -> EncodedVideoChunkType {
        self.type_
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-timestamp>
    fn Timestamp(&self) -> i64 {
        self.timestamp
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-duration>
    fn GetDuration(&self) -> Option<u64> {
        self.duration
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-bytelength>
    fn ByteLength(&self) -> u32 {
        self.data.len() as u32
    }

    /// <https://www.w3.org/TR/webcodecs/#dom-encodedvideochunk-copyto>
    #[allow(unsafe_code)]
    fn CopyTo(&self, mut destination: ArrayBufferViewOrArrayBuffer) -> Fallible<()> {
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
