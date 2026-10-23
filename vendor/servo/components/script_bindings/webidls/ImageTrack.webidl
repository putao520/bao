/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs ImageTrack (REQ-BRW-051, Bao fork self-build; contract =
// https://wicg.github.io/image-output/#imagetrack-interface). Tracks are only
// produced once the ImageDecoder data plane lands; the type itself carries the
// full surface for feature detection and idlharness.

[Exposed=(Window,DedicatedWorker), SecureContext]
interface ImageTrack {
  readonly attribute boolean animated;
  readonly attribute unsigned long frameCount;
  readonly attribute unrestricted float repetitionCount;
  attribute boolean selected;
};
