# Environment Variable Contract (docs/env-vars.md)

SSOT companion to `bun_core::getenv_z` / `getenv_z_any_case` (the env read
layer: a `BUN_<SUFFIX>` lookup that misses falls back to `BAO_<SUFFIX>`;
explicit `BUN_` wins; the host process env is never mutated by the alias).

Census: **154 distinct `BUN_*`/`BAO_*` string literals** across `src/`
(w22c freeze inventory; the lock test re-derives the accessor section from
`src/bun_core/env_var.rs` and fails on any drift vs this document).

## 1. 产品契约 — `bun_core::env_var` accessors (82 keys)

Single enumeration point: `src/bun_core/env_var.rs` (macro-declared
accessors). The lock test (`env_alias_doc_lock_tests`) diff-checks this
section against that file in BOTH directions.

| Key | 定义/消费点 |
|---|---|
| `BUN_AGENT_RULE_DISABLED` | `bun_core/env_var.rs:48`(定义) |
| `BUN_ASSUME_PERFECT_INCREMENTAL` | `bun_core/env_var.rs:187`(定义) |
| `BUN_BE_BUN` | `bun_core/env_var.rs:188`(定义) |
| `BUN_COMPILE_TARGET_TARBALL_URL` | `bun_core/env_var.rs:49`(定义) |
| `BUN_CONFIG_DISABLE_COPY_FILE_RANGE` | `bun_core/env_var.rs:50`(定义) |
| `BUN_CONFIG_DISABLE_ioctl_ficlonerange` | `bun_core/env_var.rs:51`(定义) |
| `BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS` | `bun_core/env_var.rs:57`(定义) — 消费: `bao_runtime/tests/suite/env_alias_tests.rs:68`; `bao_runtime/tests/suite/env_alias_tests.rs:70` |
| `BUN_CONFIG_HTTP_IDLE_TIMEOUT` | `bun_core/env_var.rs:62`(定义) — 消费: `bao_runtime/tests/suite/env_alias_tests.rs:37`; `bao_runtime/tests/suite/env_alias_tests.rs:52` |
| `BUN_CRASH_REPORT_URL` | `bun_core/env_var.rs:63`(定义) |
| `BUN_DEBUG` | `bun_core/env_var.rs:64`(定义) |
| `BUN_DEBUG_ALL` | `bun_core/env_var.rs:65`(定义) |
| `BUN_DEBUG_CSS_ORDER` | `bun_core/env_var.rs:66`(定义) |
| `BUN_DEBUG_ENABLE_RESTORE_FROM_TRANSPILER_CACHE` | `bun_core/env_var.rs:67`(定义) |
| `BUN_DEBUG_FORCE_NIX_HOST` | `bun_core/env_var.rs:71`(定义) |
| `BUN_DEBUG_HASH_RANDOM_SEED` | `bun_core/env_var.rs:72`(定义) |
| `BUN_DEBUG_NO_DUMP` | `bun_core/env_var.rs:189`(定义) |
| `BUN_DEBUG_QUIET_LOGS` | `bun_core/env_var.rs:73`(定义) |
| `BUN_DEBUG_TEST_TEXT_LOCKFILE` | `bun_core/env_var.rs:74`(定义) |
| `BUN_DESTRUCT_VM_ON_EXIT` | `bun_core/env_var.rs:190`(定义) |
| `BUN_DEV_SERVER_TEST_RUNNER` | `bun_core/env_var.rs:75`(定义) |
| `BUN_DISABLE_SLOW_LIFECYCLE_SCRIPT_LOGGING` | `bun_core/env_var.rs:215`(定义) |
| `BUN_DISABLE_SOURCE_CODE_PREVIEW` | `bun_core/env_var.rs:216`(定义) |
| `BUN_DISABLE_TRANSPILED_SOURCE_CODE_PREVIEW` | `bun_core/env_var.rs:220`(定义) |
| `BUN_DUMP_STATE_ON_CRASH` | `bun_core/env_var.rs:222`(定义) |
| `BUN_DUMP_SYMBOLS` | `bun_core/env_var.rs:78`(定义) |
| `BUN_ENABLE_CRASH_REPORTING` | `bun_core/env_var.rs:79`(定义) |
| `BUN_ENABLE_EXPERIMENTAL_SHELL_BUILTINS` | `bun_core/env_var.rs:223`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_ADDRCONFIG` | `bun_core/env_var.rs:198`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_ASYNC_TRANSPILER` | `bun_core/env_var.rs:199`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_DNS_CACHE` | `bun_core/env_var.rs:201`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_DNS_CACHE_LIBINFO` | `bun_core/env_var.rs:202`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_IGNORE_SCRIPTS` | `bun_core/env_var.rs:196`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_INSTALL_INDEX` | `bun_core/env_var.rs:203`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_IO_POOL` | `bun_core/env_var.rs:208`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_IPV4` | `bun_core/env_var.rs:209`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_IPV6` | `bun_core/env_var.rs:210`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_ISOLATION_SOURCE_CACHE` | `bun_core/env_var.rs:200`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_MEMFD` | `bun_core/env_var.rs:211`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_NATIVE_DEPENDENCY_LINKER` | `bun_core/env_var.rs:193`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_REDIS_AUTO_PIPELINING` | `bun_core/env_var.rs:213`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_RWF_NONBLOCK` | `bun_core/env_var.rs:214`(定义) — 消费: `sys/lib.rs:5703` |
| `BUN_FEATURE_FLAG_DISABLE_SOURCE_MAPS` | `bun_core/env_var.rs:217`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_SPAWNSYNC_FAST_PATH` | `bun_core/env_var.rs:218`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_SQL_AUTO_PIPELINING` | `bun_core/env_var.rs:219`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_STREAMING_INSTALL` | `bun_core/env_var.rs:207`(定义) |
| `BUN_FEATURE_FLAG_DISABLE_UV_FS_COPYFILE` | `bun_core/env_var.rs:221`(定义) |
| `BUN_FEATURE_FLAG_DUMP_CODE` | `bun_core/env_var.rs:85`(定义) |
| `BUN_FEATURE_FLAG_EXPERIMENTAL_BAKE` | `bun_core/env_var.rs:224`(定义) |
| `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP2_CLIENT` | `bun_core/env_var.rs:231`(定义) |
| `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP3_CLIENT` | `bun_core/env_var.rs:236`(定义) |
| `BUN_FEATURE_FLAG_FORCE_IO_POOL` | `bun_core/env_var.rs:237`(定义) |
| `BUN_FEATURE_FLAG_FORCE_WINDOWS_JUNCTIONS` | `bun_core/env_var.rs:238`(定义) |
| `BUN_FEATURE_FLAG_LAST_MODIFIED_PRETEND_304` | `bun_core/env_var.rs:246`(定义) |
| `BUN_FEATURE_FLAG_NO_LIBDEFLATE` | `bun_core/env_var.rs:248`(定义) |
| `BUN_FEATURE_FLAG_NO_ORPHANS` | `bun_core/env_var.rs:84`(定义) — 消费: `io/ParentDeathWatchdog.rs:248`; `install/lib.rs:908` |
| `BUN_INOTIFY_COALESCE_INTERVAL` | `bun_core/env_var.rs:88`(定义) |
| `BUN_INSPECT` | `bun_core/env_var.rs:89`(定义) |
| `BUN_INSPECT_CONNECT_TO` | `bun_core/env_var.rs:90`(定义) |
| `BUN_INSPECT_PRELOAD` | `bun_core/env_var.rs:91`(定义) |
| `BUN_INSTALL` | `bun_core/env_var.rs:92`(定义) — 消费: `install/PackageManager/PackageManagerDirectories.rs:461` |
| `BUN_INSTALL_BIN` | `bun_core/env_var.rs:93`(定义) |
| `BUN_INSTALL_GLOBAL_DIR` | `bun_core/env_var.rs:94`(定义) |
| `BUN_INSTALL_STREAMING_MIN_SIZE` | `bun_core/env_var.rs:99`(定义) |
| `BUN_INSTRUMENTS` | `bun_core/env_var.rs:239`(定义) |
| `BUN_INTERNAL_BUNX_INSTALL` | `bun_core/env_var.rs:240`(定义) |
| `BUN_INTERNAL_SUPPRESS_CRASH_IN_BUN_RUN` | `bun_core/env_var.rs:241`(定义) |
| `BUN_INTERNAL_SUPPRESS_CRASH_ON_NAPI_ABORT` | `bun_core/env_var.rs:242`(定义) |
| `BUN_INTERNAL_SUPPRESS_CRASH_ON_UV_STUB` | `bun_core/env_var.rs:245`(定义) |
| `BUN_INTERNAL_WEBVIEW_HOST` | `bun_core/env_var.rs:150`(定义) |
| `BUN_NEEDS_PROC_SELF_WORKAROUND` | `bun_core/env_var.rs:100`(定义) |
| `BUN_NO_CODESIGN_MACHO_BINARY` | `bun_core/env_var.rs:247`(定义) |
| `BUN_OPTIONS` | `bun_core/env_var.rs:101`(定义) |
| `BUN_POSTGRES_SOCKET_MONITOR` | `bun_core/env_var.rs:102`(定义) |
| `BUN_POSTGRES_SOCKET_MONITOR_READER` | `bun_core/env_var.rs:103`(定义) |
| `BUN_RUNTIME_TRANSPILER_CACHE_PATH` | `bun_core/env_var.rs:104`(定义) |
| `BUN_SSG_DISABLE_STATIC_ROUTE_VISITOR` | `bun_core/env_var.rs:105`(定义) |
| `BUN_TCC_OPTIONS` | `bun_core/env_var.rs:106`(定义) |
| `BUN_TMPDIR` | `bun_core/env_var.rs:113`(定义) |
| `BUN_TRACE` | `bun_core/env_var.rs:250`(定义) |
| `BUN_TRACK_LAST_FN_NAME` | `bun_core/env_var.rs:114`(定义) |
| `BUN_TRACY_PATH` | `bun_core/env_var.rs:115`(定义) |
| `BUN_WATCHER_TRACE` | `bun_core/env_var.rs:116`(定义) |

## 2. 产品契约 — direct `getenv_z` consumers, `BUN_*` (44 keys)

Read via `bun_core::getenv_z`/`getenv_z_any_case`/raw literals outside the
accessor module (threading/ast/md/sys/output/debug faces). Same alias
semantics; not covered by the accessor lock (documented for completeness).

| Key | 引用数 | 引用点 |
|---|---|---|
| `BUN_API_PASSED` | 1 | `bao_runtime/tests/suite/npm_project_e2e_tests.rs:470` |
| `BUN_CODEGEN_DIR` | 5 | `resolver/node_fallbacks.rs:52`; `bun_core/util.rs:3308` |
| `BUN_CONFIG_HTTP_RETRY_COUNT` | 1 | `install/PackageManager/PackageManagerOptions.rs:718` |
| `BUN_CONFIG_MANIFEST_CACHE_CONTROL_TIMESTAMP` | 1 | `install/PackageManager.rs:2327` |
| `BUN_CONFIG_MAX_HTTP_REQUESTS` | 1 | `http/AsyncHTTP.rs:234` |
| `BUN_CONFIG_NO_CLEAR_TERMINAL_ON_RELOAD` | 1 | `dotenv/env_loader.rs:544` |
| `BUN_CONFIG_NO_VERIFY` | 1 | `install/PackageManager/PackageManagerOptions.rs:739` |
| `BUN_CONFIG_REGISTRY` | 1 | `install/PackageManager/PackageManagerOptions.rs:631` |
| `BUN_CONFIG_SKIP_INSTALL_PACKAGES` | 1 | `install/PackageManager/PackageManagerOptions.rs:735` |
| `BUN_CONFIG_SKIP_LOAD_LOCKFILE` | 1 | `install/PackageManager/PackageManagerOptions.rs:731` |
| `BUN_CONFIG_SKIP_SAVE_LOCKFILE` | 1 | `install/PackageManager/PackageManagerOptions.rs:727` |
| `BUN_CONFIG_TOKEN` | 1 | `install/PackageManager/PackageManagerOptions.rs:693` |
| `BUN_CONFIG_YARN_LOCKFILE` | 1 | `install/PackageManager/PackageManagerOptions.rs:714` |
| `BUN_DEBUG_` | 1 | `bun_core/output.rs:1568` |
| `BUN_DEBUG_CSS_ORDER_` | 1 | `bundler/linker_context/findImportedFilesInCSSOrder.rs:927` |
| `BUN_DEBUG_CSS_TARGET_android` | 1 | `css/targets.rs:90` |
| `BUN_DEBUG_CSS_TARGET_chrome` | 1 | `css/targets.rs:91` |
| `BUN_DEBUG_CSS_TARGET_edge` | 1 | `css/targets.rs:92` |
| `BUN_DEBUG_CSS_TARGET_firefox` | 1 | `css/targets.rs:93` |
| `BUN_DEBUG_CSS_TARGET_ie` | 1 | `css/targets.rs:94` |
| `BUN_DEBUG_CSS_TARGET_ios_saf` | 1 | `css/targets.rs:95` |
| `BUN_DEBUG_CSS_TARGET_opera` | 1 | `css/targets.rs:96` |
| `BUN_DEBUG_CSS_TARGET_safari` | 1 | `css/targets.rs:97` |
| `BUN_DEBUG_CSS_TARGET_samsung` | 1 | `css/targets.rs:98` |
| `BUN_DEBUG_uv` | 1 | `libuv_sys/libuv.rs:37` |
| `BUN_DEFAULT_MAX_HTTP_HEADER_SIZE` | 1 | `http/lib.rs:321` |
| `BUN_DISABLE_KITTY_PROBE` | 1 | `md/ansi_renderer.rs:2535` |
| `BUN_DISABLE_STORE_AST_HEAP` | 1 | `ast/lib.rs:3572` |
| `BUN_DISABLE_TRANSPILER` | 1 | `bundler/transpiler.rs:842` |
| `BUN_ENV` | 1 | `dotenv/env_loader.rs:214` |
| `BUN_ENVALIAS_ANYCZ` | 4 | `bao_runtime/tests/suite/env_alias_tests.rs:111`; `bao_runtime/tests/suite/env_alias_tests.rs:125` |
| `BUN_ENVALIAS_DIRECTZ` | 4 | `bao_runtime/tests/suite/env_alias_tests.rs:109`; `bao_runtime/tests/suite/env_alias_tests.rs:119` |
| `BUN_ENVALIAS_JS_A` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:246`; `bao_runtime/tests/suite/env_alias_tests.rs:296` |
| `BUN_ENVALIAS_JS_B` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:248`; `bao_runtime/tests/suite/env_alias_tests.rs:251` |
| `BUN_ENVALIAS_LCY_A` | 5 | `bao_runtime/tests/suite/env_alias_tests.rs:187`; `bao_runtime/tests/suite/env_alias_tests.rs:196` |
| `BUN_ENVALIAS_LCY_B` | 4 | `bao_runtime/tests/suite/env_alias_tests.rs:188`; `bao_runtime/tests/suite/env_alias_tests.rs:213` |
| `BUN_FEATURE_FLAG_FORCE_WAITER_THREAD` | 1 | `install/PackageManager.rs:1940` |
| `BUN_INSTALL_CACHE_DIR` | 2 | `install/PackageManager/PackageManagerDirectories.rs:445`; `sys/lib.rs:9348` |
| `BUN_INSTALL_GLOBAL_STORE` | 1 | `install/PackageManager/PackageManagerOptions.rs:611` |
| `BUN_INSTALL_PROGRESS` | 1 | `install/PackageManager/PackageManagerOptions.rs:616` |
| `BUN_INSTALL_VERBOSE` | 2 | `install/PackageManager.rs:1936`; `install/PackageManager.rs:2377` |
| `BUN_MANIFEST_CACHE` | 2 | `install/PackageManager.rs:2188`; `install/PackageManager.rs:2611` |
| `BUN_THREADPOOL_STATS` | 1 | `threading/ThreadPool.rs:42` |
| `BUN_WHICH_IGNORE_CWD` | 1 | `install/PackageManager.rs:1355` |

## 3. 产品契约 — `BAO_*` native readers (3 keys)

| Key | 引用数 | 引用点 |
|---|---|---|
| `BAO_PAGE_PHASE_BUDGET_MS` | 1 | `bao_browser/src/phase_watch.rs:156` |
| `BAO_UWS_WITH_TLS` | 1 | `uws_sys/build.rs:28` |
| `BAO_XDR_CACHE_DIR` | 1 | `bao_engine/src/xdr_cache.rs:97`(定义;REQ-ENG-012 stage2 持久 XDR 缓存 opt-in 开关——未设=层禁用零磁盘 IO) |

## 4. 测试 harness (non-contract) (15 keys)

Test-suite gates and fixtures — NOT product promises; listed to prevent
accidental product reliance.

| Key | 引用数 | 引用点 |
|---|---|---|
| `BAO_BIN` | 3 | `bao_browser/tests/suite/bao_cli_timeout_e2e_tests.rs:52`; `bao_browser/tests/suite/bao_cli_e2e_tests.rs:34` |
| `BAO_CONFIG_DNS_TIME_TO_LIVE_SECONDS` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:69`; `bao_runtime/tests/suite/env_alias_tests.rs:71` |
| `BAO_CONFIG_HTTP_IDLE_TIMEOUT` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:38`; `bao_runtime/tests/suite/env_alias_tests.rs:39` |
| `BAO_REG_NO_STEALTH` | 1 | `bao_browser/tests/suite/opaque_origin_startup_regression_tests.rs:90` |
| `BAO_SUITE_ISOLATED_TEST` | 1 | `bao_browser/tests/suite/common/mod.rs:38` |
| `BAO_SUITE_ISOLATION_DEPTH` | 1 | `bao_runtime/tests/suite/exit_isolation.rs:32` |
| `BAO_SUITE_ISOLATION_TIMEOUT_SECS` | 1 | `bao_runtime/tests/suite/exit_isolation.rs:33` |
| `BAO_TEST_BAO_BIN` | 8 | `bao_runtime/tests/suite/p0_cluster_fork_tests.rs:118`; `bao_runtime/tests/suite/cluster_isprimary_race_regression_tests.rs:38` |
| `BAO_TEST_CHROME_URL` | 2 | `bao_cdp_client/tests/suite/e2e_external_chrome.rs:460`; `bao_cdp_client/tests/suite/e2e_external_chrome.rs:550` |
| `BAO_TEST_GETCWD_DELETED_CWD_CHILD` | 1 | `resolver/lib.rs:2620` |
| `BAO_TEST_NETWORK` | 23 | `bao_runtime/tests/suite/h3_fetch_tests.rs:27`; `bao_browser/tests/suite/worker_fingerprint_consistency_tests.rs:52` |
| `BAO_TEST_PLAYWRIGHT` | 2 | `bao_cdp_client/tests/suite/e2e_playwright_compat.rs:487`; `bao_cdp_client/tests/suite/e2e_playwright_compat.rs:504` |
| `BAO_TEST_REAL_SERVO` | 6 | `bao_browser/tests/suite/multi_page_security_e2e_tests.rs:122`; `bao_browser/tests/suite/click_human_e2e_tests.rs:169` |
| `BAO_TEST_TRACE` | 2 | `http/tests/tls_info_and_streaming_tests.rs:98`; `http/tests/connection_close_guard_tests.rs:94` |
| `BAO_TEST_VAR` | 1 | `bao_runtime/src/bun_shell.rs:1663` |

## 5. 内部瞬态 — process-internal signalling (11 keys)

Cluster worker bootstrap / alias-test signalling / spawn internal maps —
never a public contract.

| Key | 引用数 | 引用点 |
|---|---|---|
| `BAO_CLUSTER_EXEC` | 4 | `bao_runtime/src/node_cluster.rs:885`; `bao_runtime/tests/suite/p0_cluster_fork_tests.rs:198` |
| `BAO_CLUSTER_IPC_FD` | 3 | `bao_runtime/src/node_cluster.rs:943`; `bao_runtime/tests/suite/cluster_isprimary_race_regression_tests.rs:161` |
| `BAO_CLUSTER_PRIMARY_PID` | 3 | `bao_runtime/src/node_cluster.rs:940`; `bao_runtime/tests/suite/cluster_isprimary_race_regression_tests.rs:160` |
| `BAO_CLUSTER_WORKER_ID` | 8 | `bao_runtime/src/node_cluster.rs:260`; `bao_runtime/src/node_cluster.rs:291` |
| `BAO_ENVALIAS_ANYCZ` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:112`; `bao_runtime/tests/suite/env_alias_tests.rs:114` |
| `BAO_ENVALIAS_DIRECTZ` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:110`; `bao_runtime/tests/suite/env_alias_tests.rs:113` |
| `BAO_ENVALIAS_JS_A` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:247`; `bao_runtime/tests/suite/env_alias_tests.rs:250` |
| `BAO_ENVALIAS_JS_B` | 3 | `bao_runtime/tests/suite/env_alias_tests.rs:249`; `bao_runtime/tests/suite/env_alias_tests.rs:252` |
| `BAO_ENVALIAS_LCY_A` | 2 | `bao_runtime/tests/suite/env_alias_tests.rs:189`; `bao_runtime/tests/suite/env_alias_tests.rs:228` |
| `BAO_ENVALIAS_LCY_B` | 2 | `bao_runtime/tests/suite/env_alias_tests.rs:207`; `bao_runtime/tests/suite/env_alias_tests.rs:229` |
| `BAO_WINRED_MAP` | 2 | `spawn/process.rs:2216`; `spawn/process.rs:2615` |
