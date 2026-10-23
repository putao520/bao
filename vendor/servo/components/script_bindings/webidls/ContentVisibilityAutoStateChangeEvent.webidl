/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://drafts.csswg.org/css-contain/#content-visibility-auto-state-change-event
// Bao fork self-build (REQ-BRW-051 C2: css-contain 族 DOM 事件面;上游
// origin/main c8f16b892 与 vendor 648de26fa 均无此类型=fork 自治自研)。
// 渲染联动(元素 skipped 态判定→事件派发)登记为后续最小增面,见
// `ContentVisibilityAutoStateChangeEvent` Rust 侧接线点注释。

dictionary ContentVisibilityAutoStateChangeEventInit : EventInit {
  boolean skipped = false;
};

[Exposed=Window]
interface ContentVisibilityAutoStateChangeEvent : Event {
  constructor(DOMString type, optional ContentVisibilityAutoStateChangeEventInit eventInitDict = {});
  readonly attribute boolean skipped;
};
