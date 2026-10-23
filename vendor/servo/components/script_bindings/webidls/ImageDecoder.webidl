/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs ImageDecoder (REQ-BRW-051, Bao fork self-build; contract =
// https://wicg.github.io/image-output/ + WPT webcodecs image-decoder tests).
// Scope boundary (registered): the decoding data plane is a later slice —
// `decode()` rejects NotSupportedError and `isTypeSupported()` resolves false
// honestly. `ImageBufferSource` is narrowed to `BufferSource` (the
// ReadableStream incremental-delivery path is not implemented).

[Exposed=(Window,DedicatedWorker), SecureContext]
interface ImageDecoder {
  [Throws] constructor(ImageDecoderInit init);

  readonly attribute DOMString type;
  readonly attribute boolean complete;
  readonly attribute Promise<undefined> completed;
  readonly attribute ImageTrackList tracks;

  Promise<ImageDecodeResult> decode(optional ImageDecodeOptions options = {});
  [Throws] undefined reset();
  [Throws] undefined close();

  static Promise<boolean> isTypeSupported(DOMString type);
};

dictionary ImageDecoderInit {
  required DOMString type;
  required BufferSource data;
  ColorSpaceConversion colorSpaceConversion = "default";
  [EnforceRange] unsigned long desiredWidth;
  [EnforceRange] unsigned long desiredHeight;
  boolean preferAnimation;
};

dictionary ImageDecodeOptions {
  [EnforceRange] unsigned long frameIndex = 0;
  boolean completeFramesOnly = true;
};

dictionary ImageDecodeResult {
  required VideoFrame image;
  required boolean complete;
};
