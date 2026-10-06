/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://webaudio.github.io/web-audio-api/#PeriodicWave
 */

dictionary PeriodicWaveConstraints {
    boolean disableNormalization = false;
};

dictionary PeriodicWaveOptions : PeriodicWaveConstraints {
    sequence<float> real;
    sequence<float> imag;
};

// (Bao) Exposed=(Window,Worker): waveform carrier of OscillatorNode — the
// new upstream snapshot activated setPeriodicWave/getPeriodicWave on
// OscillatorNode, which is worker-exposed (REQ-BRW-004 C15 type-visibility
// consequence, user ruling 2026-09-09).
[Exposed=(Window,Worker)]
interface PeriodicWave {
    [Throws] constructor (BaseAudioContext context, optional PeriodicWaveOptions options = {});
};
