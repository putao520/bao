/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// skip-unless CARGO_FEATURE_WEBCODECS

// WebCodecs video color space (REQ-BRW-051, Bao fork self-build; contract =
// https://w3c.github.io/webcodecs/#videocolorspace-interface). The `toJSON()`
// JSONifier is a registered boundary (not yet implemented).

[Exposed=(Window,DedicatedWorker)]
interface VideoColorSpace {
  constructor(optional VideoColorSpaceInit init = {});

  readonly attribute VideoColorPrimaries? primaries;
  readonly attribute VideoTransferCharacteristics? transfer;
  readonly attribute VideoMatrixCoefficients? matrix;
  readonly attribute boolean? fullRange;
};

dictionary VideoColorSpaceInit {
  VideoColorPrimaries? primaries = null;
  VideoTransferCharacteristics? transfer = null;
  VideoMatrixCoefficients? matrix = null;
  boolean? fullRange = null;
};

enum VideoColorPrimaries {
  "bt709",
  "bt470bg",
  "smpte170m",
  "bt2020",
  "smpte432",
};

enum VideoTransferCharacteristics {
  "bt709",
  "smpte170m",
  "iec61966-2-1",
  "linear",
  "pq",
  "hlg",
};

enum VideoMatrixCoefficients {
  "rgb",
  "bt709",
  "bt470bg",
  "smpte170m",
  "bt2020-ncl",
};
