/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs raw audio data (REQ-BRW-051, Bao fork self-build; contract =
// https://w3c.github.io/webcodecs/#audiodata-interface + WPT webcodecs).
// [Serializable]/[Transferable] are deliberately not declared yet: the
// structured-clone integration is a later slice, so posting an AudioData
// throws DataCloneError instead of silently copying (honest, registered).
// `[EnforceRange]` must follow `required` on required dictionary members: the
// servo WebIDL parser (WebIDL.py p_DictionaryMemberRequired) rejects the
// member-prefix form used by the W3C IDL, mirroring Gecko's own webidl files.

[Exposed=(Window,DedicatedWorker)]
interface AudioData {
  [Throws] constructor(AudioDataInit init);

  readonly attribute AudioSampleFormat? format;
  readonly attribute float sampleRate;
  readonly attribute unsigned long numberOfFrames;
  readonly attribute unsigned long numberOfChannels;
  readonly attribute unsigned long long duration;
  readonly attribute long long timestamp;

  [Throws] unsigned long allocationSize(AudioDataCopyToOptions options);
  [Throws] undefined copyTo(BufferSource destination, AudioDataCopyToOptions options);
  [Throws] AudioData clone();
  undefined close();
};

dictionary AudioDataInit {
  required AudioSampleFormat format;
  required float sampleRate;
  required [EnforceRange] unsigned long numberOfFrames;
  required [EnforceRange] unsigned long numberOfChannels;
  required [EnforceRange] long long timestamp;
  required BufferSource data;
};

dictionary AudioDataCopyToOptions {
  required [EnforceRange] unsigned long planeIndex;
  [EnforceRange] unsigned long frameOffset = 0;
  [EnforceRange] unsigned long frameCount;
  AudioSampleFormat format;
};

enum AudioSampleFormat {
  "u8",
  "s16",
  "s32",
  "f32",
  "u8-planar",
  "s16-planar",
  "s32-planar",
  "f32-planar",
};
