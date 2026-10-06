/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletProcessor
//
// (Bao) Upstream servo: zero AudioWorklet runtime (see AudioWorklet.webidl).
// Authored in-tree per the fork-self-maintenance ruling (user ruling
// 2026-10-05). 段(1) delivers the interface + port face; processor
// instantiation (the scope invokes the registered constructor per render
// block) is the 段(2) servo-media bridge face.
[Exposed=AudioWorklet]
interface AudioWorkletProcessor {
    // (Bao e122) [Throws]: the reentrancy face — a second construction
    // while a node instantiation is in flight is the spec's TypeError.
    [Throws]
    constructor(optional object options);
    readonly attribute MessagePort port;
};
