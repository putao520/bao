# C4 Canvas/Audio 反指纹：JS 层 → Servo 渲染层迁移

## 变更总览

REQ-STL-003 要求"在 servo webrender 渲染层注入确定性随机噪声"，验收标准第5条"JS 层无法检测噪声注入"。
此前 Canvas 噪声通过 JS prototype hook 拦截 `getImageData/toDataURL/toBlob`，`Function.prototype.toString()` 可暴露非原生函数，违反 SPEC。

本次将 Canvas 噪声注入从 JS 层迁移到 servo 的 `CanvasData::read_pixels()` 渲染层，同时修复 Audio JS hooks 的 `toString()` 检测问题。

---

## Servo 改动（6 文件，1 新建）

### 1. `components/canvas/canvas_noise.rs` — **新建**

- `CanvasNoiseConfig` 结构体（`seed`, `noise_amplitude`, `enabled`）
  - `disabled()` / `new(seed, amplitude)` / `is_enabled()`
  - `apply_to_pixels(&self, data: &mut [u8], width, height)` — 遍历 RGBA8 像素数据，R×255/G×127/B×63 通道加权噪声，alpha 不变
  - `deterministic_noise(x, y)` — 与 `bao_stealth::canvas::CanvasNoise` 完全一致的 splitmix64 算法
- **全局 Atomic 跨线程通信**（script thread 写，canvas paint thread 读）：
  - `set_global_canvas_noise(seed, amplitude)` — 写入 `AtomicU64`/`AtomicU8`
  - `get_global_canvas_noise() -> Option<(u64, f64)>` — 读取全局配置
- 8 个单元测试（disabled/enabled/deterministic/different_seeds/range/parity/preserve_signal/zero_dims）

### 2. `components/canvas/canvas_data.rs` — 修改

- `CanvasData<DrawTarget>` 新增 `noise_config: CanvasNoiseConfig` 字段
- `new()` 初始化时从 `get_global_canvas_noise()` 读取配置（非 disabled）
- `read_pixels()` 签名改为 `read_pixels(&mut self, read_rect, apply_noise: bool) -> Snapshot`
  - `apply_noise=true && noise_config.enabled` → 先 `snapshot.transform()` 确保 RGBA8 unpremultiplied，再 `noise_config.apply_to_pixels(snapshot.as_raw_bytes_mut())`
  - `apply_noise=false` → 返回原始 snapshot（防止 canvas→canvas 拷贝噪声叠加）
- 新增 `set_noise_config(&mut self, config)`

### 3. `components/canvas/canvas_paint_thread.rs` — 修改

- `CanvasPaintThread` 新增 `default_noise_config: CanvasNoiseConfig` 字段
- `new()` 从 `get_global_canvas_noise()` 初始化
- `create_canvas()` 创建 canvas 后调用 `canvas.set_noise_config(self.default_noise_config.clone())`
- `Canvas2dMsg::GetImageData` → `read_pixels(dest_rect, true)` (apply_noise=true)
- `Canvas2dMsg::DrawImageInOther` → `read_pixels(..., false)` (apply_noise=false，防止噪声叠加)
- `Canvas` enum 新增 `set_noise_config()` 方法，转发到 `CanvasData::set_noise_config()`
- `Canvas::read_pixels()` 签名添加 `apply_noise: bool` 参数

### 4. `components/canvas/lib.rs` — 修改

- 新增 `pub mod canvas_noise`

### 5. `components/servo/lib.rs` — 修改

- 新增公开 API：
  ```rust
  pub fn set_canvas_noise_seed(seed: u64, noise_amplitude: f64) {
      servo_canvas::canvas_noise::set_global_canvas_noise(seed, noise_amplitude);
  }
  ```

### 6. `components/servo/Cargo.toml` — 修改

- 新增依赖 `servo-canvas = { workspace = true }`

---

## Bao 改动（2 文件）

### 7. `src/bao_stealth/src/engine_props.rs` — 修改

**移除 Canvas JS hooks：**
- 删除 `inject_canvas_audio_hooks()` 中全部 Canvas JS 代码（~90 行）
  - 删除 `getImageData` / `toDataURL` / `toBlob` 的 JS prototype 拦截
  - 删除 `deterministicNoise(x, y)` / `applyNoiseToPixel` / `noisifyImageData` JS 函数
- 重命名为 `inject_audio_hooks()`

**新增访问器（供 servo 渲染层通过 runtime_bridge 读取）：**
- `pub fn canvas_seed() -> u64`
- `pub fn canvas_amplitude() -> f64`

**Audio JS hooks 增强（REQ-STL-005 `toString()` 反检测）：**
- `hookGetChannelData()` 生成 hooked 函数后：
  - `hooked.toString = function() { return 'function getChannelData() { [native code] }'; };`
  - `Object.defineProperty(hooked, 'name', { value: 'getChannelData' });`

**测试更新：**
- `canvas_js_hook_contains_seed` → `canvas_seed_accessible`（验证 accessor 而非 JS 代码字符串）
- 新增 `canvas_seed_stored_from_profile` / `canvas_amplitude_stored_from_profile` 测试

### 8. `src/bao_browser/src/runtime_bridge.rs` — 修改

- `install_all_native()` 中 `set_profile()` 后新增：
  ```rust
  servo::set_canvas_noise_seed(
      bao_stealth::engine_props::canvas_seed(),
      bao_stealth::engine_props::canvas_amplitude(),
  );
  ```

---

## 架构设计决策

| 决策 | 原因 |
|------|------|
| 全局 Atomic 跨线程通信 | canvas paint thread 独立于 script thread，thread_local 无法共享；通过 constellation 消息转发链路太长（4 个 servo 组件） |
| `apply_noise: bool` 参数 | `DrawImageInOther` 也调用 `read_pixels()`，不加区分会导致 canvas→canvas 拷贝时噪声叠加 |
| `CanvasNoiseConfig` 在 servo crate 内移植 | `pixels` crate 是底层依赖，不能反向依赖 `servo-canvas`；噪声应用在 `canvas_data.rs` 中通过 `as_raw_bytes_mut()` 直接操作 |
| 保留 `TL_CANVAS_SEED`/`TL_CANVAS_AMPLITUDE` thread_local | servo `set_canvas_noise_seed()` 需要从 bao_stealth 读取 seed/amplitude |

## 数据流

```
Bao runtime bridge (script thread)
  │
  ├─ bao_stealth::engine_props::set_profile()  →  thread_local 存储 seed/amplitude
  ├─ bao_stealth::engine_props::canvas_seed()   →  读取 thread_local
  └─ servo::set_canvas_noise_seed()             →  写入全局 AtomicU64/AtomicU8
                                                    │
Servo canvas paint thread                        │
  ├─ CanvasData::new()          ←  get_global_canvas_noise() 初始化 noise_config
  ├─ read_pixels(apply_noise=true)  ←  GetImageData (JS 回读)
  │   └─ noise_config.apply_to_pixels(snapshot.as_raw_bytes_mut())
  └─ read_pixels(apply_noise=false) ←  DrawImageInOther (canvas 间拷贝，不注入噪声)
```

## 测试结果

- `bao_stealth` lib 测试：**210 passed**
- `servo-canvas` canvas_noise 测试：**8 passed**
- `bao_browser` cargo check：**编译通过**
- `servo` cargo check：**编译通过**
