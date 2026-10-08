/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://w3c.github.io/edit-context/#dom-editcontext
// Bao fork self-build (REQ-BRW-050): upstream servo has zero implementation of
// the W3C EditContext API (no webidl, 17 ini entries expected-FAIL). Chromium
// 121+ ships it enabled by default, so the absence is a fingerprinting vector.

dictionary EditContextInit {
  DOMString text = "";
  unsigned long selectionStart = 0;
  unsigned long selectionEnd = 0;
};

[Exposed=Window]
interface EditContext : EventTarget {
  constructor(optional EditContextInit options = {});

  undefined updateText(unsigned long rangeStart, unsigned long rangeEnd, DOMString text);
  undefined updateSelection(unsigned long start, unsigned long end);
  undefined updateControlBounds(DOMRect controlBounds);
  undefined updateSelectionBounds(DOMRect selectionBounds);
  undefined updateCharacterBounds(unsigned long rangeStart, sequence<DOMRect> characterBounds);

  sequence<HTMLElement> attachedElements();

  readonly attribute DOMString text;
  readonly attribute unsigned long selectionStart;
  readonly attribute unsigned long selectionEnd;
  readonly attribute unsigned long characterBoundsRangeStart;
  sequence<DOMRect> characterBounds();

  attribute EventHandler ontextupdate;
  attribute EventHandler ontextformatupdate;
  attribute EventHandler oncharacterboundsupdate;
  attribute EventHandler oncompositionstart;
  attribute EventHandler oncompositionend;
};

// https://w3c.github.io/edit-context/#textupdateevent
dictionary TextUpdateEventInit : EventInit {
  unsigned long updateRangeStart = 0;
  unsigned long updateRangeEnd = 0;
  DOMString text = "";
  unsigned long selectionStart = 0;
  unsigned long selectionEnd = 0;
};

[Exposed=Window]
interface TextUpdateEvent : Event {
  constructor(DOMString type, optional TextUpdateEventInit options = {});
  readonly attribute unsigned long updateRangeStart;
  readonly attribute unsigned long updateRangeEnd;
  readonly attribute DOMString text;
  readonly attribute unsigned long selectionStart;
  readonly attribute unsigned long selectionEnd;
};

// https://w3c.github.io/edit-context/#characterboundsupdateevent
dictionary CharacterBoundsUpdateEventInit : EventInit {
  unsigned long rangeStart = 0;
  unsigned long rangeEnd = 0;
};

[Exposed=Window]
interface CharacterBoundsUpdateEvent : Event {
  constructor(DOMString type, optional CharacterBoundsUpdateEventInit options = {});
  readonly attribute unsigned long rangeStart;
  readonly attribute unsigned long rangeEnd;
};
