/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// https://github.com/ricea/websocketstream-explainer

[Exposed=(Window,Worker)]
interface WebSocketStream {
  [Throws] constructor(USVString url, optional WebSocketStreamOptions options = {});

  readonly attribute USVString url;
  readonly attribute Promise<any> opened;
  readonly attribute Promise<any> closed;

  [Throws] undefined close(optional WebSocketCloseInfo closeInfo = {});
};

dictionary WebSocketStreamOptions {
  sequence<DOMString> protocols = [];
  AbortSignal signal;
};

dictionary WebSocketCloseInfo {
  unsigned short closeCode;
  USVString reason = "";
};
