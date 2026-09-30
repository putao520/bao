# WPT First Run — Bao (subset, #14-C)

- date: 2026-10-01 · driver: bao browser (headless CDP) + python ws
- suite root: upstream tests/wpt/tests (dom subset), static http server
- manifest: 40 files · PASS: 0 · non-PASS: 40
- KNOWN LIMITATION (w55 stop clause confirmed): pump-timed injection lands
  after synchronously-completing testharness files — those report NO-HARVEST.
  Realm-entry-timed injection (vendor face) is the full fix.

| test file | status | subtests pass/fail |
|---|---|---|
| nodes/Element-hasAttribute.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/event-global-is-still-set-when-coercing-beforeunload-result.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-dispatch-multiple-stopPropagation.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/remove-all-listeners.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-dispatch-order-at-target.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/event-disabled-dynamic.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-dispatch-target-removed.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| ranges/Range-mutations-removeChild.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/CharacterData-insertData.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/Document-createElement-namespace.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/EventListener-incumbent-global-subframe-1.sub.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/webkit-animation-iteration-event.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-dispatch-redispatch.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-defaultPrevented-after-dispatch.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-subclasses-constructors.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-returnValue.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| collections/HTMLCollection-empty-name.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| traversal/NodeIterator-removal.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/label-default-action.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/Node-cloneNode-on-inactive-document-crash.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| ranges/Range-attributes.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/MutationObserver-inner-outer.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/NodeList-static-length-getter-tampered-2.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/CharacterData-deleteData.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-stopImmediatePropagation.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/event-src-element-nullable.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| collections/namednodemap-supported-property-names.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| lists/DOMTokenList-coverage-for-attributes.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/ParentNode-querySelectorAll-removed-elements.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/DOMImplementation-createDocument-with-null-browsing-context-crash.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| ranges/StaticRange-constructor.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| ranges/Range-collapse.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| nodes/Document-createComment.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| ranges/Range-commonAncestorContainer.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/Event-dispatch-bubble-canceled.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| historical.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| traversal/NodeIterator.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| events/EventTarget-dispatchEvent-returnvalue.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| window-extends-event-target.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |
| abort/abort-signal-timeout.html | NO-HARVEST(pump-timing: sync-completing tests finish before injection lands) | -/- |

## 失败/超时明细

### nodes/Element-hasAttribute.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/event-global-is-still-set-when-coercing-beforeunload-result.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-dispatch-multiple-stopPropagation.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/remove-all-listeners.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-dispatch-order-at-target.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/event-disabled-dynamic.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-dispatch-target-removed.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### ranges/Range-mutations-removeChild.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/CharacterData-insertData.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/Document-createElement-namespace.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/EventListener-incumbent-global-subframe-1.sub.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/webkit-animation-iteration-event.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-dispatch-redispatch.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-defaultPrevented-after-dispatch.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-subclasses-constructors.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-returnValue.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### collections/HTMLCollection-empty-name.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### traversal/NodeIterator-removal.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/label-default-action.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/Node-cloneNode-on-inactive-document-crash.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### ranges/Range-attributes.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/MutationObserver-inner-outer.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/NodeList-static-length-getter-tampered-2.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/CharacterData-deleteData.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-stopImmediatePropagation.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/event-src-element-nullable.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### collections/namednodemap-supported-property-names.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### lists/DOMTokenList-coverage-for-attributes.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/ParentNode-querySelectorAll-removed-elements.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/DOMImplementation-createDocument-with-null-browsing-context-crash.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### ranges/StaticRange-constructor.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### ranges/Range-collapse.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### nodes/Document-createComment.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### ranges/Range-commonAncestorContainer.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/Event-dispatch-bubble-canceled.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### historical.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### traversal/NodeIterator.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### events/EventTarget-dispatchEvent-returnvalue.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### window-extends-event-target.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

### abort/abort-signal-timeout.html — NO-HARVEST(pump-timing: sync-completing tests finish before injection lands)

