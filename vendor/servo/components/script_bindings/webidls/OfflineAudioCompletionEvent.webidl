/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
/*
 * For more information on this interface please see
 * https://webaudio.github.io/web-audio-api/#offlineaudiocompletionevent
 */

dictionary OfflineAudioCompletionEventInit : EventInit {
  required AudioBuffer renderedBuffer;
};

// (Bao) Exposed=(Window,Worker): the "complete" event fired when an offline
// rendering finishes must exist in the realm that started it (REQ-BRW-004 C15,
// user ruling 2026-09-09).
[Exposed=(Window,Worker)]
interface OfflineAudioCompletionEvent : Event {
  [Throws] constructor(DOMString type, OfflineAudioCompletionEventInit eventInitDict);
  readonly attribute AudioBuffer renderedBuffer;
};
