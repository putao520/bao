/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://html.spec.whatwg.org/multipage/#messageevent
// (Bao) AudioWorklet added: the AudioWorklet realm receives `message`
// events on MessagePorts (node↔processor port pairs; same shape as the
// MessagePort exposure edit). Upstream has no AudioWorklet realm, so
// upstream parity is unaffected. (e102 回放曾回退此轴——缺轴 =
// worklet 原型无 data 访问器,handler 读 e.data 为 undefined。)
[Exposed=(Window,Worker,AudioWorklet)]
interface MessageEvent : Event {
  [Throws] constructor(DOMString type, optional MessageEventInit eventInitDict = {});
  readonly attribute any data;
  readonly attribute DOMString origin;
  readonly attribute DOMString lastEventId;
  readonly attribute MessageEventSource? source;
  readonly attribute /*FrozenArray<MessagePort>*/any ports;

  undefined initMessageEvent(
    DOMString type,
    optional boolean bubbles = false,
    optional boolean cancelable = false,
    optional any data = null,
    optional DOMString origin = "",
    optional DOMString lastEventId = "",
    optional MessageEventSource? source = null,
    optional sequence<MessagePort> ports = []
  );
};

dictionary MessageEventInit : EventInit {
  any data = null;
  DOMString origin = "";
  DOMString lastEventId = "";
  //DOMString channel;
  MessageEventSource? source = null;
  sequence<MessagePort> ports = [];
};

typedef (WindowProxy or MessagePort or ServiceWorker) MessageEventSource;
