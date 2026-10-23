/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * The origin of this IDL file is
 * https://w3c.github.io/long-animation-frames/#the-performancelonganimationframetiming-interface
 */

[Exposed=Window]
interface PerformanceLongAnimationFrameTiming : PerformanceEntry {
    readonly attribute DOMHighResTimeStamp renderStart;
    readonly attribute DOMHighResTimeStamp styleAndLayoutStart;
    readonly attribute DOMHighResTimeStamp blockingDuration;
    readonly attribute DOMHighResTimeStamp firstUIEventTimestamp;
    readonly attribute DOMHighResTimeStamp styleDuration;
    readonly attribute DOMHighResTimeStamp layoutDuration;
    readonly attribute unsigned long scriptCount;
    readonly attribute /* FrozenArray<PerformanceScriptTiming> */ any scripts;
    [Default] object toJSON();
};

PerformanceLongAnimationFrameTiming includes PaintTimingMixin;
