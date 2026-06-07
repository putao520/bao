# 反爬虫行为模拟升级方案 — SPEC 设计

> 目标：将现有简单行为模拟升级为真实人类行为级模拟，通过 creepjs/pixelscan 等行为指纹检测。
> 约束：纯 Rust、零外部依赖、seed-based 确定性、Firefox/Chrome 行为差异。

## 一、现有代码分析

**文件**: `src/bao_stealth/src/behavior.rs` (104 行)

| 功能 | 现有实现 | 问题 |
|------|---------|------|
| 鼠标路径 | 线性插值 + 随机抖动 + 简单二次贝塞尔 | 路径不自然，无速度曲线，不符合 Fitts' Law |
| 键盘延迟 | 均匀随机 30-150ms | 无节奏模式，无思考停顿，无退格修正 |
| 滚动增量 | 分三段（加速/匀速/减速）+ 10% 噪声 | 无惯性回弹，无精确停止 |

## 二、升级设计

### 2.1 核心数据结构

```rust
/// 行为模拟配置 — Firefox/Chrome 各有不同参数
#[derive(Debug, Clone)]
pub struct BehaviorConfig {
    /// 鼠标配置
    pub mouse: MouseConfig,
    /// 键盘配置
    pub keyboard: KeyboardConfig,
    /// 滚动配置
    pub scroll: ScrollConfig,
    /// 点击配置
    pub click: ClickConfig,
}

#[derive(Debug, Clone)]
pub struct MouseConfig {
    /// 贝塞尔曲线阶数 (2=二次, 3=三次)
    pub bezier_order: u8,
    /// 控制点偏移幅度 (像素)
    pub control_point_spread: f64,
    /// Fitts' Law 系数 a (截距, ms)
    pub fitts_a: f64,
    /// Fitts' Law 系数 b (斜率, ms/bit)
    pub fitts_b: f64,
    /// 最小移动时间 (ms)
    pub min_move_time_ms: f64,
    /// 最大移动时间 (ms)
    pub max_move_time_ms: f64,
    /// 路径抖动幅度 (像素)
    pub jitter_amplitude: f64,
    /// 微颤抖频率 (Hz)
    pub tremor_frequency: f64,
    /// 微颤抖幅度 (像素)
    pub tremor_amplitude: f64,
}

#[derive(Debug, Clone)]
pub struct KeyboardConfig {
    /// 基础按键间隔均值 (ms)
    pub base_interval_ms: f64,
    /// 按键间隔标准差 (ms)
    pub interval_stddev_ms: f64,
    /// 思考停顿概率 (每个词首字符)
    pub thinking_pause_probability: f64,
    /// 思考停顿均值 (ms)
    pub thinking_pause_mean_ms: f64,
    /// 退格修正概率
    pub typo_probability: f64,
    /// 退格后重打延迟 (ms)
    pub typo_correction_delay_ms: f64,
    /// 词间停顿均值 (ms)
    pub word_gap_mean_ms: f64,
    /// 标点后停顿均值 (ms)
    pub punctuation_pause_ms: f64,
}

#[derive(Debug, Clone)]
pub struct ScrollConfig {
    /// 初始速度 (px/step)
    pub initial_speed: f64,
    /// 加速度 (px/step²)
    pub acceleration: f64,
    /// 摩擦系数 (每步速度衰减比, 0.9-0.98)
    pub friction: f64,
    /// 惯性回弹概率
    pub overshoot_probability: f64,
    /// 回弹幅度比 (0.05-0.15)
    pub overshoot_ratio: f64,
    /// 最小速度阈值 (低于此值停止)
    pub stop_threshold: f64,
}

#[derive(Debug, Clone)]
pub struct ClickConfig {
    /// 按压时长均值 (ms)
    pub press_duration_mean_ms: f64,
    /// 按压时长标准差 (ms)
    pub press_duration_stddev_ms: f64,
    /// 按压前微移概率
    pub pre_click_move_probability: f64,
    /// 按压前微移幅度 (px)
    pub pre_click_move_amplitude: f64,
    /// 双击间隔均值 (ms)
    pub dbl_click_interval_mean_ms: f64,
    /// 双击间隔标准差 (ms)
    pub dbl_click_interval_stddev_ms: f64,
}
```

### 2.2 Firefox vs Chrome 行为差异

| 参数 | Firefox | Chrome | 依据 |
|------|---------|--------|------|
| 贝塞尔阶数 | 3 (三次) | 3 (三次) | 两者一致 |
| 控制点偏移 | 较大 (30-80px) | 较小 (20-50px) | Firefox 用户鼠标速度通常更不规则 |
| Fitts' b 系数 | 150 | 120 | Firefox 渲染延迟略高 |
| 按键间隔均值 | 95ms | 85ms | Chrome 用户整体打字稍快 |
| 退格概率 | 4% | 3% | 统计差异 |
| 滚动摩擦系数 | 0.94 | 0.92 | Chrome 平滑滚动更激进 |
| 按压时长均值 | 85ms | 75ms | Chrome 用户点击更快 |
| 微颤抖幅度 | 0.8px | 0.5px | Chrome 触控板优化更好 |

### 2.3 算法设计

#### 2.3.1 三次贝塞尔曲线鼠标路径

```
算法流程:
1. 根据 Fitts' Law 计算总移动时间 T = a + b * log2(D/W + 1)
   - D = 起点到终点距离
   - W = 目标元素宽度 (默认 20px)
2. 根据 T 计算步数 steps = T / sampling_interval (默认 8ms)
3. 生成 2 个随机控制点 (三次贝塞尔需要 P0,P1,P2,P3)
   - P1 = 起点 + (方向偏移 30-80px)
   - P2 = 终点 + (方向偏移 30-80px)
4. 沿贝塞尔曲线采样 steps+1 个点
5. 叠加微颤抖 (正弦波 + 随机噪声)
6. 叠加速度曲线 (缓入缓出 — ease-in-out)
```

**速度曲线 (ease-in-out)**:
```
velocity(t) = sin(π * t)  其中 t ∈ [0, 1]
position(t) = ∫₀ᵗ sin(π * s) ds = (1 - cos(π * t)) / 2
```

这产生自然的加速-匀速-减速行为，而不是均匀速度。

#### 2.3.2 三阶段点击模拟

```
点击序列:
1. mousemove → 到达目标附近 (贝塞尔路径)
2. mousemove → 微调到精确位置 (pre-click jitter, 0-3px)
3. mousedown → 按下
4. 等待 60-120ms (正态分布, μ=80ms, σ=15ms)
5. mouseup → 释放
6. 等待 0-50ms
7. click → 触发

双击:
8. 等待 200-400ms (正态分布, μ=300ms, σ=40ms)
9. 重复 3-7
10. dblclick → 触发
```

#### 2.3.3 人类节奏键盘输入

```
算法流程:
1. 将输入文本按字符遍历
2. 每个字符生成一个延迟:
   - 词首字符: 基础延迟 + 思考停顿 (概率触发, 200-500ms)
   - 空格后: 词间停顿 (100-200ms)
   - 标点后: 标点停顿 (150-300ms)
   - 普通字符: 正态分布延迟 (μ=90ms, σ=25ms)
3. 退格修正 (概率触发):
   - 插入 1-2 个错误字符
   - 退格键延迟 (50-100ms)
   - 修正字符延迟 (80-150ms)
4. 所有延迟通过 Box-Muller 变换生成正态分布
```

**Box-Muller 变换** (零依赖正态分布):
```rust
fn normal_random(state: &mut u64, mean: f64, stddev: f64) -> f64 {
    let u1 = next_random(state);
    let u2 = next_random(state);
    let z0 = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    mean + z0 * stddev
}
```

#### 2.3.4 惯性滚动

```
算法流程:
1. 初始速度 v₀ = initial_speed (正态分布, μ=30, σ=10)
2. 每步: v = v * friction + noise
3. 如果 v < stop_threshold → 停止
4. 惯性回弹 (概率触发):
   - 滚过目标 5-15%
   - 然后反向滚回 1-2 次
5. 输出每步的 deltaY 值
```

### 2.4 API 设计

```rust
impl BehaviorSimulator {
    // ---- 现有 API 保持兼容 ----
    pub fn generate_mouse_path(&self, x1: f64, y1: f64, x2: f64, y2: f64, steps: usize) -> Vec<(f64, f64)>;
    pub fn generate_typing_delays(&self, count: usize) -> Vec<u64>;
    pub fn generate_scroll_deltas(&self, total: f64, steps: usize) -> Vec<f64>;

    // ---- 新增 API ----

    /// 贝塞尔曲线鼠标路径 + Fitts' Law 速度曲线
    /// 自动计算步数，返回 (x, y, time_ms) 三元组
    pub fn generate_human_mouse_path(&self, start: (f64, f64), end: (f64, f64), target_width: f64) -> Vec<(f64, f64, f64)>;

    /// 三阶段点击序列
    /// 返回事件序列: [(event_type, x, y, delay_ms), ...]
    pub fn generate_click_sequence(&self, x: f64, y: f64, target_width: f64) -> Vec<ClickEvent>;

    /// 双击序列
    pub fn generate_double_click_sequence(&self, x: f64, y: f64, target_width: f64) -> Vec<ClickEvent>;

    /// 人类节奏键盘输入
    /// 返回带延迟的字符序列，包含可能的退格修正
    pub fn generate_human_typing(&self, text: &str) -> Vec<TypingEvent>;

    /// 惯性滚动序列
    /// 返回每帧 deltaY 值（自然衰减 + 可能的回弹）
    pub fn generate_inertia_scroll(&self, initial_speed: f64) -> Vec<f64>;
}

#[derive(Debug, Clone)]
pub struct ClickEvent {
    pub event_type: ClickEventType,
    pub x: f64,
    pub y: f64,
    pub delay_after_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClickEventType {
    MouseDown,
    MouseUp,
    Click,
    DoubleClick,
}

#[derive(Debug, Clone)]
pub struct TypingEvent {
    pub char: char,
    pub delay_before_ms: u64,
    pub is_backspace: bool,
}
```

### 2.5 行为事件输出 — 集成到 bao_browser

```rust
// 在 bao_browser 的 page.rs 中集成
impl PageHandle {
    /// 人类级鼠标移动 — 自动生成贝塞尔路径并逐步派发 mousemove 事件
    pub async fn human_move_to(&self, x: f64, y: f64, target_width: f64) -> Result<(), BrowserError>;

    /// 人类级点击 — 移动 + 微调 + 按下/释放
    pub async fn human_click(&self, x: f64, y: f64) -> Result<(), BrowserError>;

    /// 人类级键盘输入 — 带节奏的逐字输入
    pub async fn human_type(&self, text: &str) -> Result<(), BrowserError>;

    /// 人类级滚动 — 惯性滚动
    pub async fn human_scroll(&self, delta_y: f64) -> Result<(), BrowserError>;
}
```

## 三、测试策略

### 3.1 单元测试

| 测试类别 | 测试项 | 数量 |
|---------|--------|------|
| 贝塞尔曲线 | 曲线通过起止点、控制点影响形状、确定性 | 8 |
| Fitts' Law | 距离越远时间越长、目标越小时间越长、参数正确 | 5 |
| 速度曲线 | 缓入缓出、速度峰值在中段、起止速度≈0 | 4 |
| 微颤抖 | 幅度合理、频率合理、不偏离主路径 | 3 |
| 点击序列 | 三阶段完整、按压时长分布、双击间隔 | 6 |
| 键盘节奏 | 词间停顿、标点停顿、退格修正、确定性 | 7 |
| 惯性滚动 | 衰减收敛、回弹、总距离合理 | 5 |
| 跨 profile | Firefox≠Chrome 参数、行为指纹不同 | 4 |

### 3.2 反检测验证

| 检测维度 | 验证方法 |
|---------|---------|
| 路径自然度 | 曲率连续、无锐角、速度分布符合 Fitts' Law |
| 点击真实度 | 按压时长 60-120ms、位置微偏移 |
| 键盘节奏 | 延迟正态分布、变异系数 0.25-0.40 |
| 滚动物理 | 速度指数衰减、回弹幅度 5-15% |
| 统计不可区分 | 1000 次采样的均值/方差/偏度/峰度在人类范围内 |

## 四、实施计划

| 步骤 | 内容 | 涉及文件 |
|------|------|---------|
| 1 | 新增 BehaviorConfig + MouseConfig/KeyboardConfig/ScrollConfig/ClickConfig | behavior.rs |
| 2 | 实现 Box-Muller 正态分布 | behavior.rs |
| 3 | 重写 generate_human_mouse_path (三次贝塞尔 + Fitts' Law + 速度曲线) | behavior.rs |
| 4 | 新增 generate_click_sequence / generate_double_click_sequence | behavior.rs |
| 5 | 新增 generate_human_typing (节奏键盘 + 退格修正) | behavior.rs |
| 6 | 重写 generate_inertia_scroll (惯性滚动 + 回弹) | behavior.rs |
| 7 | Firefox/Chrome profile 注入不同 BehaviorConfig | profile.rs |
| 8 | 保持旧 API 向后兼容（内部委托到新算法） | behavior.rs |
| 9 | 编写全部单元测试 + 反检测验证测试 | tests/ |
| 10 | 更新 SPEC REQ-STL-006 验收标准 | .spec/ |

## 五、风险评估

| 风险 | 缓解措施 |
|------|---------|
| 行为模式被统计学习识别 | 每次运行使用不同 seed，正态分布足够宽 |
| 退格修正过于规律 | 退格概率、错误字符数、修正延迟都随机化 |
| Fitts' Law 参数不准确 | 使用学术界验证的参数 (Card et al. 1978) |
| 惯性滚动过于机械 | 摩擦系数 + 噪声 + 回弹组合产生自然衰减 |
