/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs codec control interfaces (REQ-BRW-051, Bao fork self-build;
// upstream servo has no implementation; contract =
// https://w3c.github.io/webcodecs/ 2026-10-19 WD + WPT webcodecs oracle).
// `[EnforceRange]` must follow `required` on required dictionary members: the
// servo WebIDL parser rejects the W3C member-prefix form (see AudioData.webidl).

// AllowSharedBufferSource has no upstream-wide typedef in this tree (the spec
// defines it as `(ArrayBufferView or ArrayBuffer)`, optionally
// SharedArrayBuffer-backed). The generated union is identical in shape to
// `BufferSource`; the SharedArrayBuffer member is the only difference, and
// this fork's buffer-source plumbing does not carry a SAB variant
// (buffer_source.rs ArrayBufferViewOrArrayBuffer has none), so the shared
// surface is narrower than the spec name implies. Adjudicated 2026-10-21
// (main session): without this typedef codegen parser.finish() fails when the
// webcodecs webidl set is enabled.
typedef (ArrayBufferView or ArrayBuffer) AllowSharedBufferSource;

[Exposed=(Window,DedicatedWorker), SecureContext]
interface AudioDecoder : EventTarget {
  [Throws] constructor(AudioDecoderInit init);

  readonly attribute CodecState state;
  readonly attribute unsigned long decodeQueueSize;
  attribute EventHandler ondequeue;

  [Throws] undefined configure(AudioDecoderConfig config);
  [Throws] undefined decode(EncodedAudioChunk chunk);
  [Throws] Promise<undefined> flush();
  [Throws] undefined reset();
  [Throws] undefined close();

  [Throws] static Promise<AudioDecoderSupport> isConfigSupported(AudioDecoderConfig config);
};

dictionary AudioDecoderInit {
  required AudioDataOutputCallback output;
  required WebCodecsErrorCallback error;
};

callback AudioDataOutputCallback = undefined(AudioData output);

[Exposed=(Window,DedicatedWorker), SecureContext]
interface VideoDecoder : EventTarget {
  [Throws] constructor(VideoDecoderInit init);

  readonly attribute CodecState state;
  readonly attribute unsigned long decodeQueueSize;
  attribute EventHandler ondequeue;

  [Throws] undefined configure(VideoDecoderConfig config);
  [Throws] undefined decode(EncodedVideoChunk chunk);
  [Throws] Promise<undefined> flush();
  [Throws] undefined reset();
  [Throws] undefined close();

  [Throws] static Promise<VideoDecoderSupport> isConfigSupported(VideoDecoderConfig config);
};

dictionary VideoDecoderInit {
  required VideoFrameOutputCallback output;
  required WebCodecsErrorCallback error;
};

callback VideoFrameOutputCallback = undefined(VideoFrame output);

[Exposed=(Window,DedicatedWorker), SecureContext]
interface AudioEncoder : EventTarget {
  [Throws] constructor(AudioEncoderInit init);

  readonly attribute CodecState state;
  readonly attribute unsigned long encodeQueueSize;
  attribute EventHandler ondequeue;

  [Throws] undefined configure(AudioEncoderConfig config);
  [Throws] undefined encode(AudioData data);
  [Throws] Promise<undefined> flush();
  [Throws] undefined reset();
  [Throws] undefined close();

  [Throws] static Promise<AudioEncoderSupport> isConfigSupported(AudioEncoderConfig config);
};

dictionary AudioEncoderInit {
  required EncodedAudioChunkOutputCallback output;
  required WebCodecsErrorCallback error;
};

callback EncodedAudioChunkOutputCallback =
    undefined(EncodedAudioChunk output,
              optional EncodedAudioChunkMetadata metadata = {});

dictionary EncodedAudioChunkMetadata {
  AudioDecoderConfig decoderConfig;
};

[Exposed=(Window,DedicatedWorker), SecureContext]
interface VideoEncoder : EventTarget {
  [Throws] constructor(VideoEncoderInit init);

  readonly attribute CodecState state;
  readonly attribute unsigned long encodeQueueSize;
  attribute EventHandler ondequeue;

  [Throws] undefined configure(VideoEncoderConfig config);
  [Throws] undefined encode(VideoFrame frame, optional VideoEncoderEncodeOptions options = {});
  [Throws] Promise<undefined> flush();
  [Throws] undefined reset();
  [Throws] undefined close();

  [Throws] static Promise<VideoEncoderSupport> isConfigSupported(VideoEncoderConfig config);
};

dictionary VideoEncoderInit {
  required EncodedVideoChunkOutputCallback output;
  required WebCodecsErrorCallback error;
};

callback EncodedVideoChunkOutputCallback =
    undefined(EncodedVideoChunk chunk,
              optional EncodedVideoChunkMetadata metadata = {});

dictionary EncodedVideoChunkMetadata {
  VideoDecoderConfig decoderConfig;
  SvcOutputMetadata svc;
  BufferSource alphaSideData;
};

dictionary SvcOutputMetadata {
  unsigned long temporalLayerId;
};

dictionary AudioDecoderSupport {
  boolean supported;
  AudioDecoderConfig config;
};

dictionary VideoDecoderSupport {
  boolean supported;
  VideoDecoderConfig config;
};

dictionary AudioEncoderSupport {
  boolean supported;
  AudioEncoderConfig config;
};

dictionary VideoEncoderSupport {
  boolean supported;
  VideoEncoderConfig config;
};

dictionary AudioDecoderConfig {
  required DOMString codec;
  required [EnforceRange] unsigned long sampleRate;
  required [EnforceRange] unsigned long numberOfChannels;
  AllowSharedBufferSource description;
};

dictionary VideoDecoderConfig {
  required DOMString codec;
  AllowSharedBufferSource description;
  [EnforceRange] unsigned long codedWidth;
  [EnforceRange] unsigned long codedHeight;
  [EnforceRange] unsigned long displayAspectWidth;
  [EnforceRange] unsigned long displayAspectHeight;
  VideoColorSpaceInit colorSpace;
  HardwareAcceleration hardwareAcceleration = "no-preference";
  boolean optimizeForLatency;
  double rotation = 0;
  boolean flip = false;
};

dictionary AudioEncoderConfig {
  required DOMString codec;
  required [EnforceRange] unsigned long sampleRate;
  required [EnforceRange] unsigned long numberOfChannels;
  [EnforceRange] unsigned long long bitrate;
  BitrateMode bitrateMode = "variable";
};

dictionary VideoEncoderConfig {
  required DOMString codec;
  required [EnforceRange] unsigned long width;
  required [EnforceRange] unsigned long height;
  [EnforceRange] unsigned long displayWidth;
  [EnforceRange] unsigned long displayHeight;
  [EnforceRange] unsigned long long bitrate;
  double framerate;
  HardwareAcceleration hardwareAcceleration = "no-preference";
  AlphaOption alpha = "discard";
  DOMString scalabilityMode;
  VideoEncoderBitrateMode bitrateMode = "variable";
  LatencyMode latencyMode = "quality";
  DOMString contentHint;
};

enum HardwareAcceleration {
  "no-preference",
  "prefer-hardware",
  "prefer-software",
};

enum AlphaOption {
  "keep",
  "discard",
};

enum LatencyMode {
  "quality",
  "realtime"
};

dictionary VideoEncoderEncodeOptions {
  boolean keyFrame = false;
};

enum VideoEncoderBitrateMode {
  "constant",
  "variable",
  "quantizer"
};

// `BitrateMode` (AudioEncoderConfig.bitrateMode above) was referenced without
// a definition anywhere in the tree — the same parse-failure class as the
// AllowSharedBufferSource typedef above (codegen parser.finish() fails at
// enable time; hard finding #3, confirmed by the main session 2026-10-21).
// Values per the WebCodecs spec AudioEncoderConfig enum.
enum BitrateMode {
  "constant",
  "variable",
  "quantizer"
};

enum CodecState {
  "unconfigured",
  "configured",
  "closed"
};

callback WebCodecsErrorCallback = undefined(DOMException error);
