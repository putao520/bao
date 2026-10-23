/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! MathML DOM interfaces.
//!
//! BAO PATCH (REQ-BRW-051 C2, MathML layout, 2026-10-21): fork-maintained.
//! Before this, every MathML element in this Servo snapshot was created as a
//! bare `Element` (see `element::create::create_element`), so MathML content had
//! no `dataset`, `style`, `tabIndex`, focusability or global event handlers.

pub(crate) mod mathmlelement;

pub(crate) use self::mathmlelement::MathMLElement;
