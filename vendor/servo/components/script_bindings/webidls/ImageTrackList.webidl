/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs ImageTrackList (REQ-BRW-051, Bao fork self-build; contract =
// https://wicg.github.io/image-output/#imagetracklist-interface). With the
// ImageDecoder data plane unimplemented the list is always empty (length 0,
// selectedIndex -1, selectedTrack null) — registered honest boundary.

[Exposed=(Window,DedicatedWorker), SecureContext]
interface ImageTrackList {
  getter ImageTrack (unsigned long index);

  readonly attribute Promise<undefined> ready;
  readonly attribute unsigned long length;
  readonly attribute long selectedIndex;
  readonly attribute ImageTrack? selectedTrack;
};
