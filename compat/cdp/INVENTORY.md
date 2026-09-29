# CDP API INVENTORY(G1-CDP 全量盘点)

> 机械可重建;分类五档:**Supported**(协议有+bao 实现+测试断言)/ **Supported(ack)**(ack 即完整协议语义:enable/订阅/弃用接受)/ **Partial**(实现无测试断言,或 ack-only 无投递)/ **Explicitly-Unsupported**(-32000 裁决面:blackbox 类,显式拒绝非静默)/ **Unsupported**(协议有+bao 未分派,-32601)。
> 本表只立「协议面×实现×测试」事实矩阵,不造通过率。

## 协议版本锁定(协议真源)

- **cdp-protocol 0.3.1**(Cargo.lock 钉版本;registry source;协议面=该 crate 类型面)
- 类型面实测:**54 域 / 662 methods**(`impl Method for` × `const NAME` wire 名逐条提取)+ 事件面(Event serde rename 枚举)
- chromedevtools 官方 JSON:本合同未联网取(离线);**以 crate 0.3.1 为协议版本锚**——crate 即上游 Chrome 协议的 Rust 镜像,版本漂移时以 Cargo.lock bump 为准重跑本表
- bao 实现面:`src/bao_cdp/src/protocol.rs` 21 域分派(170 arms;机械抽取+人工校准)
- 测试佐证:`src/bao_cdp/tests/suite/`(24 文件,`"Domain.method"` 字符串断言面)

## 分派域矩阵(21 域,method 级;含 Console/Inspector 两张未分派域的 method 级全 Unsupported 表——共 23 张表 = 21 分派 + 2 未分派)


### Browser(20 methods:S 0+ack0 / Partial 0+ackonly2 / EU 0 / Unsupported 18)

| method | bao_impl | tests | status |
|---|---|---|---|
| Browser.setPermission | — | — | Unsupported |
| Browser.grantPermissions | — | — | Unsupported |
| Browser.resetPermissions | — | — | Unsupported |
| Browser.setDownloadBehavior | ack(无投递) | — | Partial(ack-only) |
| Browser.cancelDownload | — | — | Unsupported |
| Browser.close | — | — | Unsupported |
| Browser.crash | — | — | Unsupported |
| Browser.crashGpuProcess | — | — | Unsupported |
| Browser.getVersion | ack(无投递) | — | Partial(ack-only) |
| Browser.getBrowserCommandLine | — | — | Unsupported |
| Browser.getHistograms | — | — | Unsupported |
| Browser.getHistogram | — | — | Unsupported |
| Browser.getWindowBounds | — | — | Unsupported |
| Browser.getWindowForTarget | — | — | Unsupported |
| Browser.setWindowBounds | — | — | Unsupported |
| Browser.setContentsSize | — | — | Unsupported |
| Browser.setDockTile | — | — | Unsupported |
| Browser.executeBrowserCommand | — | — | Unsupported |
| Browser.addPrivacySandboxEnrollmentOverride | — | — | Unsupported |
| Browser.addPrivacySandboxCoordinatorKeyConfig | — | — | Unsupported |

### Console(3 methods:S 0+ack0 / Partial 0+ackonly0 / EU 0 / Unsupported 3)

| method | bao_impl | tests | status |
|---|---|---|---|
| Console.clearMessages | — | — | Unsupported |
| Console.disable | — | — | Unsupported |
| Console.enable | — | — | Unsupported |

### CSS(37 methods:S 6+ack0 / Partial 0+ackonly0 / EU 0 / Unsupported 31)

| method | bao_impl | tests | status |
|---|---|---|---|
| CSS.addRule | — | — | Unsupported |
| CSS.collectClassNames | — | — | Unsupported |
| CSS.createStyleSheet | — | — | Unsupported |
| CSS.disable | ✓ | domain_handler_response_field_boundary_tests | Supported |
| CSS.enable | ✓ | backend_bridge_channel_deep_tests.rs,domain_ | Supported |
| CSS.forcePseudoState | — | — | Unsupported |
| CSS.forceStartingStyle | — | — | Unsupported |
| CSS.getBackgroundColors | — | — | Unsupported |
| CSS.getComputedStyleForNode | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| CSS.resolveValues | — | — | Unsupported |
| CSS.getLonghandProperties | — | — | Unsupported |
| CSS.getInlineStylesForNode | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| CSS.getAnimatedStylesForNode | — | — | Unsupported |
| CSS.getMatchedStylesForNode | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| CSS.getEnvironmentVariables | — | — | Unsupported |
| CSS.getMediaQueries | — | — | Unsupported |
| CSS.getPlatformFontsForNode | — | — | Unsupported |
| CSS.getStyleSheetText | — | — | Unsupported |
| CSS.getLayersForNode | — | — | Unsupported |
| CSS.getLocationForSelector | — | — | Unsupported |
| CSS.trackComputedStyleUpdatesForNode | — | — | Unsupported |
| CSS.trackComputedStyleUpdates | — | — | Unsupported |
| CSS.takeComputedStyleUpdates | — | — | Unsupported |
| CSS.setEffectivePropertyValueForNode | — | — | Unsupported |
| CSS.setPropertyRulePropertyName | — | — | Unsupported |
| CSS.setKeyframeKey | — | — | Unsupported |
| CSS.setMediaText | — | — | Unsupported |
| CSS.setContainerQueryText | — | — | Unsupported |
| CSS.setSupportsText | — | — | Unsupported |
| CSS.setScopeText | — | — | Unsupported |
| CSS.setRuleSelector | — | — | Unsupported |
| CSS.setStyleSheetText | — | — | Unsupported |
| CSS.setStyleTexts | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| CSS.startRuleUsageTracking | — | — | Unsupported |
| CSS.stopRuleUsageTracking | — | — | Unsupported |
| CSS.takeCoverageDelta | — | — | Unsupported |
| CSS.setLocalFontsEnabled | — | — | Unsupported |

### Debugger(33 methods:S 12+ack0 / Partial 0+ackonly2 / EU 1 / Unsupported 18)

| method | bao_impl | tests | status |
|---|---|---|---|
| Debugger.continueToLocation | — | — | Unsupported |
| Debugger.disable | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Debugger.enable | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Debugger.evaluateOnCallFrame | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Debugger.getPossibleBreakpoints | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Debugger.getScriptSource | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Debugger.disassembleWasmModule | — | — | Unsupported |
| Debugger.nextWasmDisassemblyChunk | — | — | Unsupported |
| Debugger.getWasmBytecode | — | — | Unsupported |
| Debugger.getStackTrace | — | — | Unsupported |
| Debugger.pause | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Debugger.pauseOnAsyncCall | — | — | Unsupported |
| Debugger.removeBreakpoint | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Debugger.restartFrame | — | — | Unsupported |
| Debugger.resume | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Debugger.searchInContent | — | — | Unsupported |
| Debugger.setAsyncCallStackDepth | — | — | Unsupported |
| Debugger.setBlackboxExecutionContexts | — | — | Unsupported |
| Debugger.setBlackboxPatterns | — | — | Unsupported |
| Debugger.setBlackboxedRanges | — | — | Unsupported |
| Debugger.setBreakpoint | — | — | Unsupported |
| Debugger.setInstrumentationBreakpoint | — | — | Unsupported |
| Debugger.setBreakpointByUrl | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Debugger.setBreakpointOnFunctionCall | — | — | Unsupported |
| Debugger.setBreakpointsActive | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Debugger.setPauseOnExceptions | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,do | Explicitly-Unsupported |
| Debugger.setReturnValue | — | — | Unsupported |
| Debugger.setScriptSource | — | — | Unsupported |
| Debugger.setSkipAllPauses | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Debugger.setVariableValue | — | — | Unsupported |
| Debugger.stepInto | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Debugger.stepOut | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Debugger.stepOver | ✓ | domain_handler_response_field_boundary_tests | Supported |

### DOM(53 methods:S 11+ack0 / Partial 0+ackonly0 / EU 1 / Unsupported 41)

| method | bao_impl | tests | status |
|---|---|---|---|
| DOM.collectClassNamesFromSubtree | — | — | Unsupported |
| DOM.copyTo | — | — | Unsupported |
| DOM.describeNode | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| DOM.scrollIntoViewIfNeeded | — | — | Unsupported |
| DOM.disable | ✓ | domain_handler_response_field_boundary_tests | Supported |
| DOM.discardSearchResults | — | — | Unsupported |
| DOM.enable | ✓ | domain_handler_response_field_boundary_tests | Supported |
| DOM.focus | — | — | Unsupported |
| DOM.getAttributes | — | — | Unsupported |
| DOM.getBoxModel | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| DOM.getContentQuads | — | — | Unsupported |
| DOM.getDocument | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| DOM.getFlattenedDocument | ✓ | devtools_dom_css_tests.rs,protocol_subcomman | Supported |
| DOM.getNodesForSubtreeByStyle | — | — | Unsupported |
| DOM.getNodeForLocation | — | — | Unsupported |
| DOM.getOuterHTML | ✓ | bridge_channel_timeout_edge_deep_tests.rs,pr | Supported |
| DOM.getRelayoutBoundary | — | — | Unsupported |
| DOM.getSearchResults | — | — | Unsupported |
| DOM.hideHighlight | — | — | Unsupported |
| DOM.highlightNode | — | — | Unsupported |
| DOM.highlightRect | — | — | Unsupported |
| DOM.markUndoableState | — | — | Unsupported |
| DOM.moveTo | — | — | Unsupported |
| DOM.performSearch | — | — | Unsupported |
| DOM.pushNodeByPathToFrontend | — | — | Unsupported |
| DOM.pushNodesByBackendIdsToFrontend | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| DOM.querySelector | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| DOM.querySelectorAll | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| DOM.getTopLayerElements | — | — | Unsupported |
| DOM.getElementByRelation | — | — | Unsupported |
| DOM.redo | — | — | Unsupported |
| DOM.removeAttribute | — | domain_handler_response_field_boundary_tests | Unsupported |
| DOM.removeNode | — | domain_handler_response_field_boundary_tests | Unsupported |
| DOM.requestChildNodes | — | — | Unsupported |
| DOM.requestNode | — | — | Unsupported |
| DOM.resolveNode | ✓ | bridge_channel_timeout_edge_deep_tests.rs,de | Supported |
| DOM.setAttributeValue | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,pr | Explicitly-Unsupported |
| DOM.setAttributesAsText | — | — | Unsupported |
| DOM.setFileInputFiles | — | — | Unsupported |
| DOM.setNodeStackTracesEnabled | — | — | Unsupported |
| DOM.getNodeStackTraces | — | — | Unsupported |
| DOM.getFileInfo | — | — | Unsupported |
| DOM.getDetachedDomNodes | — | — | Unsupported |
| DOM.setInspectedNode | — | — | Unsupported |
| DOM.setNodeName | — | — | Unsupported |
| DOM.setNodeValue | — | — | Unsupported |
| DOM.setOuterHTML | — | domain_handler_response_field_boundary_tests | Unsupported |
| DOM.undo | — | — | Unsupported |
| DOM.getFrameOwner | — | — | Unsupported |
| DOM.getContainerForNode | — | — | Unsupported |
| DOM.getQueryingDescendantsForContainer | — | — | Unsupported |
| DOM.getAnchorElement | — | — | Unsupported |
| DOM.forceShowPopover | — | — | Unsupported |

### Emulation(46 methods:S 2+ack1 / Partial 0+ackonly6 / EU 0 / Unsupported 37)

| method | bao_impl | tests | status |
|---|---|---|---|
| Emulation.canEmulate | — | — | Unsupported |
| Emulation.clearDeviceMetricsOverride | ack(完整语义) | bridge_channel_timeout_edge_deep_tests.rs,do | Supported(ack) |
| Emulation.clearGeolocationOverride | — | — | Unsupported |
| Emulation.resetPageScaleFactor | — | — | Unsupported |
| Emulation.setFocusEmulationEnabled | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Emulation.setAutoDarkModeOverride | — | — | Unsupported |
| Emulation.setCPUThrottlingRate | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Emulation.setDefaultBackgroundColorOverride | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Emulation.setSafeAreaInsetsOverride | — | — | Unsupported |
| Emulation.setDeviceMetricsOverride | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Emulation.setDevicePostureOverride | — | — | Unsupported |
| Emulation.clearDevicePostureOverride | — | — | Unsupported |
| Emulation.setDisplayFeaturesOverride | — | — | Unsupported |
| Emulation.clearDisplayFeaturesOverride | — | — | Unsupported |
| Emulation.setScrollbarsHidden | — | — | Unsupported |
| Emulation.setDocumentCookieDisabled | — | — | Unsupported |
| Emulation.setEmitTouchEventsForMouse | — | — | Unsupported |
| Emulation.setEmulatedMedia | ack(无投递) | — | Partial(ack-only) |
| Emulation.setEmulatedVisionDeficiency | — | — | Unsupported |
| Emulation.setEmulatedOSTextScale | — | — | Unsupported |
| Emulation.setGeolocationOverride | — | — | Unsupported |
| Emulation.getOverriddenSensorInformation | — | — | Unsupported |
| Emulation.setSensorOverrideEnabled | — | — | Unsupported |
| Emulation.setSensorOverrideReadings | — | — | Unsupported |
| Emulation.setPressureSourceOverrideEnabled | — | — | Unsupported |
| Emulation.setPressureStateOverride | — | — | Unsupported |
| Emulation.setPressureDataOverride | — | — | Unsupported |
| Emulation.setIdleOverride | — | — | Unsupported |
| Emulation.clearIdleOverride | — | — | Unsupported |
| Emulation.setNavigatorOverrides | — | — | Unsupported |
| Emulation.setPageScaleFactor | — | — | Unsupported |
| Emulation.setScriptExecutionDisabled | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Emulation.setTouchEmulationEnabled | ack(无投递) | domain_handler_response_field_boundary_tests | Partial(ack-only) |
| Emulation.setVirtualTimePolicy | — | — | Unsupported |
| Emulation.setLocaleOverride | — | — | Unsupported |
| Emulation.setTimezoneOverride | — | — | Unsupported |
| Emulation.setVisibleSize | — | — | Unsupported |
| Emulation.setDisabledImageTypes | — | — | Unsupported |
| Emulation.setDataSaverOverride | — | — | Unsupported |
| Emulation.setHardwareConcurrencyOverride | — | — | Unsupported |
| Emulation.setUserAgentOverride | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Emulation.setAutomationOverride | — | — | Unsupported |
| Emulation.setSmallViewportHeightDifferenceOverride | — | — | Unsupported |
| Emulation.getScreenInfos | — | — | Unsupported |
| Emulation.addScreen | — | — | Unsupported |
| Emulation.removeScreen | — | — | Unsupported |

### Fetch(9 methods:S 0+ack0 / Partial 0+ackonly0 / EU 1 / Unsupported 8)

| method | bao_impl | tests | status |
|---|---|---|---|
| Fetch.disable | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,do | Explicitly-Unsupported |
| Fetch.enable | — | backend_bridge_channel_deep_tests.rs,bridge_ | Unsupported |
| Fetch.failRequest | — | backend_bridge_channel_deep_tests.rs,bridge_ | Unsupported |
| Fetch.fulfillRequest | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |
| Fetch.continueRequest | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |
| Fetch.continueWithAuth | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |
| Fetch.continueResponse | — | — | Unsupported |
| Fetch.getResponseBody | — | — | Unsupported |
| Fetch.takeResponseBodyAsStream | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |

### HeapProfiler(12 methods:S 0+ack2 / Partial 4+ackonly0 / EU 0 / Unsupported 6)

| method | bao_impl | tests | status |
|---|---|---|---|
| HeapProfiler.addInspectedHeapObject | — | — | Unsupported |
| HeapProfiler.collectGarbage | ✓(无测试断言) | — | Partial |
| HeapProfiler.disable | ack(完整语义) | — | Supported(ack) |
| HeapProfiler.enable | ack(完整语义) | — | Supported(ack) |
| HeapProfiler.getHeapObjectId | — | — | Unsupported |
| HeapProfiler.getObjectByHeapObjectId | — | — | Unsupported |
| HeapProfiler.getSamplingProfile | — | — | Unsupported |
| HeapProfiler.startSampling | — | — | Unsupported |
| HeapProfiler.startTrackingHeapObjects | ✓(无测试断言) | — | Partial |
| HeapProfiler.stopSampling | — | — | Unsupported |
| HeapProfiler.stopTrackingHeapObjects | ✓(无测试断言) | — | Partial |
| HeapProfiler.takeHeapSnapshot | ✓(无测试断言) | — | Partial |

### Input(13 methods:S 3+ack2 / Partial 0+ackonly0 / EU 1 / Unsupported 7)

| method | bao_impl | tests | status |
|---|---|---|---|
| Input.dispatchDragEvent | — | — | Unsupported |
| Input.dispatchKeyEvent | ✓ | bridge_channel_timeout_edge_deep_tests.rs,pr | Supported |
| Input.insertText | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Input.imeSetComposition | — | — | Unsupported |
| Input.dispatchMouseEvent | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Input.dispatchTouchEvent | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,do | Explicitly-Unsupported |
| Input.cancelDragging | — | — | Unsupported |
| Input.emulateTouchFromMouseEvent | — | — | Unsupported |
| Input.setIgnoreInputEvents | ack(完整语义) | domain_handler_response_field_boundary_tests | Supported(ack) |
| Input.setInterceptDrags | ack(完整语义) | domain_handler_response_field_boundary_tests | Supported(ack) |
| Input.synthesizePinchGesture | — | — | Unsupported |
| Input.synthesizeScrollGesture | — | — | Unsupported |
| Input.synthesizeTapGesture | — | — | Unsupported |

### Inspector(2 methods:S 0+ack0 / Partial 0+ackonly0 / EU 0 / Unsupported 2)

| method | bao_impl | tests | status |
|---|---|---|---|
| Inspector.disable | — | — | Unsupported |
| Inspector.enable | — | — | Unsupported |

### Log(5 methods:S 0+ack0 / Partial 0+ackonly2 / EU 0 / Unsupported 3)

| method | bao_impl | tests | status |
|---|---|---|---|
| Log.clear | — | backend_bridge_channel_deep_tests.rs,bridge_ | Unsupported |
| Log.disable | — | domain_handler_response_field_boundary_tests | Unsupported |
| Log.enable | — | backend_bridge_channel_deep_tests.rs,domain_ | Unsupported |
| Log.startViolationsReport | ack(无投递) | bridge_channel_timeout_edge_deep_tests.rs,do | Partial(ack-only) |
| Log.stopViolationsReport | ack(无投递) | bridge_channel_timeout_edge_deep_tests.rs,do | Partial(ack-only) |

### Memory(11 methods:S 0+ack0 / Partial 2+ackonly1 / EU 0 / Unsupported 8)

| method | bao_impl | tests | status |
|---|---|---|---|
| Memory.getDOMCounters | ✓(无测试断言) | — | Partial |
| Memory.getDOMCountersForLeakDetection | — | — | Unsupported |
| Memory.prepareForLeakDetection | ack(无投递) | — | Partial(ack-only) |
| Memory.forciblyPurgeJavaScriptMemory | ✓(无测试断言) | — | Partial |
| Memory.setPressureNotificationsSuppressed | — | — | Unsupported |
| Memory.simulatePressureNotification | — | — | Unsupported |
| Memory.startSampling | — | — | Unsupported |
| Memory.stopSampling | — | — | Unsupported |
| Memory.getAllTimeSamplingProfile | — | — | Unsupported |
| Memory.getBrowserSamplingProfile | — | — | Unsupported |
| Memory.getSamplingProfile | — | — | Unsupported |

### Network(40 methods:S 14+ack2 / Partial 0+ackonly3 / EU 18 / Unsupported 3)

| method | bao_impl | tests | status |
|---|---|---|---|
| Network.setAcceptedEncodings | ack(完整语义) | network_schema_conformance_tests.rs | Supported(ack) |
| Network.clearAcceptedEncodingsOverride | ack(完整语义) | network_schema_conformance_tests.rs | Supported(ack) |
| Network.canClearBrowserCache | ack(无投递) | network_schema_conformance_tests.rs | Partial(ack-only) |
| Network.canClearBrowserCookies | ack(无投递) | network_schema_conformance_tests.rs | Partial(ack-only) |
| Network.canEmulateNetworkConditions | ack(无投递) | network_schema_conformance_tests.rs | Partial(ack-only) |
| Network.clearBrowserCache | ✓ | network_schema_conformance_tests.rs | Supported |
| Network.clearBrowserCookies | ✓ | network_schema_conformance_tests.rs | Supported |
| Network.continueInterceptedRequest | — | network_schema_conformance_tests.rs,protocol | Unsupported |
| Network.deleteCookies | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Network.disable | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Network.emulateNetworkConditions | — | domain_handler_response_field_boundary_tests | Unsupported |
| Network.emulateNetworkConditionsByRule | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.overrideNetworkState | ✓ | network_schema_conformance_tests.rs | Supported |
| Network.enable | ✓ | backend_bridge_channel_deep_tests.rs,domain_ | Supported |
| Network.configureDurableMessages | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.getAllCookies | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Network.getCertificate | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.getCookies | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Network.getResponseBody | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Network.getRequestPostData | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.getResponseBodyForInterception | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.takeResponseBodyForInterceptionAsStream | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.replayXHR | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.searchInResponseBody | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.setBlockedURLs | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.setBypassServiceWorker | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.setCacheDisabled | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Network.setCookie | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Network.setCookies | ✓ | network_schema_conformance_tests.rs | Supported |
| Network.setExtraHTTPHeaders | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Network.setAttachDebugStack | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.setRequestInterception | — | network_schema_conformance_tests.rs,protocol | Unsupported |
| Network.setUserAgentOverride | ✓ | network_schema_conformance_tests.rs | Supported |
| Network.streamResourceContent | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.getSecurityIsolationStatus | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.enableReportingApi | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.enableDeviceBoundSessions | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.fetchSchemefulSite | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.loadNetworkResource | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |
| Network.setCookieControls | not_supported(-32000 裁决) | network_schema_conformance_tests.rs | Explicitly-Unsupported |

### Overlay(30 methods:S 0+ack2 / Partial 0+ackonly1 / EU 0 / Unsupported 27)

| method | bao_impl | tests | status |
|---|---|---|---|
| Overlay.disable | ack(完整语义) | domain_handler_response_field_boundary_tests | Supported(ack) |
| Overlay.enable | ack(完整语义) | backend_bridge_channel_deep_tests.rs,domain_ | Supported(ack) |
| Overlay.getHighlightObjectForTest | — | — | Unsupported |
| Overlay.getGridHighlightObjectsForTest | — | — | Unsupported |
| Overlay.getSourceOrderHighlightObjectForTest | — | — | Unsupported |
| Overlay.hideHighlight | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |
| Overlay.highlightFrame | — | — | Unsupported |
| Overlay.highlightNode | — | backend_bridge_channel_deep_tests.rs,bridge_ | Unsupported |
| Overlay.highlightQuad | — | — | Unsupported |
| Overlay.highlightRect | — | — | Unsupported |
| Overlay.highlightSourceOrder | — | — | Unsupported |
| Overlay.setInspectMode | — | bridge_channel_timeout_edge_deep_tests.rs,do | Unsupported |
| Overlay.setShowAdHighlights | — | — | Unsupported |
| Overlay.setPausedInDebuggerMessage | ack(无投递) | bridge_channel_timeout_edge_deep_tests.rs,do | Partial(ack-only) |
| Overlay.setShowDebugBorders | — | — | Unsupported |
| Overlay.setShowFPSCounter | — | — | Unsupported |
| Overlay.setShowGridOverlays | — | — | Unsupported |
| Overlay.setShowFlexOverlays | — | — | Unsupported |
| Overlay.setShowScrollSnapOverlays | — | — | Unsupported |
| Overlay.setShowContainerQueryOverlays | — | — | Unsupported |
| Overlay.setShowInspectedElementAnchor | — | — | Unsupported |
| Overlay.setShowPaintRects | — | — | Unsupported |
| Overlay.setShowLayoutShiftRegions | — | — | Unsupported |
| Overlay.setShowScrollBottleneckRects | — | — | Unsupported |
| Overlay.setShowHitTestBorders | — | — | Unsupported |
| Overlay.setShowWebVitals | — | — | Unsupported |
| Overlay.setShowViewportSizeOnResize | — | — | Unsupported |
| Overlay.setShowHinge | — | — | Unsupported |
| Overlay.setShowIsolatedElements | — | — | Unsupported |
| Overlay.setShowWindowControlsOverlay | — | — | Unsupported |

### Page(61 methods:S 8+ack3 / Partial 0+ackonly0 / EU 2 / Unsupported 48)

| method | bao_impl | tests | status |
|---|---|---|---|
| Page.addScriptToEvaluateOnLoad | — | — | Unsupported |
| Page.addScriptToEvaluateOnNewDocument | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Page.bringToFront | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Page.captureScreenshot | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Page.captureSnapshot | — | — | Unsupported |
| Page.clearDeviceMetricsOverride | — | — | Unsupported |
| Page.clearDeviceOrientationOverride | — | — | Unsupported |
| Page.clearGeolocationOverride | — | — | Unsupported |
| Page.createIsolatedWorld | — | — | Unsupported |
| Page.deleteCookie | — | — | Unsupported |
| Page.disable | ack(完整语义) | backend_bridge_channel_deep_tests.rs,domain_ | Supported(ack) |
| Page.enable | ack(完整语义) | backend_bridge_channel_deep_tests.rs,bridge_ | Supported(ack) |
| Page.getAppManifest | — | — | Unsupported |
| Page.getInstallabilityErrors | — | — | Unsupported |
| Page.getManifestIcons | — | — | Unsupported |
| Page.getAppId | — | — | Unsupported |
| Page.getAdScriptAncestry | — | — | Unsupported |
| Page.getFrameTree | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Page.getLayoutMetrics | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Page.getNavigationHistory | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,pr | Explicitly-Unsupported |
| Page.resetNavigationHistory | — | — | Unsupported |
| Page.getResourceContent | — | — | Unsupported |
| Page.getResourceTree | — | — | Unsupported |
| Page.handleJavaScriptDialog | — | — | Unsupported |
| Page.navigate | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Page.navigateToHistoryEntry | — | — | Unsupported |
| Page.printToPDF | — | — | Unsupported |
| Page.reload | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Page.removeScriptToEvaluateOnLoad | — | — | Unsupported |
| Page.removeScriptToEvaluateOnNewDocument | not_supported(-32000 裁决) | bridge_channel_timeout_edge_deep_tests.rs,do | Explicitly-Unsupported |
| Page.screencastFrameAck | — | — | Unsupported |
| Page.searchInResource | — | — | Unsupported |
| Page.setAdBlockingEnabled | — | — | Unsupported |
| Page.setBypassCSP | — | — | Unsupported |
| Page.getPermissionsPolicyState | — | — | Unsupported |
| Page.getOriginTrials | — | — | Unsupported |
| Page.setDeviceMetricsOverride | — | — | Unsupported |
| Page.setDeviceOrientationOverride | — | — | Unsupported |
| Page.setFontFamilies | — | — | Unsupported |
| Page.setFontSizes | — | — | Unsupported |
| Page.setDocumentContent | — | — | Unsupported |
| Page.setDownloadBehavior | — | — | Unsupported |
| Page.setGeolocationOverride | — | — | Unsupported |
| Page.setLifecycleEventsEnabled | ack(完整语义) | — | Supported(ack) |
| Page.setTouchEmulationEnabled | — | — | Unsupported |
| Page.startScreencast | — | — | Unsupported |
| Page.stopLoading | — | — | Unsupported |
| Page.crash | — | — | Unsupported |
| Page.close | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Page.setWebLifecycleState | — | — | Unsupported |
| Page.stopScreencast | — | — | Unsupported |
| Page.produceCompilationCache | — | — | Unsupported |
| Page.addCompilationCache | — | — | Unsupported |
| Page.clearCompilationCache | — | — | Unsupported |
| Page.setSPCTransactionMode | — | — | Unsupported |
| Page.setRPHRegistrationMode | — | — | Unsupported |
| Page.generateTestReport | — | — | Unsupported |
| Page.waitForDebugger | — | — | Unsupported |
| Page.setInterceptFileChooserDialog | — | — | Unsupported |
| Page.setPrerenderingAllowed | — | — | Unsupported |
| Page.getAnnotatedPageContent | — | — | Unsupported |

### Performance(4 methods:S 0+ack0 / Partial 1+ackonly2 / EU 0 / Unsupported 1)

| method | bao_impl | tests | status |
|---|---|---|---|
| Performance.disable | ack(无投递) | — | Partial(ack-only) |
| Performance.enable | ack(无投递) | — | Partial(ack-only) |
| Performance.setTimeDomain | — | — | Unsupported |
| Performance.getMetrics | ✓(无测试断言) | — | Partial |

### Profiler(9 methods:S 0+ack2 / Partial 3+ackonly0 / EU 0 / Unsupported 4)

| method | bao_impl | tests | status |
|---|---|---|---|
| Profiler.disable | ack(完整语义) | — | Supported(ack) |
| Profiler.enable | ack(完整语义) | — | Supported(ack) |
| Profiler.getBestEffortCoverage | — | — | Unsupported |
| Profiler.setSamplingInterval | ✓(无测试断言) | — | Partial |
| Profiler.start | ✓(无测试断言) | — | Partial |
| Profiler.startPreciseCoverage | — | — | Unsupported |
| Profiler.stop | ✓(无测试断言) | — | Partial |
| Profiler.stopPreciseCoverage | — | — | Unsupported |
| Profiler.takePreciseCoverage | — | — | Unsupported |

### Runtime(23 methods:S 6+ack4 / Partial 0+ackonly0 / EU 0 / Unsupported 13)

| method | bao_impl | tests | status |
|---|---|---|---|
| Runtime.awaitPromise | — | — | Unsupported |
| Runtime.callFunctionOn | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Runtime.compileScript | ack(完整语义) | domain_handler_response_field_boundary_tests | Supported(ack) |
| Runtime.disable | ack(完整语义) | bridge_channel_timeout_edge_deep_tests.rs,do | Supported(ack) |
| Runtime.discardConsoleEntries | — | — | Unsupported |
| Runtime.enable | ack(完整语义) | backend_bridge_channel_deep_tests.rs,bridge_ | Supported(ack) |
| Runtime.evaluate | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Runtime.getIsolateId | — | — | Unsupported |
| Runtime.getHeapUsage | — | — | Unsupported |
| Runtime.getProperties | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Runtime.globalLexicalScopeNames | — | — | Unsupported |
| Runtime.queryObjects | — | — | Unsupported |
| Runtime.releaseObject | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Runtime.releaseObjectGroup | ✓ | domain_handler_response_field_boundary_tests | Supported |
| Runtime.runIfWaitingForDebugger | ack(完整语义) | — | Supported(ack) |
| Runtime.runScript | ✓ | bridge_channel_timeout_edge_deep_tests.rs,do | Supported |
| Runtime.setAsyncCallStackDepth | — | — | Unsupported |
| Runtime.setCustomObjectFormatterEnabled | — | — | Unsupported |
| Runtime.setMaxCallStackSizeToCapture | — | — | Unsupported |
| Runtime.terminateExecution | — | — | Unsupported |
| Runtime.addBinding | — | — | Unsupported |
| Runtime.removeBinding | — | — | Unsupported |
| Runtime.getExceptionDetails | — | — | Unsupported |

### Security(5 methods:S 0+ack0 / Partial 3+ackonly1 / EU 0 / Unsupported 1)

| method | bao_impl | tests | status |
|---|---|---|---|
| Security.disable | ✓(无测试断言) | — | Partial |
| Security.enable | ✓(无测试断言) | — | Partial |
| Security.setIgnoreCertificateErrors | — | — | Unsupported |
| Security.handleCertificateError | ack(无投递) | — | Partial(ack-only) |
| Security.setOverrideCertificateErrors | ✓(无测试断言) | — | Partial |

### ServiceWorker(12 methods:S 0+ack2 / Partial 2+ackonly4 / EU 0 / Unsupported 4)

| method | bao_impl | tests | status |
|---|---|---|---|
| ServiceWorker.deliverPushMessage | ack(无投递) | — | Partial(ack-only) |
| ServiceWorker.disable | ack(完整语义) | — | Supported(ack) |
| ServiceWorker.dispatchSyncEvent | ack(无投递) | — | Partial(ack-only) |
| ServiceWorker.dispatchPeriodicSyncEvent | ack(无投递) | — | Partial(ack-only) |
| ServiceWorker.enable | ack(完整语义) | — | Supported(ack) |
| ServiceWorker.setForceUpdateOnPageLoad | — | — | Unsupported |
| ServiceWorker.skipWaiting | — | — | Unsupported |
| ServiceWorker.startWorker | — | — | Unsupported |
| ServiceWorker.stopAllWorkers | — | — | Unsupported |
| ServiceWorker.stopWorker | ✓(无测试断言) | — | Partial |
| ServiceWorker.unregister | ✓(无测试断言) | — | Partial |
| ServiceWorker.updateRegistration | ack(无投递) | — | Partial(ack-only) |

### Storage(38 methods:S 0+ack0 / Partial 2+ackonly0 / EU 0 / Unsupported 36)

| method | bao_impl | tests | status |
|---|---|---|---|
| Storage.getStorageKeyForFrame | — | — | Unsupported |
| Storage.getStorageKey | — | — | Unsupported |
| Storage.clearDataForOrigin | ✓(无测试断言) | — | Partial |
| Storage.clearDataForStorageKey | — | — | Unsupported |
| Storage.getCookies | ✓(无测试断言) | — | Partial |
| Storage.setCookies | — | — | Unsupported |
| Storage.clearCookies | — | — | Unsupported |
| Storage.getUsageAndQuota | — | — | Unsupported |
| Storage.overrideQuotaForOrigin | — | — | Unsupported |
| Storage.trackCacheStorageForOrigin | — | — | Unsupported |
| Storage.trackCacheStorageForStorageKey | — | — | Unsupported |
| Storage.trackIndexedDBForOrigin | — | — | Unsupported |
| Storage.trackIndexedDBForStorageKey | — | — | Unsupported |
| Storage.untrackCacheStorageForOrigin | — | — | Unsupported |
| Storage.untrackCacheStorageForStorageKey | — | — | Unsupported |
| Storage.untrackIndexedDBForOrigin | — | — | Unsupported |
| Storage.untrackIndexedDBForStorageKey | — | — | Unsupported |
| Storage.getTrustTokens | — | — | Unsupported |
| Storage.clearTrustTokens | — | — | Unsupported |
| Storage.getInterestGroupDetails | — | — | Unsupported |
| Storage.setInterestGroupTracking | — | — | Unsupported |
| Storage.setInterestGroupAuctionTracking | — | — | Unsupported |
| Storage.getSharedStorageMetadata | — | — | Unsupported |
| Storage.getSharedStorageEntries | — | — | Unsupported |
| Storage.setSharedStorageEntry | — | — | Unsupported |
| Storage.deleteSharedStorageEntry | — | — | Unsupported |
| Storage.clearSharedStorageEntries | — | — | Unsupported |
| Storage.resetSharedStorageBudget | — | — | Unsupported |
| Storage.setSharedStorageTracking | — | — | Unsupported |
| Storage.setStorageBucketTracking | — | — | Unsupported |
| Storage.deleteStorageBucket | — | — | Unsupported |
| Storage.runBounceTrackingMitigations | — | — | Unsupported |
| Storage.setAttributionReportingLocalTestingMode | — | — | Unsupported |
| Storage.setAttributionReportingTracking | — | — | Unsupported |
| Storage.sendPendingAttributionReports | — | — | Unsupported |
| Storage.getRelatedWebsiteSets | — | — | Unsupported |
| Storage.getAffectedUrlsForThirdPartyCookieMetadata | — | — | Unsupported |
| Storage.setProtectedAudienceKAnonymity | — | — | Unsupported |

### SystemInfo(3 methods:S 0+ack0 / Partial 2+ackonly0 / EU 0 / Unsupported 1)

| method | bao_impl | tests | status |
|---|---|---|---|
| SystemInfo.getInfo | ✓(无测试断言) | — | Partial |
| SystemInfo.getFeatureState | — | — | Unsupported |
| SystemInfo.getProcessInfo | ✓(无测试断言) | — | Partial |

### Target(19 methods:S 4+ack2 / Partial 0+ackonly0 / EU 3 / Unsupported 10)

| method | bao_impl | tests | status |
|---|---|---|---|
| Target.activateTarget | — | — | Unsupported |
| Target.attachToTarget | not_supported(-32000 裁决) | backend_bridge_channel_deep_tests.rs,bridge_ | Explicitly-Unsupported |
| Target.attachToBrowserTarget | — | — | Unsupported |
| Target.closeTarget | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Target.exposeDevToolsProtocol | — | — | Unsupported |
| Target.createBrowserContext | — | — | Unsupported |
| Target.getBrowserContexts | — | — | Unsupported |
| Target.createTarget | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Target.detachFromTarget | not_supported(-32000 裁决) | protocol_all_domains_internal_backend_tests. | Explicitly-Unsupported |
| Target.disposeBrowserContext | — | — | Unsupported |
| Target.getTargetInfo | ✓ | bridge_channel_timeout_edge_deep_tests.rs,pr | Supported |
| Target.getTargets | ✓ | backend_bridge_channel_deep_tests.rs,bridge_ | Supported |
| Target.sendMessageToTarget | not_supported(-32000 裁决) | protocol_all_domains_internal_backend_tests. | Explicitly-Unsupported |
| Target.setAutoAttach | ack(完整语义) | backend_bridge_channel_deep_tests.rs,protoco | Supported(ack) |
| Target.autoAttachRelated | — | — | Unsupported |
| Target.setDiscoverTargets | ack(完整语义) | protocol_all_domains_internal_backend_tests. | Supported(ack) |
| Target.setRemoteLocations | — | — | Unsupported |
| Target.getDevToolsTarget | — | — | Unsupported |
| Target.openDevTools | — | — | Unsupported |

## 非分派域(31 域,crate 协议面有、bao 无分派 → 域级 Unsupported)

| domain | methods | status |
|---|---:|---|
| accessibility | 8 | Unsupported(域级未分派) |
| animation | 10 | Unsupported(域级未分派) |
| audits | 5 | Unsupported(域级未分派) |
| autofill | 4 | Unsupported(域级未分派) |
| background_service | 4 | Unsupported(域级未分派) |
| bluetooth_emulation | 15 | Unsupported(域级未分派) |
| cache_storage | 5 | Unsupported(域级未分派) |
| cast | 6 | Unsupported(域级未分派) |
| device_access | 4 | Unsupported(域级未分派) |
| device_orientation | 2 | Unsupported(域级未分派) |
| dom_debugger | 10 | Unsupported(域级未分派) |
| dom_snapshot | 4 | Unsupported(域级未分派) |
| dom_storage | 6 | Unsupported(域级未分派) |
| event_breakpoints | 3 | Unsupported(域级未分派) |
| extensions | 7 | Unsupported(域级未分派) |
| fed_cm | 7 | Unsupported(域级未分派) |
| file_system | 1 | Unsupported(域级未分派) |
| headless_experimental | 3 | Unsupported(域级未分派) |
| indexed_db | 9 | Unsupported(域级未分派) |
| io | 3 | Unsupported(域级未分派) |
| layer_tree | 9 | Unsupported(域级未分派) |
| media | 2 | Unsupported(域级未分派) |
| performance_timeline | 1 | Unsupported(域级未分派) |
| preload | 2 | Unsupported(域级未分派) |
| pwa | 7 | Unsupported(域级未分派) |
| schema | 1 | Unsupported(域级未分派) |
| smart_card_emulation | 12 | Unsupported(域级未分派) |
| tethering | 2 | Unsupported(域级未分派) |
| tracing | 6 | Unsupported(域级未分派) |
| web_audio | 3 | Unsupported(域级未分派) |
| web_authn | 13 | Unsupported(域级未分派) |
| **合计** | **174** |  |

> 覆盖对账:21 分派域 methods 662 + 非分派域 174 = 662(crate 全量,零遗漏)。
> 非分派域多为 DevTools 专项(panel 专属:Accessibility/Animation/Overlay 深面/CSS 深面/DOMDebugger/DOMSnapshot…)与
> 安装器/预加载域(Install/Pwa/FedCm…)——是否纳入产品面属产品决策(非义务),本表只立事实。


## Debugger 域 Partial/Unsupported 缺口清单(#11-B,09-27 保真波后现状)

**Supported 12**(enable/disable、setBreakpointByUrl/removeBreakpoint、pause/resume/stepOver/stepInto/stepOut、evaluateOnCallFrame、getPossibleBreakpoints、getScriptSource)——SM Debugger 原生面(裁决 2)。

**Partial(ack-only)2**:
- `Debugger.setBreakpointsActive` — ack(无投递;SM 无该开关原生面)
- `Debugger.setSkipAllPauses` — ack(同上)

**Explicitly-Unsupported 1**:
- `Debugger.setPauseOnExceptions` — -32000 裁决(SM pause-on-exceptions 经 interrupt 桥未接,显式拒绝)

**Unsupported 18**(协议有、bao 未分派):
- wasm 族 ×4:disassembleWasmModule / nextWasmDisassemblyChunk / getWasmBytecode / (wasm 断点)
- 异步栈 ×3:getStackTrace / pauseOnAsyncCall / setAsyncCallStackDepth
- blackbox 族 ×3:setBlackboxExecutionContexts / setBlackboxPatterns / setBlackboxedRanges(SM 无原生 blackbox——与 setPauseOnExceptions 同族,可裁决转 Explicitly-Unsupported)
- 逐帧/值修改 ×4:restartFrame / setReturnValue / setVariableValue / setScriptSource(活编辑,需 engine 侧重编译面)
- 其余 ×4:continueToLocation / setInstrumentationBreakpoint / setBreakpointOnFunctionCall / searchInContent / setBreakpoint(按位置逐点断点;bao 仅有 setBreakpointByUrl)

**后续波立清单建议**:①blackbox 3 项 → Explicitly-Unsupported 裁决(与 blackbox 显式拒绝先例一致);②wasm 4 项 → Explicitly-Unsupported(SM wasm 调试面未接);③setScriptSource/setReturnValue/restartFrame/setVariableValue → 行为测试+实现或显式裁决(产品决策);④continueToLocation/setBreakpoint(位置断点)→ 可由现有 setBreakpointByUrl 语义扩展(工程项)。


## 生命周期/并发边界矩阵(#11-C/#11-F 数据基础)

| 边界项 | 上游语义 | bao 现状 | 测试在否 | 文件 |
|---|---|---|---|---|
| Runtime.executionContextCreated / cleared 事件面 | 每执行上下文创建/销毁通知 | executionContextsCleared 语义在 Runtime.enable 响应契约中明示(navigation 后 registry 失效以事件宣告);Created 事件面=ctx 计数面 | 部分(e2e 文档性断言) | cdp_ws_command_face_tests.rs(object-protocol 注释锚) |
| objectId 跨 document 失效 | navigation 后旧 objectId 失效(Execution context destroyed) | registry 随 document 销毁;失效 objectId → 显式错误(非复活) | ✓ 文档性断言("objectIds surviving navigation is not a CDP promise") | cdp_ws_command_face_tests.rs(object_protocol_phase) |
| 双 transport parity(memory:// ≡ WS) | 同一 CDP 语义经两个 transport 结果一致 | 内部 backend(memory 面,router/protocol 测试)与 WS 面(cdp_ws_command_face_tests)分别覆盖;**无逐命令 parity 对照测试**(parity=间接:双侧各自 suite 绿 + 同一 handle_command 分派) | 部分(间接) | internal backend:protocol_*_tests 群;WS:bao_browser cdp_ws_command_face_tests.rs |
| 事件广播(EventBroadcaster) | 事件经 broadcaster 送订阅 session | broadcaster 面有专测;事件面(Network/Debugger/Page 生命周期)经 translate 桥 | ✓(broadcaster deep tests) | protocol_broadcaster_deep_tests.rs, bao_cdp_client bridge translate |

## 事件面(上游 Event 枚举 vs bao 广播面)

- 上游事件面:Event serde rename 枚举全量(数百事件,54 域)
- bao 广播面(实测):Network.requestWillBeSent/responseReceived、Debugger.paused/scriptParsed、Page 生命周期(loadEventFired/frameNavigated)、Runtime executionContextsCleared、Console entry 族——经 ServoEvent translate + EventBroadcaster(见上矩阵)
- 差额=协议事件面 − 广播面:如实标注为 Unsupported-事件(未逐条列,聚合行;逐条展开归事件专项波)
