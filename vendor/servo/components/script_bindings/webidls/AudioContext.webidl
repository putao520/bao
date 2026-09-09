/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * The origin of this IDL file is
 * https://webaudio.github.io/web-audio-api/#dom-audiocontext
 */

enum AudioContextLatencyCategory {
  "balanced",
  "interactive",
  "playback"
};

dictionary AudioContextOptions {
  (AudioContextLatencyCategory or double) latencyHint = "interactive";
  float sampleRate;
};

dictionary AudioTimestamp {
  double contextTime;
  DOMHighResTimeStamp performanceTime;
};

// (Bao) Exposed=(Window,Worker): REQ-BRW-004 C15 — the real-time AudioContext
// constructor is worker-reachable. Rendering runs on servo-media's own
// AudioRenderThread (sink built on the render thread; the calling thread only
// does channel control + a bounded init handshake), so worker-realm
// construction has no Window dependency. The createMedia* members stay behind
// member-level [Exposed=Window] gates: they reference Window-only types
// (HTMLMediaElement / MediaStream / MediaStreamTrack / Media*Audio*Node).
[Exposed=(Window,Worker)]
interface AudioContext : BaseAudioContext {
  [Throws] constructor(optional AudioContextOptions contextOptions = {});
  readonly attribute double baseLatency;
  readonly attribute double outputLatency;

  AudioTimestamp getOutputTimestamp();

  Promise<undefined> suspend();
  Promise<undefined> close();

  [Exposed=Window, Throws] MediaElementAudioSourceNode createMediaElementSource(HTMLMediaElement mediaElement);
  [Exposed=Window, Throws] MediaStreamAudioSourceNode createMediaStreamSource(MediaStream mediaStream);
  [Exposed=Window, Throws] MediaStreamTrackAudioSourceNode createMediaStreamTrackSource(MediaStreamTrack mediaStreamTrack);
  [Exposed=Window, Throws] MediaStreamAudioDestinationNode createMediaStreamDestination();
};
