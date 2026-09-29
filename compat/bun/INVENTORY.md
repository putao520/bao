# Bun API INVENTORY(G1-Bun 全量盘点)

> 机械可重建;分类三态:**Supported**(实现+测试佐证)/ **Partial**(实现无测试或部分语义)/ **Unsupported**(无实现,如实列明非义务)。
> 本表只回答「有/无+证据」,不造通过率数据(通过率聚合是后续波)。

## 上游锚(ref 锁定)

- upstream repo:`~/code/rust/bun`(Rust-port workspace)
- HEAD:`e85606d48421d6c5aa6a41882f39b93d313616fd`(2026-05-28;package.json version **1.4.0**)
- 面来源:`src/runtime/api/BunObject.{zig,rs}`(static fn 面 ~70)、`src/runtime/api/*.classes.ts`(class 面)、`src/resolve_builtins/HardcodedModule.zig`(bun:*/node:* 模块面)、`src/node-fallbacks/`(node 回退实现)、`src/cli/`(CLI 子命令)
- bao 面来源:`src/bao_runtime/src/bun_api.rs`(`populate_bun_object`)+ 卫星 `bun_*_api.rs`/`bun_{sqlite,ffi,test,build,shell,password,listen,udp}.rs` + `bun_builtins.rs`/`require.rs`(模块注册)

## Bun.* 静态成员面

| api | class | bao_impl | tests | status | upstream_ref |
|---|---|---|---|---|---|
| Bun.version | — | bun_api.rs populate | bun_api_tests.rs, bun_api_deep_tests.rs, test_bun_api.js | Supported | BunObject.zig |
| Bun.env | — | bun_api.rs(getter→process.env 代理) | bun_api_tests.rs | Supported | getEnvNames/getEnvValue |
| Bun.argv | — | bun_api.rs | bun_api_tests.rs | Supported | getArgv |
| Bun.main | — | bun_api.rs | bun_api_tests.rs | Supported | get_main |
| Bun.cwd | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | getCWD |
| Bun.exit | — | bun_api.rs | bun_api_tests.rs | Supported | (process exit 面) |
| Bun.revision | — | bun_api.rs | bun_face_gap_tests.rs | Supported | (build info) |
| Bun.execPath / Bun.platform / Bun.arch 等进程信息 | — | bun_api.rs | bun_api_deep_tests.rs | Supported | (process 面) |
| Bun.file | BunFile | bun_api.rs:6827 `bun_file`(方法面 text/json/arrayBuffer/stream/writer/slice/exists/read) | bun_file_methods_e2e_tests.rs | Supported(方法面见下「高优先四项」) | mmapFile/open 面 |
| Bun.write | — | bun_api.rs `bun_write` | bun_face_e2e_tests.rs | Supported | (write 面) |
| Bun.read / Bun.readFile | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | (read 面) |
| Bun.serve | BunServer | bun_api.rs + bun_listen.rs + fetch 栈 | bun_serve_abort_surface_tests.rs, bun_serve_binary_body_tests.rs, bun_listen_tcp_behavior_tests.rs | Supported | serve |
| Bun.spawn / Bun.spawnSync | Subprocess | bun_spawn_sync.rs | bun_spawn_events_tests.rs, child_process_spawn_events_tests.rs | Supported | spawn/spawnSync+Subprocess.classes |
| Bun.sleep / Bun.sleepSync | — | bun_api.rs | bun_api_tests.rs | Supported | sleep/sleepSync |
| Bun.which | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | which |
| Bun.resolve / Bun.resolveSync | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | resolve/resolveSync |
| Bun.gc | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | gc |
| Bun.nanoseconds | — | bun_api.rs | bun_api_tests.rs | Supported | nanoseconds |
| Bun.sha / Bun.hash / Bun.CryptoHasher | HashObject | bun_hash_api.rs | bun_face_e2e_tests.rs(city32/64,xxHash32/64/3,murmur,rapidhash,sha) | Supported | getHashObject |
| Bun.gzip/gunzip/deflate/inflate(+Sync 族) | — | bun_api.rs compress 面 | bun_face_e2e_tests.rs(adler32/zip) | Supported | compress/decompress 族 |
| Bun.concatArrayBuffers | — | bun_api.rs | bun_concat_gc_rooting_tests.rs | Supported | (buffer 工具) |
| Bun.escapeHTML | — | bun_util_api.rs | bun_face_e2e_tests.rs | Supported | (escape 面) |
| Bun.fileURLToPath / Bun.pathToFileURL | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | (url 面) |
| Bun.deepLink / Bun.openInNewTab | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | (deep-link 面) |
| Bun.listen | TCPSocket/Listener | bun_listen.rs | bun_listen_tcp_behavior_tests.rs | Supported | (listen 面) |
| Bun.connect / Bun.tcpSocket | TCPSocket | bun_udp.rs/bun_listen.rs | bun_socket_uncaught_tests.rs | Partial(语义面窄于上游) | (connect 面) |
| Bun.Glob | Glob | bun_glob_api.rs | bun_face_e2e_tests.rs | Supported | getGlobConstructor+Glob.classes |
| Bun.Mime | Mime | bun_mime_api.rs | bun_face_e2e_tests.rs(css/mp3/png/svg/webp/woff2/mp4/pdf) | Supported | (mime 面) |
| Bun.TOML / Bun.YAML / Bun.JSONC | — | bun_serde_api.rs | bun_face_e2e_tests.rs | Supported | getTOMLObject/getYAMLObject/getJSONCObject |
| Bun.JSON5 | — | bun_serde_api.rs | (无专属测试命中) | Partial | getJSON5Object |
| Bun.inspect | — | bun_inspect_api.rs | bun_face_e2e_tests.rs | Supported | getInspect/inspectTable |
| Bun.peek / Bun.peekStatus | — | bun_api.rs | bun_face_e2e_tests.rs | Supported | (peek 面) |
| Bun.stringWidth | — | bun_util_api.rs | bun_face_e2e_tests.rs | Supported | stringWidth |
| Bun.readableStreamToArray 等 stream 工具 | — | web_streams.js/streams 面 | bun_face_e2e_tests.rs | Supported | ReadableStreamInternals |
| Bun.password | — | bun_password.rs | (测试命中歧义,无直接面测试) | Partial | (password 面) |
| Bun.shell / Bun.shellEscape | — | bun_shell.rs | bun_api_deep_tests.rs | Supported | shell/shellEscape/shell.ts |
| Bun.build | JSBundler | bun_build.rs | bun_build_e2e_tests.rs, test_bun_build.js | Supported | JSBundler.classes |
| Bun.test / Bun.testRun | — | bun_test.rs | bun_test_deep_tests.rs, test_bun_test.js | Supported | (test runner 面) |
| Bun.stdin / Bun.stdout / Bun.stderr | — | bun_api.rs(createBunStd* 面指针) | bun_api_tests.rs | Supported | createBunStdin/Stdout/Stderr |
| Bun.Transpiler / Bun.JSX | — | (API 面检索 0 命中;lib.rs 仅字符串提及) | 无 | Unsupported | getTranspilerConstructor+JSTranspiler |
| Bun.openInEditor | — | 无 | 无 | Unsupported | openInEditor |
| Bun.embeddedFiles / Bun.Archive | — | 无 | 无 | Unsupported | getEmbeddedFiles/getArchiveConstructor |
| Bun.S3 | S3Client | 无 | 无 | Unsupported | getS3ClientConstructor |
| Bun.Valkey | ValkeyClient | 无 | 无 | Unsupported | getValkeyClientConstructor |
| Bun.CSRF | — | 无 | 无 | Unsupported | getCSRFObject |
| Bun.FileSystemRouter | — | 无 | 无 | Unsupported | getFileSystemRouter |
| Bun.Image | — | 无 | 无 | Unsupported | getImageConstructor |
| Bun.Markdown | — | 无 | 无 | Unsupported | getMarkdownObject |
| Bun.Terminal | Terminal | 无 | 无 | Unsupported | getTerminalConstructor |
| Bun.x509 / Bun.TLS ciphers 面 | — | 无(stealth 栈内嵌 TLS,无 Bun.x509 面) | 无 | Unsupported | x509/getTLSDefaultCiphers |
| Bun.ResourceUsage(ResourceUsage 类) | ResourceUsage | (memoryUsage 部分字段经 process 面) | 无直接面测试 | Partial | ResourceUsage.classes |

## bun:* 模块面

| 模块 | bao_impl | tests | status | upstream_ref |
|---|---|---|---|---|
| bun:sqlite | bun_sqlite.rs | bun_sqlite_backup_tests.rs + REQ-ENG-008 面 | Supported | HardcodedModule @"bun:sqlite" |
| bun:ffi | bun_ffi.rs | REQ-ENG-009 面 | Supported | @"bun:ffi" |
| bun:test | bun_test.rs | bun_test_deep_tests.rs | Supported | @"bun:test" |
| bun:wrap | bun_builtins.rs + require.rs | (模块注册面) | Supported | @"bun:wrap" |
| bun:jsc | 无 | 无 | Unsupported | @"bun:jsc" |
| bun:main | 无 | 无 | Unsupported | @"bun:main" |
| bun:app | 无 | 无 | Unsupported | @"bun:app" |

## node:* 模块面(上游 HardcodedModule 40 项 + node-fallbacks)

上游 HardcodedModule 列 40 个 `node:*`;`src/node-fallbacks/` 提供 24 个 .js 回退实现(assert/buffer/console/crypto/domain/events/http/https/net/os/path/process/punycode/querystring/stream/string_decoder/sys/timers/tty/url/util/zlib + constants)。
bao 侧 `require.rs` 的内置解析面消费同一清单(bao_runtime/node_*.rs 为重实现:node_url/node_http/node_crypto/node_dns/node_worker_threads/node_inspector/node_tty/node_fs 族)。
**状态:aggregate Supported(逐模块语义面归 G1-Node inventory,本表不展开)。**

## CLI 子命令面(上游 24: add audit build bunx create discord exec fuzzilli init install link outdated pack patch publish remove repl run scan test unlink update upgrade why)

| 子命令 | bao_impl | status |
|---|---|---|
| run / test / build | bao_cli(bao run/test/build 面) | Supported(面归 CLI REQ 域) |
| add / remove / install / update / link / unlink / publish / outdated / why / pack / audit / create / bunx / upgrade / init | 包管理器族 | Unsupported(bao 无包管理器;非义务) |
| repl / exec / discord / fuzzilli / scan | 交互/内部工具 | Unsupported |

## 高优先四项量化(P0)

### Bun.file
- 实现:`src/bao_runtime/src/bun_api.rs:6827` `bun_file`(host fn;构造 BunFile 对象)
- 方法面(grep 实证):`text` `json` `arrayBuffer` `stream` `writer` `slice` `exists` `read`
- 测试:`bun_file_methods_e2e_tests.rs`(专测)
- 上游 diff 概要:核心读/流/探测面齐;上游增量(增量读、写入型方法经 Bun.write 承担、大文件 mmap 读)未对齐

### Bun.write
- 实现:`src/bao_runtime/src/bun_api.rs` `bun_write`
- 测试:`bun_face_e2e_tests.rs`(写文件+返回值断言)
- 上游 diff 概要:路径+数据核心形态齐;fd/Blob/Response 高级形态未全对齐

### Bun.serve
- 实现:`bun_api.rs` `serve` + `bun_listen.rs` + fetch 栈(bao_runtime fetch_api/server 面)
- 测试:`bun_serve_abort_surface_tests.rs` `bun_serve_binary_body_tests.rs` `bun_listen_tcp_behavior_tests.rs`
- 上游 diff 概要:fetch handler + port + abort + binary body + websocket upgrade 面有测试;上游 `BunServer.publish`/TLS serve 细分形态未全对齐

### Bun.spawn
- 实现:`bun_spawn_sync.rs`(spawn/spawnSync)+ Subprocess 面(pid/stdin/stdout/stderr/kill/exit/on-exit 事件)
- 测试:`bun_spawn_events_tests.rs` `child_process_spawn_events_tests.rs`
- 上游 diff 概要:spawn/spawnSync + Subprocess 事件面齐;上游 IPC fd 传递/interpreter 透传细分形态未全对齐

## 汇总

- 总面 N = **58**(Bun.* 静态成员族 51 行 + bun:* 模块 7 行;node:* 40 项与 CLI 24 子命令以聚合行另列,不计入 N)
- **Supported 40 / Partial 4(connect·tcpSocket、JSON5、password、ResourceUsage)/ Unsupported 14**(Transpiler·JSX、openInEditor、embeddedFiles·Archive、S3、Valkey、CSRF、FileSystemRouter、Image、Markdown、Terminal、x509·TLS 面 = 11 行;bun:jsc、bun:main、bun:app = 3 行)
- 高优先四项:**file=Supported**(BunFile 方法面 9 项,专测)/ **write=Supported** / **serve=Supported**(三专测)/ **spawn=Supported**(双专测)
