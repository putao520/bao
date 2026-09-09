/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://webaudio.github.io/web-audio-api/#BaseAudioContext
 */

enum AudioContextState {
  "suspended",
  "running",
  "closed"
};

callback DecodeErrorCallback = undefined (DOMException error);
callback DecodeSuccessCallback = undefined (AudioBuffer decodedData);

// (Bao) Exposed=(Window,Worker): base of OfflineAudioContext, whose offline
// rendering is pure software and global-agnostic (REQ-BRW-004 C15, user ruling
// 2026-09-09). Members returning node types that stay Window-only carry
// member-level [Exposed=Window] gates.
[Exposed=(Window,Worker)]
interface BaseAudioContext : EventTarget {
  readonly attribute AudioDestinationNode destination;
  readonly attribute float sampleRate;
  readonly attribute double currentTime;
  // (Bao) AudioListener stays Window-only.
  [Exposed=Window] readonly attribute AudioListener listener;
  readonly attribute AudioContextState  state;
  Promise<undefined> resume();
  attribute EventHandler onstatechange;
  [Throws] AudioBuffer createBuffer(unsigned long numberOfChannels,
                                    unsigned long length,
                                    float sampleRate);
  Promise<AudioBuffer> decodeAudioData(ArrayBuffer audioData,
                                       optional DecodeSuccessCallback successCallback,
                                       optional DecodeErrorCallback errorCallback);
  [Throws] AudioBufferSourceNode createBufferSource();
  // (Bao) The node factories below return node types that remain Window-only
  // (not part of the C15 worker surface), so they are gated member-level.
  [Exposed=Window, Throws] ConstantSourceNode createConstantSource();
  // ScriptProcessorNode createScriptProcessor(optional unsigned long bufferSize = 0,
  //                                           optional unsigned long numberOfInputChannels = 2,
  //                                           optional unsigned long numberOfOutputChannels = 2);
  [Exposed=Window, Throws] AnalyserNode createAnalyser();
  [Throws]  GainNode createGain();
  // DelayNode createDelay(optional double maxDelayTime = 1);
  [Exposed=Window, Throws] BiquadFilterNode createBiquadFilter();
  [Exposed=Window, Throws] IIRFilterNode createIIRFilter(sequence<double> feedforward,
                                sequence<double> feedback);
  // WaveShaperNode createWaveShaper();
  [Exposed=Window, Throws] PannerNode createPanner();
  [Exposed=Window, Throws] StereoPannerNode createStereoPanner();
  // ConvolverNode createConvolver();
  [Exposed=Window, Throws] ChannelSplitterNode createChannelSplitter(optional unsigned long numberOfOutputs = 6);
  [Exposed=Window, Throws] ChannelMergerNode createChannelMerger(optional unsigned long numberOfInputs = 6);
  // DynamicsCompressorNode createDynamicsCompressor();
  [Throws]  OscillatorNode createOscillator();
  // PeriodicWave createPeriodicWave(sequence<float> real,
  //                                 sequence<float> imag,
  //                                 optional PeriodicWaveConstraints constraints);
};
