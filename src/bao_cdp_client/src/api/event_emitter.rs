//! EventEmitter — Playwright 风格事件订阅 trait + 单线程 Inner 实现。
//!
//! 高层 API 类(Browser/BrowserContext/Page/Frame/...)共享同一套事件订阅语义:
//! - `on(event, handler) -> HandlerId`:注册持久 handler
//! - `once(event, handler) -> HandlerId`:注册一次性 handler(触发后自动移除)
//! - `off(event, handler_id)`:按 ID 移除
//! - `remove_all_listeners(event)`:清空指定 event(或全部)
//! - `listener_count(event) -> usize`:计数
//! - `emit(event, args)`:同步触发所有 handler
//!
//! # 单线程模型
//!
//! `EventEmitterInner` 用 `Rc<RefCell<...>>` 而非 `Arc<Mutex<...>>`,与
//! servo `JSContext` 单线程模型一致(DEC-JSC-001:JSContext 寄生 servo,所有
//! JS API 在 servo 主线程调用)。线程安全由调用方保证(`Browser`/`Page` 等
//! 高层类型 `!Send + !Sync`)。
//!
//! @trace REQ-BAO-API-006 [level:library]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

/// Handler ID。全局递增(单线程内),用作 off() 索引。
///
/// @trace REQ-BAO-API-006 [level:library]
pub type HandlerId = u64;

/// 事件 handler 闭包。`Arc<dyn Fn>` 便于克隆传递(`Rc` 不可跨所有权)。
///
/// 实际上单线程下用 `Rc<dyn Fn>` 也行,但 `Arc` 更稳妥(允许 handler 内部
/// 持有跨线程资源,如日志 channel)。
///
/// @trace REQ-BAO-API-006 [level:library]
pub type EventHandler = Arc<dyn Fn(&[Value])>;

/// 事件订阅结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionResult {
    /// handler 已注册。
    Registered(HandlerId),
    /// handler 被移除(off/removeAllListeners)。
    Removed,
    /// handler 未找到(off 时 ID 不存在)。
    NotFound,
}

/// EventEmitter trait — 高层 API 类共享接口。
///
/// 所有实现必须是 `!Send`(单线程,servo JSContext 寄生约束)。
///
/// @trace REQ-BAO-API-006 [level:library]
pub trait EventEmitter {
    /// 注册持久 handler,返回 HandlerId(可用于 off)。
    fn on(&self, event: &str, handler: EventHandler) -> HandlerId;

    /// 注册一次性 handler(emit 后自动移除)。
    fn once(&self, event: &str, handler: EventHandler) -> HandlerId;

    /// 按 ID 移除 handler。返回 NotFound 时表示 ID 不存在。
    fn off(&self, event: &str, handler_id: HandlerId) -> SubscriptionResult;

    /// 移除指定事件的所有 handler;`event=None` 时清空所有事件的所有 handler。
    fn remove_all_listeners(&self, event: Option<&str>);

    /// 返回指定事件的 handler 数量。
    fn listener_count(&self, event: &str) -> usize;

    /// 同步触发事件,按注册顺序调用所有 handler。
    ///
    /// once handler 在调用后立即从内部列表移除。
    fn emit(&self, event: &str, args: &[Value]);
}

/// 单条 handler 记录。
#[derive(Clone)]
struct HandlerEntry {
    id: HandlerId,
    handler: EventHandler,
    once: bool,
}

/// EventEmitter 单线程 Inner — 用 `Rc<RefCell<...>>` 共享。
///
/// 高层类(`Browser` / `Page` 等)持有 `Rc<EventEmitterInner>` 并把
/// `EventEmitter` trait 委托到 Inner。
///
/// @trace REQ-BAO-API-006 [level:library]
pub struct EventEmitterInner {
    handlers: RefCell<HashMap<String, Vec<HandlerEntry>>>,
    next_id: RefCell<HandlerId>,
}

impl std::fmt::Debug for EventEmitterInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.handlers.borrow().len();
        f.debug_struct("EventEmitterInner")
            .field("event_count", &count)
            .finish()
    }
}

impl EventEmitterInner {
    /// 构造空 Inner。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn new() -> Self {
        Self {
            handlers: RefCell::new(HashMap::new()),
            next_id: RefCell::new(1),
        }
    }

    fn alloc_id(&self) -> HandlerId {
        let mut next = self.next_id.borrow_mut();
        let id = *next;
        *next += 1;
        id
    }

    fn register(&self, event: &str, handler: EventHandler, once: bool) -> HandlerId {
        let id = self.alloc_id();
        let entry = HandlerEntry { id, handler, once };
        self.handlers
            .borrow_mut()
            .entry(event.to_string())
            .or_default()
            .push(entry);
        id
    }

    /// 注册持久 handler。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn on(&self, event: &str, handler: EventHandler) -> HandlerId {
        self.register(event, handler, false)
    }

    /// 注册一次性 handler。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn once(&self, event: &str, handler: EventHandler) -> HandlerId {
        self.register(event, handler, true)
    }

    /// 按 ID 移除 handler。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn off(&self, event: &str, handler_id: HandlerId) -> SubscriptionResult {
        let mut map = self.handlers.borrow_mut();
        if let Some(list) = map.get_mut(event) {
            let before = list.len();
            list.retain(|e| e.id != handler_id);
            if list.len() < before {
                if list.is_empty() {
                    map.remove(event);
                }
                return SubscriptionResult::Removed;
            }
        }
        SubscriptionResult::NotFound
    }

    /// 清空指定事件或全部事件。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn remove_all_listeners(&self, event: Option<&str>) {
        let mut map = self.handlers.borrow_mut();
        match event {
            Some(name) => {
                map.remove(name);
            }
            None => map.clear(),
        }
    }

    /// 返回指定事件的 handler 数量。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn listener_count(&self, event: &str) -> usize {
        self.handlers
            .borrow()
            .get(event)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// 触发事件。同步调用所有 handler。once handler 在调用后移除。
    ///
    /// 注意:不能在持有 `borrow_mut` 期间回调 handler(可能再触发 emit → 死锁 / panic)。
    /// 因此先把 handler 闭包克隆出,释放 borrow 后再调用。
    ///
    /// @trace REQ-BAO-API-006 [level:library]
    pub fn emit(&self, event: &str, args: &[Value]) {
        // 1. 收集 (handler, once_flag, id) 三元组,克隆 handler。
        let to_call: Vec<(HandlerId, EventHandler, bool)> = {
            let map = self.handlers.borrow();
            match map.get(event) {
                Some(list) => list
                    .iter()
                    .map(|e| (e.id, e.handler.clone(), e.once))
                    .collect(),
                None => return,
            }
        };

        // 2. 收集需要移除的 once handler ID。
        let once_ids: Vec<HandlerId> = to_call
            .iter()
            .filter(|(_, _, o)| *o)
            .map(|(id, _, _)| *id)
            .collect();

        // 3. 调用所有 handler(此时未持有任何 borrow)。
        for (_, handler, _) in &to_call {
            handler(args);
        }

        // 4. 移除 once handler。
        if !once_ids.is_empty() {
            let mut map = self.handlers.borrow_mut();
            if let Some(list) = map.get_mut(event) {
                list.retain(|e| !once_ids.contains(&e.id));
                if list.is_empty() {
                    map.remove(event);
                }
            }
        }
    }
}

impl Default for EventEmitterInner {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 `EventEmitter` trait 委托给 `Rc<EventEmitterInner>` 的宏。
///
/// 高层类(`Browser` / `Page` / `Frame` / ...)用此宏避免重复样板。
///
/// @trace REQ-BAO-API-006 [level:library]
#[macro_export]
macro_rules! delegate_event_emitter {
    ($self:ident, $field:ident) => {
        fn on(
            &self,
            event: &str,
            handler: $crate::api::event_emitter::EventHandler,
        ) -> $crate::api::event_emitter::HandlerId {
            self.$field.on(event, handler)
        }
        fn once(
            &self,
            event: &str,
            handler: $crate::api::event_emitter::EventHandler,
        ) -> $crate::api::event_emitter::HandlerId {
            self.$field.once(event, handler)
        }
        fn off(
            &self,
            event: &str,
            handler_id: $crate::api::event_emitter::HandlerId,
        ) -> $crate::api::event_emitter::SubscriptionResult {
            self.$field.off(event, handler_id)
        }
        fn remove_all_listeners(&self, event: Option<&str>) {
            self.$field.remove_all_listeners(event)
        }
        fn listener_count(&self, event: &str) -> usize {
            self.$field.listener_count(event)
        }
        fn emit(&self, event: &str, args: &[serde_json::Value]) {
            self.$field.emit(event, args)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn counter_handler() -> (EventHandler, Rc<Cell<u32>>) {
        let counter = Rc::new(Cell::new(0u32));
        let c = counter.clone();
        let handler: EventHandler = Arc::new(move |_args: &[Value]| {
            c.set(c.get() + 1);
        });
        (handler, counter)
    }

    #[test]
    fn on_registers_and_emit_invokes() {
        let inner = EventEmitterInner::new();
        let (h, counter) = counter_handler();
        let id = inner.on("test", h);
        assert!(id > 0);
        assert_eq!(inner.listener_count("test"), 1);
        inner.emit("test", &[]);
        assert_eq!(counter.get(), 1);
        // on 是持久,再 emit 仍触发
        inner.emit("test", &[]);
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn once_invoked_only_once() {
        let inner = EventEmitterInner::new();
        let (h, counter) = counter_handler();
        inner.once("boom", h);
        assert_eq!(inner.listener_count("boom"), 1);
        inner.emit("boom", &[]);
        assert_eq!(counter.get(), 1);
        assert_eq!(inner.listener_count("boom"), 0);
        inner.emit("boom", &[]);
        assert_eq!(counter.get(), 1);
    }

    #[test]
    fn off_removes_handler() {
        let inner = EventEmitterInner::new();
        let (h1, c1) = counter_handler();
        let (h2, c2) = counter_handler();
        let id1 = inner.on("e", h1);
        let _id2 = inner.on("e", h2);
        assert_eq!(inner.listener_count("e"), 2);
        let res = inner.off("e", id1);
        assert_eq!(res, SubscriptionResult::Removed);
        assert_eq!(inner.listener_count("e"), 1);
        inner.emit("e", &[]);
        assert_eq!(c1.get(), 0);
        assert_eq!(c2.get(), 1);
    }

    #[test]
    fn off_unknown_returns_not_found() {
        let inner = EventEmitterInner::new();
        let res = inner.off("e", 999);
        assert_eq!(res, SubscriptionResult::NotFound);
    }

    #[test]
    fn remove_all_listeners_specific_event() {
        let inner = EventEmitterInner::new();
        let (h, _) = counter_handler();
        inner.on("a", h.clone());
        inner.on("b", h);
        assert_eq!(inner.listener_count("a"), 1);
        assert_eq!(inner.listener_count("b"), 1);
        inner.remove_all_listeners(Some("a"));
        assert_eq!(inner.listener_count("a"), 0);
        assert_eq!(inner.listener_count("b"), 1);
    }

    #[test]
    fn remove_all_listeners_all_events() {
        let inner = EventEmitterInner::new();
        let (h, _) = counter_handler();
        inner.on("a", h.clone());
        inner.on("b", h);
        inner.remove_all_listeners(None);
        assert_eq!(inner.listener_count("a"), 0);
        assert_eq!(inner.listener_count("b"), 0);
    }

    #[test]
    fn listener_count_zero_for_unknown_event() {
        let inner = EventEmitterInner::new();
        assert_eq!(inner.listener_count("nope"), 0);
    }

    #[test]
    fn emit_unknown_event_noop() {
        let inner = EventEmitterInner::new();
        // Should not panic
        inner.emit("nope", &[]);
    }

    #[test]
    fn multiple_handlers_called_in_order() {
        let inner = EventEmitterInner::new();
        let order = Rc::new(RefCell::new(Vec::<u32>::new()));
        let o1 = order.clone();
        let h1: EventHandler = Arc::new(move |_| o1.borrow_mut().push(1));
        let o2 = order.clone();
        let h2: EventHandler = Arc::new(move |_| o2.borrow_mut().push(2));
        let o3 = order.clone();
        let h3: EventHandler = Arc::new(move |_| o3.borrow_mut().push(3));
        inner.on("seq", h1);
        inner.on("seq", h2);
        inner.on("seq", h3);
        inner.emit("seq", &[]);
        assert_eq!(*order.borrow(), vec![1, 2, 3]);
    }

    #[test]
    fn emit_handler_can_emit_recursively() {
        // 验证 handler 内部 emit 同一事件不会死锁(borrow 已释放)。
        let inner = Rc::new(EventEmitterInner::new());
        let counter = Rc::new(Cell::new(0u32));
        let inner_clone = inner.clone();
        let counter_clone = counter.clone();
        let h: EventHandler = Arc::new(move |_args: &[Value]| {
            let c = counter_clone.get();
            counter_clone.set(c + 1);
            if c < 2 {
                inner_clone.emit("recurse", &[]);
            }
        });
        inner.on("recurse", h);
        inner.emit("recurse", &[]);
        assert_eq!(counter.get(), 3);
    }

    #[test]
    fn args_passed_through() {
        let inner = EventEmitterInner::new();
        let captured = Rc::new(RefCell::new(Vec::<Value>::new()));
        let cap = captured.clone();
        let h: EventHandler = Arc::new(move |args: &[Value]| {
            *cap.borrow_mut() = args.to_vec();
        });
        inner.on("args", h);
        inner.emit("args", &[Value::from(42), Value::from("hi")]);
        assert_eq!(captured.borrow().len(), 2);
        assert_eq!(captured.borrow()[0], 42);
        assert_eq!(captured.borrow()[1], "hi");
    }
}
