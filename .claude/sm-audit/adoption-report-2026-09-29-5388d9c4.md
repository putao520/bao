# SM capability adoption report — 2026-09-29-5388d9c4

- inventory: `inventory-2026-09-29-5388d9c4.json`(bao `5388d9c47e26`)
- 扫描面: src/**/*.rs 全树(1533 文件);symbols=1106
- 初判两分类: **native-used = 174** / **unused-pending-verdict = 932**
- low-confidence used(通用名误报风险,裁决前必看 usage_files): 1

> 本报告是 symbol 级事实映射(#30 v1)。capability 裁决(used-native/wrapped/emulated/missing/deliberately-unused)仍由人工账本 `.claude/sm-capability-ledger.json` 持有;unused-pending-verdict ≠ 该弃用。

## 按域汇总

| category_guess | total | native-used | unused-pending-verdict |
|---|---:|---:|---:|
| GC/rooting/heap/weak-refs | 114 | 5 | 109 |
| compile/stencil/module/xdr/cache | 74 | 21 | 53 |
| core-value/object-surface | 656 | 114 | 542 |
| debugger/profiling/memory | 38 | 2 | 36 |
| embedding-hooks/callbacks | 19 | 1 | 18 |
| glue(mozjs-crate) | 46 | 3 | 43 |
| interrupt/cancellation | 7 | 2 | 5 |
| intl/locale/timezone | 7 | 2 | 5 |
| jobs/promise/event-loop | 30 | 11 | 19 |
| principals/security/options | 18 | 0 | 18 |
| runtime/context/realm/compartment/zone | 79 | 11 | 68 |
| shared-memory/atomics | 7 | 0 | 7 |
| structured-clone/serialization | 5 | 2 | 3 |
| wasm | 6 | 0 | 6 |

## Top 40 使用

| symbol | category | binding | count | conf |
|---|---|---|---:|---|
| `JS_DefineFunction` | core-value/object-surface | safe-wrapper(wrappers2) | 887 | high |
| `JS_DefineProperty` | core-value/object-surface | safe-wrapper(wrappers2) | 730 | high |
| `JS_GetProperty` | core-value/object-surface | safe-wrapper(wrappers2) | 594 | high |
| `JS_ReportErrorUTF8` | core-value/object-surface | raw-only | 516 | high |
| `JS_NewStringCopyZ` | core-value/object-surface | safe-wrapper(wrappers2) | 335 | high |
| `JS_NewPlainObject` | core-value/object-surface | safe-wrapper(wrappers2) | 321 | high |
| `JS_ClearPendingException` | core-value/object-surface | safe-wrapper(wrappers2) | 203 | high |
| `CurrentGlobalOrNull` | runtime/context/realm/compartment/zone | safe-wrapper(wrappers2) | 159 | high |
| `Call` | core-value/object-surface | safe-wrapper(wrappers2) | 141 | high |
| `JS_CallFunctionValue` | core-value/object-surface | safe-wrapper(wrappers2) | 140 | high |
| `RunJobs` | jobs/promise/event-loop | safe-wrapper(wrappers2) | 102 | high |
| `JS_GetElement` | core-value/object-surface | safe-wrapper(wrappers2) | 93 | high |
| `JS_DefineProperty3` | core-value/object-surface | safe-wrapper(wrappers2) | 89 | high |
| `Evaluate2` | compile/stencil/module/xdr/cache | safe-wrapper(wrappers2) | 83 | high |
| `NewArrayObject1` | core-value/object-surface | safe-wrapper(wrappers2) | 83 | high |
| `JS_DefineElement` | core-value/object-surface | safe-wrapper(wrappers2) | 80 | high |
| `JS_SetProperty` | core-value/object-surface | safe-wrapper(wrappers2) | 78 | high |
| `Evaluate` | compile/stencil/module/xdr/cache | safe-wrapper(wrappers2) | 70 | high |
| `NewPromiseObject` | jobs/promise/event-loop | safe-wrapper(wrappers2) | 69 | high |
| `Construct` | core-value/object-surface | safe-wrapper(wrappers2) | 67 | high |
| `JS_NewFunction` | core-value/object-surface | safe-wrapper(wrappers2) | 66 | high |
| `JS_GetFunctionObject` | core-value/object-surface | raw-only | 64 | high |
| `ResolvePromise` | jobs/promise/event-loop | safe-wrapper(wrappers2) | 54 | high |
| `Compile` | compile/stencil/module/xdr/cache | safe-wrapper(wrappers2) | 50 | high |
| `JS_NewStringCopyN` | core-value/object-surface | safe-wrapper(wrappers2) | 43 | high |
| `JS_IsExceptionPending` | core-value/object-surface | safe-wrapper(wrappers2) | 39 | high |
| `JS_ObjectIsFunction` | core-value/object-surface | raw-only | 32 | high |
| `JS_GetPendingException` | core-value/object-surface | safe-wrapper(wrappers2) | 31 | high |
| `RejectPromise` | jobs/promise/event-loop | safe-wrapper(wrappers2) | 30 | high |
| `GetPropertyKeys` | core-value/object-surface | safe-wrapper(wrappers2) | 26 | high |
| `JS_NewGlobalObject` | runtime/context/realm/compartment/zone | safe-wrapper(wrappers2) | 26 | high |
| `JS_GetObjectAsUint8Array` | core-value/object-surface | raw-only | 25 | high |
| `JS_NewUint8Array` | core-value/object-surface | safe-wrapper(wrappers2) | 25 | high |
| `JS_CallFunctionName` | core-value/object-surface | safe-wrapper(wrappers2) | 22 | high |
| `JS_DefineProperty1` | core-value/object-surface | safe-wrapper(wrappers2) | 21 | high |
| `JS_SetReservedSlot` | core-value/object-surface | raw-only | 21 | high |
| `JS_WrapObject` | core-value/object-surface | safe-wrapper(wrappers2) | 21 | high |
| `JS_HasProperty` | core-value/object-surface | safe-wrapper(wrappers2) | 19 | high |
| `JS_ShutDown` | runtime/context/realm/compartment/zone | mozjs-layer-reference | 19 | high |
| `JS_GC` | core-value/object-surface | safe-wrapper(wrappers2) | 18 | high |

## 已绑定但 Bao 零使用的 safe-wrapper(adoption-gap 候选,供 #30 裁决)

共 546 项;样例(按域聚合后全表见 adoption JSON):

| symbol | category | stability |
|---|---|---|
| `AbortIncrementalGC` | GC/rooting/heap/weak-refs | public |
| `AddGCNurseryCollectionCallback` | GC/rooting/heap/weak-refs | public |
| `AddPromiseReactions` | jobs/promise/event-loop | public |
| `AddPromiseReactionsIgnoringUnhandledRejection` | jobs/promise/event-loop | public |
| `AddServoSizeOf` | debugger/profiling/memory | public |
| `AddSizeOfTab` | debugger/profiling/memory | public |
| `ArrayBufferClone` | core-value/object-surface | public |
| `ArrayBufferCopyData` | core-value/object-surface | public |
| `AssertSameCompartment` | runtime/context/realm/compartment/zone | friend(jsfriendapi) |
| `AssertSameCompartment1` | runtime/context/realm/compartment/zone | friend(jsfriendapi) |
| `BigIntFromBool` | core-value/object-surface | public |
| `BigIntFromInt64` | core-value/object-surface | public |
| `BigIntToString` | core-value/object-surface | public |
| `BuildStackString` | debugger/profiling/memory | public |
| `CallMethodIfWrapped` | core-value/object-surface | public |
| `CallOriginalPromiseReject` | jobs/promise/event-loop | public |
| `CancelAsyncTasks` | core-value/object-surface | public |
| `CaptureCurrentStack` | debugger/profiling/memory | public |
| `CheckRegExpSyntax` | core-value/object-surface | public |
| `CheckedUnwrapDynamic` | principals/security/options | friend(jsfriendapi) |
| `ClearKeptObjects` | core-value/object-surface | public |
| `ClearRegExpStatics` | core-value/object-surface | public |
| `CommitPendingWrapperPreservations` | principals/security/options | friend(jsfriendapi) |
| `CompileFunction` | compile/stencil/module/xdr/cache | public |
| `CompileFunction1` | compile/stencil/module/xdr/cache | public |

