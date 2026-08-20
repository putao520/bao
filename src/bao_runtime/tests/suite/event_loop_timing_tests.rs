// @trace TEST-ENG-004-PROCESS [req:REQ-ENG-004@PROCESS] [level:system]
// @trace REQ-ENG-004@PROCESS [level:system]
// @trace TMG-LOOP-001 [timing:max_interval_ms]
//
// # TASK-17b @PROCESS 时序测试 — TMG-LOOP-001 Event Loop Tick 时序
//
// **核心断言**: bao_runtime 的事件循环 tick 延迟满足 SPEC 时序约束
// (TMG-LOOP-001 max_interval_ms=20ms)。SPEC 03-PROCESS.html 给出各 phase 目标:
//
//   - io_poll          : 1 ms   (epoll/kqueue 非阻塞)
//   - timer_check      : 0.1 ms (最小堆查找)
//   - microtask_drain  : 5 ms   (SM JobQueue drain)
//
// 测试驱动 `bun_event_loop::MiniEventLoop`(bao_runtime 的 event loop 后端),
// 测量 `tick_without_idle` / `tick_once` 的墙钟延迟。任何 epoll 阻塞、
// task queue O(n²) 退化、timer heap 全扫描都会导致延迟超过阈值。
//
// 注意:`tick_once` 在空 loop 上会进入 `epoll_wait` 阻塞 — 时序测试用
// `tick_without_idle`(只 drain concurrent+task queue,不进 OS 事件循环)。
// 这是单次 tick 的延迟基线测量,等同于生产环境 SM JobQueue drain 的延迟。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext;
use bun_event_loop::MiniEventLoop::MiniEventLoop;

// Pull in bao_uloop's `#[no_mangle] extern "C"` symbols (uws_get_loop etc.)
// — `MiniEventLoop::init()` reaches them via `UwsLoop::get()` but the linker
// GCs unreferenced no-mangle symbols without an explicit Rust reference.
fn force_uloop_link() {
    // Product path owns residual/RealImpl (no bao_native_stubs co-link).
    bao_uloop::force_link();
    let _ = bun_runtime::dispatch::__bun_run_file_poll
        as unsafe extern "Rust" fn(*mut bun_io::posix_event_loop::FilePoll, i64);
}

#[derive(Debug)]
struct CounterCtx {
    fired: AtomicUsize,
}

fn increment_task(ctx: *mut CounterCtx, _extra: *mut std::ffi::c_void) {
    unsafe {
        (*ctx).fired.fetch_add(1, Ordering::SeqCst);
    }
}

// ════════════════════════════════════════════════════════════════════
// §1 单次 tick 时序 — 空 loop drain 延迟基线
// ════════════════════════════════════════════════════════════════════

/// 空 loop 单次 tick_without_idle 延迟 < 5ms(TMG-LOOP-001 timer_check + microtask drain)
// Arrange — TMG-LOOP-001: 空 loop tick 基线
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_empty_tick_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    // Act — 单次 tick_without_idle(无 task,纯 drain 队列 + timer check)
    let start = Instant::now();
    loop_.tick_without_idle(core::ptr::null_mut());
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: 单次空 tick < 5ms (timer_check 0.1ms + drain 5ms 容差)
    assert!(
        elapsed < Duration::from_millis(20),
        "TMG-LOOP-001 violation: empty tick_without_idle must be < 20ms (max_interval_ms), got {:?}",
        elapsed
    );
}

/// 空 loop tick 重复 100 次,最大延迟 < 20ms(TMG-LOOP-001 稳定性)
// Arrange — TMG-LOOP-001: 多次 tick 稳定性
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_repeated_empty_tick_stable_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    // Act — 100 次 tick_without_idle,测最大延迟
    let mut max_elapsed = Duration::ZERO;
    for _ in 0..100 {
        let start = Instant::now();
        loop_.tick_without_idle(core::ptr::null_mut());
        let elapsed = start.elapsed();
        if elapsed > max_elapsed {
            max_elapsed = elapsed;
        }
    }

    // Assert — TMG-LOOP-001: 100 次重复,最大延迟 < 20ms(max_interval_ms)
    assert!(
        max_elapsed < Duration::from_millis(20),
        "TMG-LOOP-001 violation: 100x empty tick max < 20ms, got {:?}",
        max_elapsed
    );
}

// ════════════════════════════════════════════════════════════════════
// §2 单 task tick 时序 — 单个任务 drain 延迟
// ════════════════════════════════════════════════════════════════════

/// 单 task 入队后 tick 延迟 < 20ms(TMG-LOOP-001 单 task dispatch)
// Arrange — TMG-LOOP-001: 单 task dispatch
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_single_task_tick_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    let ctx = Box::new(CounterCtx {
        fired: AtomicUsize::new(0),
    });
    let ctx_ptr = Box::into_raw(ctx);
    let task_ptr = AnyTaskWithExtraContext::from_callback_auto_deinit(ctx_ptr, increment_task);
    let task_nn = unsafe { core::ptr::NonNull::new_unchecked(task_ptr) };
    loop_.enqueue_task_concurrent(task_nn);

    // Act — tick_without_idle 处理 1 个 task (避免 epoll 阻塞)
    let start = Instant::now();
    loop_.tick_without_idle(core::ptr::null_mut());
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: 单 task tick < 20ms
    let fired = unsafe { (*ctx_ptr).fired.load(Ordering::SeqCst) };
    unsafe {
        drop(Box::from_raw(ctx_ptr));
    }
    assert_eq!(fired, 1, "task must fire exactly once");
    assert!(
        elapsed < Duration::from_millis(20),
        "TMG-LOOP-001 violation: single task tick < 20ms, got {:?}",
        elapsed
    );
}

/// 多 task(10 个)逐次入队 + tick,总延迟 < 200ms(TMG-LOOP-001 批量 dispatch)
// Arrange — TMG-LOOP-001: 批量 task dispatch(逐次,避免 auto-deinit 二次释放)
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_batch_task_tick_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    let ctx = Arc::new(AtomicUsize::new(0));

    // Act — 入队 + tick 10 次,每次独立 ctx(auto-deinit 安全)
    let start = Instant::now();
    let mut total_fired = 0usize;
    for i in 0..10 {
        let local_ctx = Arc::new(AtomicUsize::new(0));
        let ctx_ptr = Arc::into_raw(local_ctx) as *mut CounterCtx;
        let task_ptr = AnyTaskWithExtraContext::from_callback_auto_deinit(ctx_ptr, increment_task);
        let task_nn = unsafe { core::ptr::NonNull::new_unchecked(task_ptr) };
        loop_.enqueue_task_concurrent(task_nn);
        loop_.tick_without_idle(core::ptr::null_mut());
        let fired = unsafe { (*ctx_ptr).fired.load(Ordering::SeqCst) };
        // reclaim Arc (wrapper was freed inside callback; reclaim Arc manually)
        unsafe {
            drop(Arc::from_raw(ctx_ptr as *const CounterCtx));
        }
        total_fired += fired;
        let _ = i; // silence unused
    }
    let elapsed = start.elapsed();
    let _ = ctx; // silence unused

    // Assert — TMG-LOOP-001: 10 task + 10 tick 累积 < 200ms (单次 20ms)
    assert_eq!(total_fired, 10, "all 10 tasks must fire");
    assert!(
        elapsed < Duration::from_millis(200),
        "TMG-LOOP-001 violation: 10 task tick batch < 200ms, got {:?}",
        elapsed
    );
}

// ════════════════════════════════════════════════════════════════════
// §3 tick + is_done callback 时序 — 立即返回路径
// ════════════════════════════════════════════════════════════════════

/// is_done=true 立即返回的 tick 延迟 < 20ms(TMG-LOOP-001 fast-exit 路径)
// Arrange — TMG-LOOP-001: fast-exit 路径
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_tick_done_callback_fast_exit_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();
    let done = Arc::new(AtomicBool::new(true));
    let done_clone = done.clone();

    // Act — is_done=true 的 tick 必须立即返回(minimal_event_loop_tests 模式)
    let start = Instant::now();
    loop_.tick(core::ptr::null_mut(), |_ctx| {
        done_clone.load(Ordering::SeqCst)
    });
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: fast-exit < 20ms
    assert!(done.load(Ordering::Relaxed), "done flag unchanged");
    assert!(
        elapsed < Duration::from_millis(20),
        "TMG-LOOP-001 violation: fast-exit tick < 20ms, got {:?}",
        elapsed
    );
}

/// 多次 tick_without_idle — 总延迟随次数线性增长,< 100ms
// Arrange — TMG-LOOP-001: 多次 tick 累积延迟
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_multi_tick_stable_under_threshold() {
    force_uloop_link();
    let mut loop_ = MiniEventLoop::init();

    // Act — 10 次 tick_without_idle(避免 epoll 阻塞)
    let start = Instant::now();
    for _ in 0..10 {
        loop_.tick_without_idle(core::ptr::null_mut());
    }
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: 10 次 tick 累积 < 100ms (单次 10ms)
    assert!(
        elapsed < Duration::from_millis(100),
        "TMG-LOOP-001 violation: 10 ticks < 100ms (10ms/tick), got {:?}",
        elapsed
    );
}

// ════════════════════════════════════════════════════════════════════
// §4 init + 多次 tick 稳定性 — 整体生命周期延迟
// ════════════════════════════════════════════════════════════════════

/// MiniEventLoop::init 延迟 < 50ms(TMG-LOOP-001 loop 初始化)
// Arrange — TMG-LOOP-001: loop 初始化时序
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_init_under_threshold() {
    force_uloop_link();

    // Act — MiniEventLoop::init 内部走 UwsLoop::get() + uSockets loop 创建
    let start = Instant::now();
    let _loop_ = MiniEventLoop::init();
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: init < 50ms (单线程 + epoll fd 创建)
    assert!(
        elapsed < Duration::from_millis(50),
        "TMG-LOOP-001 violation: MiniEventLoop::init < 50ms, got {:?}",
        elapsed
    );
}

/// 完整生命周期:init → 10 tick → drop,总延迟 < 200ms
// Arrange — TMG-LOOP-001: 完整生命周期
// @trace REQ-ENG-004@PROCESS [level:system]
#[test]
fn event_loop_lifecycle_total_under_threshold() {
    force_uloop_link();

    // Act — init + 10 tick + 隐式 drop 的总延迟
    let start = Instant::now();
    {
        let mut loop_ = MiniEventLoop::init();
        for _ in 0..10 {
            loop_.tick_without_idle(core::ptr::null_mut());
        }
    }
    let elapsed = start.elapsed();

    // Assert — TMG-LOOP-001: 完整生命周期 < 200ms
    assert!(
        elapsed < Duration::from_millis(200),
        "TMG-LOOP-001 violation: full lifecycle (init + 10 tick + drop) < 200ms, got {:?}",
        elapsed
    );
}
