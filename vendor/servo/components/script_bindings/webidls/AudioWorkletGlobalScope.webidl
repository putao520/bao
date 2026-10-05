/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletGlobalScope
//
// (Bao) Upstream servo: zero AudioWorklet runtime (see AudioWorklet.webidl).
// Authored in-tree per the fork-self-maintenance ruling (user ruling
// 2026-10-05). The scope runs on the worklet engine's own thread(s)
// (WorkletGlobalScopeType::Audio); registerProcessor stores the
// processor constructor in the scope-local registry (the data plane 段(1)
// delivers). currentFrame/currentTime/sampleRate read live values from the
// creating AudioContext's servo-media handle (plumbed through
// WorkletGlobalScopeInit.audio) — suspended contexts report real zeros, not
// fabricated numbers.
// (Bao) Same conditional-exposure carrier as AudioWorklet.webidl: the
// parent WorkletGlobalScope is Pref-gated upstream, and the codegen rule
// rejects an unconditional child of a conditionally-exposed parent.
// Default true (prefs.rs) keeps the realm on unconditionally.
[Global=(Worklet,AudioWorklet), Exposed=AudioWorklet, Pref="dom_audio_worklet_enabled"]
interface AudioWorkletGlobalScope : WorkletGlobalScope {
    [Throws] undefined registerProcessor(DOMString name, VoidFunction processorCtor);
    readonly attribute double currentFrame;
    readonly attribute double currentTime;
    readonly attribute float sampleRate;
};
