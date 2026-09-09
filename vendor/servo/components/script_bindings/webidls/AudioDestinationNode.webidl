/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://webaudio.github.io/web-audio-api/#dom-audiodestinationnode
 */

// (Bao) Exposed=(Window,Worker): BaseAudioContext.destination return type,
// exposed by type-visibility consequence (REQ-BRW-004 C15, user ruling
// 2026-09-09).
[Exposed=(Window,Worker)]
interface AudioDestinationNode : AudioNode {
  readonly attribute unsigned long maxChannelCount;
};
