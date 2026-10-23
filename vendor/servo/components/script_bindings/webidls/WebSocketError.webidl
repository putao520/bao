/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// https://github.com/ricea/websocketstream-explainer

[Exposed=(Window,Worker)]
interface WebSocketError : DOMException {
  [Throws] constructor(optional DOMString message = "", optional WebSocketErrorInit init = {});

  readonly attribute unsigned short? closeCode;
  readonly attribute USVString reason;
};

dictionary WebSocketErrorInit {
  unsigned short closeCode;
  USVString reason = "";
};
