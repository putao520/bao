/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://webaudio.github.io/web-audio-api/#OfflineAudioContext
 */

dictionary OfflineAudioContextOptions {
  unsigned long numberOfChannels = 1;
  required unsigned long length;
  required float sampleRate;
};

// (Bao) Exposed=(Window,Worker): offline rendering is pure software (servo-media
// OfflineAudioSink, no audio hardware) and its control plane is global-agnostic;
// upstream pins Window-only only because its constructor is Window-anchored
// (REQ-BRW-004 C15, user ruling 2026-09-09).
[Exposed=(Window,Worker)]
interface OfflineAudioContext : BaseAudioContext {
  [Throws] constructor(OfflineAudioContextOptions contextOptions);
  [Throws] constructor(unsigned long numberOfChannels, unsigned long length, float sampleRate);
  readonly attribute unsigned long length;
  attribute EventHandler oncomplete;

  Promise<AudioBuffer> startRendering();
//  Promise<void> suspend(double suspendTime);
};
