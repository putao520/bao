/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs raw video frame (REQ-BRW-051, Bao fork self-build; contract =
// https://w3c.github.io/webcodecs/#videoframe-interface + WPT webcodecs).
// Scope boundary (registered): only the buffer-backed constructor is exposed;
// the CanvasImageSource-based constructor, 10/12-bit plane conversion and
// rect-cropping copies are later slices and rejected honestly (NotSupported).
// [Serializable]/[Transferable] deliberately not declared yet (same reason as
// AudioData).
// `[EnforceRange]` must follow `required` on required dictionary members: the
// servo WebIDL parser rejects the W3C member-prefix form (see AudioData.webidl).

[Exposed=(Window,DedicatedWorker)]
interface VideoFrame {
  [Throws] constructor(BufferSource data, VideoFrameBufferInit init);

  readonly attribute VideoPixelFormat? format;
  readonly attribute unsigned long codedWidth;
  readonly attribute unsigned long codedHeight;
  readonly attribute DOMRectReadOnly? codedRect;
  readonly attribute DOMRectReadOnly? visibleRect;
  readonly attribute double rotation;
  readonly attribute boolean flip;
  readonly attribute unsigned long displayWidth;
  readonly attribute unsigned long displayHeight;
  readonly attribute unsigned long long? duration;  // microseconds
  readonly attribute long long timestamp;           // microseconds
  readonly attribute VideoColorSpace colorSpace;

  VideoFrameMetadata metadata();

  [Throws] unsigned long allocationSize(
      optional VideoFrameCopyToOptions options = {});
  Promise<sequence<PlaneLayout>> copyTo(
      BufferSource destination,
      optional VideoFrameCopyToOptions options = {});
  [Throws] VideoFrame clone();
  undefined close();
};

dictionary VideoFrameBufferInit {
  required VideoPixelFormat format;
  required [EnforceRange] unsigned long codedWidth;
  required [EnforceRange] unsigned long codedHeight;
  required [EnforceRange] long long timestamp;  // microseconds
  [EnforceRange] unsigned long long duration;   // microseconds

  // Default layout is tightly-packed.
  sequence<PlaneLayout> layout;

  // Default visible rect is coded size positioned at (0,0)
  DOMRectInit visibleRect;

  double rotation = 0;
  boolean flip = false;

  // Default display dimensions match visibleRect.
  [EnforceRange] unsigned long displayWidth;
  [EnforceRange] unsigned long displayHeight;

  VideoColorSpaceInit colorSpace;

  VideoFrameMetadata metadata;
};

dictionary VideoFrameMetadata {
  // Possible members are recorded in the VideoFrame Metadata Registry.
};

dictionary VideoFrameCopyToOptions {
  DOMRectInit rect;
  sequence<PlaneLayout> layout;
  VideoPixelFormat format;
  PredefinedColorSpace colorSpace;
};

dictionary PlaneLayout {
  required [EnforceRange] unsigned long offset;
  required [EnforceRange] unsigned long stride;
};

enum VideoPixelFormat {
  // 4:2:0 Y, U, V
  "I420",
  "I420P10",
  "I420P12",
  // 4:2:0 Y, U, V, A
  "I420A",
  "I420AP10",
  "I420AP12",
  // 4:2:2 Y, U, V
  "I422",
  "I422P10",
  "I422P12",
  // 4:2:2 Y, U, V, A
  "I422A",
  "I422AP10",
  "I422AP12",
  // 4:4:4 Y, U, V
  "I444",
  "I444P10",
  "I444P12",
  // 4:4:4 Y, U, V, A
  "I444A",
  "I444AP10",
  "I444AP12",
  // 4:2:0 Y, UV
  "NV12",
  // 4:4:4 RGBA
  "RGBA",
  // 4:4:4 RGBX (opaque)
  "RGBX",
  // 4:4:4 BGRA
  "BGRA",
  // 4:4:4 BGRX (opaque)
  "BGRX",
};
