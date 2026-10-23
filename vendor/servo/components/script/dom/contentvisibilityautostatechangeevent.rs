/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ContentVisibilityAutoStateChangeEvent` from the CSS Containment
//! specification (REQ-BRW-051, Bao fork self-build — see
//! `ContentVisibilityAutoStateChangeEvent.webidl`).
//!
//! 渲染联动登记(REQ-BRW-051 C2 收敛的渲染半边):本类型只承载 DOM/事件
//! 面;「元素 computed `content-visibility: auto` 且与视口不相关 ⇒
//! `skipped = true`」的判定与派发点在 layout/渲染刷新面,登记为后续
//! 最小增面(接线点:渲染更新后逐文档检查自动态元素的 skipped 翻转,经
//! `ContentVisibilityAutoStateChangeEvent::new` + `upcast::<Event>` +
//! `event.set_trusted(true)` + `event.fire(cx, target.upcast::<EventTarget>())`
//! 派发——镜像 `dom::editcontext` 的 TextUpdateEvent 派发形)。

use dom_struct::dom_struct;
use js::context::JSContext;
use js::rust::HandleObject;
use script_bindings::inheritance::Castable;
use script_bindings::reflector::reflect_dom_object_with_proto;
use stylo_atoms::Atom;

use crate::dom::bindings::codegen::Bindings::ContentVisibilityAutoStateChangeEventBinding::{
    ContentVisibilityAutoStateChangeEventInit, ContentVisibilityAutoStateChangeEventMethods,
};
use crate::dom::bindings::codegen::Bindings::EventBinding::EventMethods;
use crate::dom::bindings::error::Fallible;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::element::Element;
use crate::dom::event::{Event, EventBubbles, EventCancelable};
use crate::dom::eventtarget::EventTarget;
use crate::dom::node::NodeTraits;
use crate::dom::types::Window;

/// <https://drafts.csswg.org/css-contain/#content-visibility-auto-state-change-event>
#[dom_struct]
pub(crate) struct ContentVisibilityAutoStateChangeEvent {
    event: Event,
    skipped: bool,
}

impl ContentVisibilityAutoStateChangeEvent {
    fn new_inherited(skipped: bool) -> ContentVisibilityAutoStateChangeEvent {
        ContentVisibilityAutoStateChangeEvent {
            event: Event::new_inherited(),
            skipped,
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        window: &Window,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        skipped: bool,
    ) -> DomRoot<ContentVisibilityAutoStateChangeEvent> {
        Self::new_with_proto(cx, window, None, type_, bubbles, cancelable, skipped)
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_proto(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: Atom,
        bubbles: EventBubbles,
        cancelable: EventCancelable,
        skipped: bool,
    ) -> DomRoot<ContentVisibilityAutoStateChangeEvent> {
        let event = Box::new(ContentVisibilityAutoStateChangeEvent::new_inherited(skipped));
        let event = reflect_dom_object_with_proto(cx, event, window, proto);
        {
            let event = event.upcast::<Event>();
            event.init_event(type_, bool::from(bubbles), bool::from(cancelable));
        }
        event
    }
}

impl ContentVisibilityAutoStateChangeEventMethods<crate::DomTypeHolder>
    for ContentVisibilityAutoStateChangeEvent
{
    /// <https://drafts.csswg.org/css-contain/#dom-contentvisibilityautostatechangeevent>
    fn Constructor(
        cx: &mut JSContext,
        window: &Window,
        proto: Option<HandleObject>,
        type_: DOMString,
        init: &ContentVisibilityAutoStateChangeEventInit,
    ) -> DomRoot<ContentVisibilityAutoStateChangeEvent> {
        Self::new_with_proto(
            cx,
            window,
            proto,
            Atom::from(type_),
            EventBubbles::from(init.parent.bubbles),
            EventCancelable::from(init.parent.cancelable),
            init.skipped,
        )
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    fn IsTrusted(&self) -> bool {
        self.event.IsTrusted()
    }

    /// <https://drafts.csswg.org/css-contain/#dom-contentvisibilityautostatechangeevent-skipped>
    fn Skipped(&self) -> bool {
        self.skipped
    }
}

/// 接线点(登记项,REQ-BRW-051 C2 渲染半边):`contentvisibilityautostatechange`
/// 的派发入口——元素的 `content-visibility: auto` skipped 态发生翻转时调用。
///
/// 调用方契约:渲染更新管线在 layout 完成「元素是否与用户相关(relevant to
/// the user)」判定后,对每个 skipped 态翻转的自动态元素调用本函数。当前
/// layout 未实现 content-visibility 相关性判定(css-contain 250 格 U0 主
/// 缺口),故本函数在树内暂零调用方;事件对象构造/trusted 位/派发链已全部
/// 就位,渲染侧接入即生效(非占位实现)。
#[allow(dead_code)]
pub(crate) fn fire_content_visibility_auto_state_change_event(
    cx: &mut JSContext,
    target: &Element,
    skipped: bool,
) {
    let window = target.owner_window();
    let event = ContentVisibilityAutoStateChangeEvent::new(
        cx,
        &window,
        Atom::from("contentvisibilityautostatechange"),
        EventBubbles::DoesNotBubble,
        EventCancelable::NotCancelable,
        skipped,
    );
    let event = event.upcast::<Event>();
    event.set_trusted(true);
    event.fire(cx, target.upcast::<EventTarget>());
}
