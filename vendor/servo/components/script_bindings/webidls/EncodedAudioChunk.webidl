/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs compressed audio chunk (REQ-BRW-051, Bao fork self-build;
// contract = https://w3c.github.io/webcodecs/#encodedaudiochunk-interface).
// The `transfer` member of `EncodedAudioChunkInit` is omitted: buffer
// detaching during construction is part of the transfer integration slice
// (registered boundary); construction copies `data` instead.
// `[EnforceRange]` must follow `required` on required dictionary members: the
// servo WebIDL parser rejects the W3C member-prefix form (see AudioData.webidl).

[Exposed=(Window,DedicatedWorker)]
interface EncodedAudioChunk {
  [Throws] constructor(EncodedAudioChunkInit init);
  readonly attribute EncodedAudioChunkType type;
  readonly attribute long long timestamp;           // microseconds
  readonly attribute unsigned long long? duration;  // microseconds
  readonly attribute unsigned long byteLength;

  [Throws] undefined copyTo(BufferSource destination);
};

dictionary EncodedAudioChunkInit {
  required EncodedAudioChunkType type;
  required [EnforceRange] long long timestamp;    // microseconds
  [EnforceRange] unsigned long long duration;     // microseconds
  required BufferSource data;
};

enum EncodedAudioChunkType {
  "key",
  "delta",
};
