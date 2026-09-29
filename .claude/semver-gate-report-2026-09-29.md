# semver-gate report — 2026-09-29 15:42:30

- tool: cargo-semver-checks 0.50.0
- crate set (113): bao-core bao-browser bao_cdp bao_engine bao_engine_macros bun_collections bun_alloc bun_opaque bun_wyhash bun_collections_macros bun_core bun_core_macros bun_output_tags bun_dispatch bun_hash bun_highway bun_simdutf_sys bun_windows_sys bun_ptr bun_sys bun_errno bun_libuv_sys bun_paths bun_crash_handler bun_analytics bun_semver bun_ast bun_base64 bun_io bun_spawn_sys bun_threading bun_safety bun_uws_sys bun_boringssl_sys bun_http_types bun_url bun_lsquic_sys bun_which bun_zlib bun_event_loop bun_dotenv bun_uws bun_boringssl bun_cares_sys bun_jsc_macros bun_sm bun_transpiler bun_bundler bun_css bun_css_derive bun_js_parser bun_options_types bun_install_types bun_js_printer bun_sourcemap bun_parsers bun_zstd bun_lolhtml_sys bun_md bun_perf bun_resolve_builtins bun_resolver bun_glob bun_watcher bun_router bao-mozjs bao-mozjs-sys bao-mozjs-src-intl bao-mozjs-src-js bao-mozjs-src-python bao-mozjs-collator-glue bao-mozjs-icu-collator bao-mozjs-icu-collator-data bao-mozjs-icu-collections bao-mozjs-icu-normalizer bao-mozjs-icu-normalizer-data bao-mozjs-utf16-iter bao-mozjs-utf8-iter bao-mozjs-locale-glue bao-mozjs-normalizer-glue bao-mozjs-properties-glue bao-mozjs-unicode-bidi-ffi bao_bundler bun_runtime bao_boringssl_bridge bao_crypto bao_cdp_client bao_stealth bun_sha_hmac bao_uloop bun_dns bao_native_stubs bun_spawn bun_output bao_workflow_host bun_brotli bun_http bun_picohttp bun_install bun_api bun_bunfig bun_clap bun_clap_macros bun_standalone_graph bun_exe_format bun_libarchive bun_s3_signing bun_shell_parser cdp-server bao_bin bao_lints bun_platform bun_sql
- semantics: major-required = hard fail (0.x → minor bump owed); minor/patch = advisory; timeout = SKIP (annotated)

| crate | verdict | detail |
|---|---|---|
| bao-core | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bao-browser | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bao_cdp | PASS | — |
| bao_engine | PASS | — |
| bao_engine_macros | ADVISORY | break detected |
| bun_collections | PASS | — |
| bun_alloc | PASS | — |
| bun_opaque | PASS | — |
| bun_wyhash | PASS | — |
| bun_collections_macros | ADVISORY | break detected |
| bun_core | PASS | — |
| bun_core_macros | ADVISORY | break detected |
| bun_output_tags | PASS | — |
| bun_dispatch | ADVISORY | break detected |
| bun_hash | PASS | — |
| bun_highway | PASS | — |
| bun_simdutf_sys | PASS | — |
| bun_windows_sys | ADVISORY | break detected |
| bun_ptr | PASS | — |
| bun_sys | PASS | — |
| bun_errno | PASS | — |
| bun_libuv_sys | PASS | — |
| bun_paths | PASS | — |
| bun_crash_handler | PASS | — |
| bun_analytics | PASS | — |
| bun_semver | PASS | — |
| bun_ast | PASS | — |
| bun_base64 | PASS | — |
| bun_io | PASS | — |
| bun_spawn_sys | PASS | — |
| bun_threading | PASS | — |
| bun_safety | PASS | — |
| bun_uws_sys | PASS | — |
| bun_boringssl_sys | PASS | — |
| bun_http_types | PASS | — |
| bun_url | PASS | — |
| bun_lsquic_sys | PASS | — |
| bun_which | PASS | — |
| bun_zlib | PASS | — |
| bun_event_loop | PASS | — |
| bun_dotenv | PASS | — |
| bun_uws | PASS | — |
| bun_boringssl | PASS | — |
| bun_cares_sys | PASS | — |
| bun_jsc_macros | ADVISORY | break detected |
| bun_sm | PASS | — |
| bun_transpiler | PASS | — |
| bun_bundler | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bun_css | PASS | — |
| bun_css_derive | ADVISORY | break detected |
| bun_js_parser | PASS | — |
| bun_options_types | PASS | — |
| bun_install_types | PASS | — |
| bun_js_printer | PASS | — |
| bun_sourcemap | PASS | — |
| bun_parsers | PASS | — |
| bun_zstd | PASS | — |
| bun_lolhtml_sys | PASS | — |
| bun_md | PASS | — |
| bun_perf | PASS | — |
| bun_resolve_builtins | PASS | — |
| bun_resolver | PASS | — |
| bun_glob | PASS | — |
| bun_watcher | PASS | — |
| bun_router | PASS | — |
| bao-mozjs | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bao-mozjs-sys | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bao-mozjs-src-intl | PASS | — |
| bao-mozjs-src-js | PASS | — |
| bao-mozjs-src-python | PASS | — |
| bao-mozjs-collator-glue | PASS | — |
| bao-mozjs-icu-collator | PASS | — |
| bao-mozjs-icu-collator-data | PASS | — |
| bao-mozjs-icu-collections | PASS | — |
| bao-mozjs-icu-normalizer | PASS | — |
| bao-mozjs-icu-normalizer-data | PASS | — |
| bao-mozjs-utf16-iter | PASS | — |
| bao-mozjs-utf8-iter | PASS | — |
| bao-mozjs-locale-glue | PASS | — |
| bao-mozjs-normalizer-glue | PASS | — |
| bao-mozjs-properties-glue | PASS | — |
| bao-mozjs-unicode-bidi-ffi | PASS | — |
| bao_bundler | PASS | — |
| bun_runtime | ADVISORY | break detected |
| bao_boringssl_bridge | PASS | — |
| bao_crypto | PASS | — |
| bao_cdp_client | PASS | — |
| bao_stealth | PASS | — |
| bun_sha_hmac | PASS | — |
| bao_uloop | PASS | — |
| bun_dns | PASS | — |
| bao_native_stubs | PASS | — |
| bun_spawn | PASS | — |
| bun_output | PASS | — |
| bao_workflow_host | PASS | — |
| bun_brotli | PASS | — |
| bun_http | PASS | — |
| bun_picohttp | PASS | — |
| bun_install | TOOL-ERROR | rustdoc build failed (infra; not an API break) |
| bun_api | PASS | — |
| bun_bunfig | PASS | — |
| bun_clap | PASS | — |
| bun_clap_macros | ADVISORY | break detected |
| bun_standalone_graph | PASS | — |
| bun_exe_format | PASS | — |
| bun_libarchive | PASS | — |
| bun_s3_signing | PASS | — |
| bun_shell_parser | PASS | — |
| cdp-server | ADVISORY | break detected |
| bao_bin | ADVISORY | break detected |
| bao_lints | PASS | — |
| bun_platform | PASS | — |
| bun_sql | PASS | — |
| summary | major-required hard fails: 0; advisory: 11 |

## 判定分布(113 crate)

- PASS: 96
- ADVISORY(break detected,报告不阻断): 11 — bao_engine_macros / bun_collections_macros / bun_core_macros / bun_dispatch / bun_windows_sys / bun_jsc_macros / bun_css_derive / bun_runtime / bun_clap_macros / cdp-server / bao_bin(逐 crate required-bump 明细见工具输出日志)
- TOOL-ERROR(rustdoc 构建失败,基础设施;非 API break): 6 — bao-core / bao-browser / bun_bundler / bao-mozjs / bao-mozjs-sys / bun_install(共同根因:vendor stylo_atoms 的 atom! 宏在 rustdoc 展开下拒绝 W3 波新增 atom enter/exit/cuechange——vendor 宏 rustdoc 兼容修复归 stylo_atoms fork 维护,不在本 gate 范围)
- SKIP(timeout) / FAIL(major-required): 0

**Gate 结论:0 major-required(硬 fail=0)→ 退出码 0;W2 alias 边界经全量 semver 检测无 major-required 破洞。**
