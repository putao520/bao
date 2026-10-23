/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this file,
 * You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * The origin of this IDL file is
 * https://w3c.github.io/long-animation-frames/#the-performancescripttiming-interface
 */

enum ScriptInvokerType {
    "classic-script",
    "module-script",
    "event-listener",
    "user-callback",
    "user-entry-point",
    "resolve-promise",
    "reject-promise"
};

enum ScriptWindowAttribution {
    "self",
    "descendant",
    "ancestor",
    "same-page",
    "other"
};

[Exposed=Window]
interface PerformanceScriptTiming : PerformanceEntry {
    readonly attribute ScriptInvokerType invokerType;
    readonly attribute DOMString invoker;
    readonly attribute DOMHighResTimeStamp executionStart;
    readonly attribute DOMString sourceURL;
    readonly attribute DOMString sourceFunctionName;
    readonly attribute long long sourceCharPosition;
    readonly attribute long long sourceLine;
    readonly attribute long long sourceColumn;
    readonly attribute DOMHighResTimeStamp pauseDuration;
    readonly attribute DOMHighResTimeStamp forcedStyleAndLayoutDuration;
    readonly attribute DOMHighResTimeStamp forcedStyleDuration;
    readonly attribute DOMHighResTimeStamp forcedLayoutDuration;
    readonly attribute Window? window;
    readonly attribute ScriptWindowAttribution windowAttribution;
    [Default] object toJSON();
};
