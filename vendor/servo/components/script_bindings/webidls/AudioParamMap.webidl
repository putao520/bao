/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioParamMap
//
// (Bao) Upstream servo has no AudioParamMap (no AudioWorklet at all,
// see AudioWorklet.webidl). Authored in-tree per the fork-self-maintenance
// ruling (user ruling 2026-10-05, "自研吧"); the name→AudioParam map of an
// AudioWorkletNode, keyed in the processor's `parameterDescriptors` order.
// When upstream lands its own bindings (Outreachy roadmap item 2), re-align
// this file against the upstream shape.
[Exposed=(Window,AudioWorklet)]
interface AudioParamMap {
  readonly maplike<DOMString, AudioParam>;
};
