/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorklet
//
// (Bao) Upstream servo has no AudioWorklet bindings at all (origin/main
// 2026-10-05: zero AudioWorklet hits outside the fetch-destination pipeline
// and WPT expectations). This interface is authored in-tree per the
// fork-self-maintenance ruling (user ruling 2026-10-05, "自研吧"); when
// upstream lands its own bindings (Outreachy roadmap item 2), re-align this
// file against the upstream shape.
//
// (Bao) Carries its own `Pref="dom_audio_worklet_enabled"` defaulting TRUE
// (bao prefs.rs): the codegen rule rejects an unconditional interface
// inheriting a conditionally-exposed one (Worklet is Pref-gated upstream).
// Defaulting true keeps Chromium parity — AudioWorklet is exposed on HTTP
// and HTTPS pages alike — while `window.Worklet` itself stays hidden under
// the upstream `dom_worklet_enabled` gate (that gate is unaffected here:
// interface objects and prototype chains are built pref-free by the
// codegen chain builder, so `addModule` resolves on the inherited
// prototype regardless of it).
[Exposed=Window, Pref="dom_audio_worklet_enabled"]
interface AudioWorklet : Worklet {
};
