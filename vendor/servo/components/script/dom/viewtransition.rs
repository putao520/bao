/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `ViewTransition` — the CSS View Transitions same-document state machine.
//!
//! BAO PATCH (REQ-BRW-051 C2): self-built. Neither servo nor its upstream has
//! any implementation of the View Transitions API (the bclass-scope report §1
//! records 373 `css/css-view-transitions` WPT grid cells as a U0 family:
//! `ViewTransition.webidl` exists on neither tree), while Chromium ships the
//! API enabled by default — so the absence is itself a fingerprinting vector.
//!
//! Spec: <https://drafts.csswg.org/css-view-transitions-1/> (§API,
//! §Algorithms) plus the Level 2 single-document members
//! (`types`, `waitUntil()`; <https://drafts.csswg.org/css-view-transitions-2/>).
//!
//! ## What is real in this landing
//!
//! * The complete phase machine — `pending-capture` → `update-callback-called`
//!   → `animating` → `done` — with the spec's exact promise settlement order:
//!   `updateCallbackDone` settles with the update callback's promise, `ready`
//!   fulfils only once the pseudo-elements exist, `finished` settles by
//!   reacting to `updateCallbackDone` (and is marked as handled at creation so
//!   a callback rejection produces exactly one `unhandledrejection`).
//! * The per-document update-callback queue ("flush the update callback
//!   queue"), `skipTransition()`, the page-visibility change steps, the
//!   rendering-suppression flag, `types` and `waitUntil()`.
//! * `document.activeViewTransition` lifetime: set synchronously by
//!   `startViewTransition()`, cleared synchronously by skip/clear.
//!
//! ## What is registered, not faked
//!
//! **Snapshot capture.** `capture the image` needs a rendering-pipeline
//! interface this runtime does not have yet: script would have to ask layout
//! for an element's ink-overflow image as a WebRender image key, plus its
//! border box, in paint order (`capture the old state` step 7 needs the whole
//! connected element set in paint order). Until that interface exists,
//! [`ViewTransition::capture_old_state`] fails closed with
//! [`ViewTransitionCaptureError::Unsupported`], and `setup view transition`
//! turns that into the spec's own failure path: *skip the view transition*
//! with an `InvalidStateError`.
//!
//! Consequences, which are explicitly degraded rather than silently wrong:
//! the update callback always runs, `updateCallbackDone` settles normally,
//! `ready` rejects, `finished` settles with `updateCallbackDone`, and
//! `document.activeViewTransition` is cleared. This is exactly the spec's
//! "transition as enhancement" contract — a failed transition never blocks the
//! DOM change — and it is the honest state of the feature until the capture
//! seam is wired. The layout-side half of that seam lives in
//! `components/layout/view_transition.rs`.
//!
//! **Pseudo-element tree.** `::view-transition`, `::view-transition-group()`,
//! `::view-transition-image-pair()`, `::view-transition-old()` and
//! `::view-transition-new()` require entries in the *stylo* pseudo-element
//! table (`vendor/stylo/servo/selector_parser.rs`), which is read-only for
//! this wave; the required entries are listed in the layout module. Until then
//! no pseudo-element tree exists, hence no animations, hence
//! [`ViewTransition::handle_transition_frame`]'s `hasActiveAnimations` is
//! always false — which the spec answers by finishing the transition.

use std::cell::{Cell, RefCell};
use std::ops::DerefMut;

use dom_struct::dom_struct;
use js::context::JSContext;
use js::conversions::{ConversionResult, FromJSValConvertible};
use js::jsapi::IsCallable;
use js::jsval::UndefinedValue;
use js::realm::CurrentRealm;
use js::rust::HandleValue;
use script_bindings::callback::ExceptionHandling;
use script_bindings::reflector::{Reflector, reflect_dom_object};
use script_bindings::root::DomRoot;

use crate::dom::RootedPromise;
use crate::dom::bindings::callback::TracedCallback;
use crate::dom::bindings::codegen::Bindings::ViewTransitionBinding::{
    ViewTransitionMethods, ViewTransitionUpdateCallback,
};
use crate::dom::bindings::conversions::{StringificationBehavior, get_property_jsval};
use crate::dom::bindings::error::{Error, Fallible};
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::Dom;
use crate::dom::bindings::str::DOMString;
use crate::dom::document::{Document, RenderingUpdateReason};
use crate::dom::promise::{Promise, TracedPromise};
use crate::dom::promisenativehandler::{Callback, PromiseNativeHandler};
use crate::dom::viewtransitiontypeset::ViewTransitionTypeSet;

/// The [`ViewTransition`] phase, in spec order.
///
/// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-phase>
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ViewTransitionPhase {
    PendingCapture,
    UpdateCallbackCalled,
    Animating,
    Done,
}

/// Why a snapshot capture step could not run.
///
/// The spec's capture steps return "failure" for a small set of conditions
/// (snapshot containing block too large, duplicate `view-transition-name`,
/// fragmented box). `Unsupported` is the extra condition this runtime reports
/// while the rendering-pipeline capture interface is missing — it is a
/// fail-closed dependency absence, not a fabricated failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewTransitionCaptureError {
    /// No per-element snapshot interface in the rendering pipeline yet.
    Unsupported,
}

/// <https://drafts.csswg.org/css-view-transitions-2/#the-viewtransition-interface>
#[dom_struct]
pub(crate) struct ViewTransition {
    reflector_: Reflector,

    /// The document whose rendering this transition captures. For transitions
    /// started on the document (this landing's only entry point) this is the
    /// document, whose root element is the transition root.
    document: Dom<Document>,

    /// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-update-callback>
    update_callback: RefCell<Option<TracedCallback<ViewTransitionUpdateCallback>>>,

    /// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-update-callback-done-promise>
    update_callback_done_promise: TracedPromise,

    /// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-ready-promise>
    ready_promise: TracedPromise,

    /// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-finished-promise>
    finished_promise: TracedPromise,

    /// <https://drafts.csswg.org/css-view-transitions-1/#viewtransition-phase>
    #[no_trace]
    #[ignore_malloc_size_of = "the phase is a plain enum"]
    phase: Cell<ViewTransitionPhase>,

    /// <https://drafts.csswg.org/css-view-transitions-2/#viewtransition-active-types>
    types: Dom<ViewTransitionTypeSet>,

    /// <https://drafts.csswg.org/css-view-transitions-2/#viewtransition-wait-until-promise-count>
    wait_until_promise_count: Cell<u32>,

    /// Whether `finished` has been wired to react to `updateCallbackDone`.
    /// The spec settles `finished` exactly once; this guards the reaction
    /// against a second attachment (e.g. skip after skip).
    finished_reaction_attached: Cell<bool>,
}

impl ViewTransition {
    fn new_inherited(
        document: &Document,
        update_callback: Option<TracedCallback<ViewTransitionUpdateCallback>>,
        update_callback_done_promise: TracedPromise,
        ready_promise: TracedPromise,
        finished_promise: TracedPromise,
        types: &ViewTransitionTypeSet,
    ) -> Self {
        ViewTransition {
            reflector_: Reflector::new(),
            document: Dom::from_ref(document),
            update_callback: RefCell::new(update_callback),
            update_callback_done_promise,
            ready_promise,
            finished_promise,
            phase: Cell::new(ViewTransitionPhase::PendingCapture),
            types: Dom::from_ref(types),
            wait_until_promise_count: Cell::new(0),
            finished_reaction_attached: Cell::new(false),
        }
    }

    /// Create a `ViewTransition` in the realm of `document`, with its three
    /// promises already created ("Initially a new promise in this's relevant
    /// Realm") and `finished` marked as handled, as the spec requires.
    pub(crate) fn new(
        cx: &mut JSContext,
        document: &Document,
        update_callback: Option<TracedCallback<ViewTransitionUpdateCallback>>,
        types: Option<Vec<DOMString>>,
    ) -> DomRoot<ViewTransition> {
        let global = document.global();
        let update_callback_done_promise = Promise::new(cx, &global).to_traced();
        let ready_promise = Promise::new(cx, &global).to_traced();
        let finished_promise = Promise::new(cx, &global);
        // Spec: "finished promise ... Initially a new promise ... marked as
        // handled", so a rejected update callback reports exactly one
        // `unhandledrejection` (from `updateCallbackDone`).
        finished_promise.set_promise_is_handled(cx);
        let types = ViewTransitionTypeSet::new(cx, &global, types);

        reflect_dom_object(
            cx,
            Box::new(ViewTransition::new_inherited(
                document,
                update_callback,
                update_callback_done_promise,
                ready_promise,
                finished_promise.to_traced(),
                &types,
            )),
            &*global,
        )
    }

    pub(crate) fn document(&self) -> DomRoot<Document> {
        DomRoot::from_ref(&*self.document)
    }

    pub(crate) fn phase(&self) -> ViewTransitionPhase {
        self.phase.get()
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#setup-view-transition-algorithm>
    pub(crate) fn setup_view_transition(&self, cx: &mut JSContext) {
        let document = self.document();
        // Step 2. Flush the update callback queue, so that DOM changes
        // scheduled by other (skipped) transitions land before the old state
        // is captured.
        document.flush_view_transition_update_callback_queue(cx);

        // Step 3. Capture the old state. Failure is the documented degraded
        // path of this landing (see the module docs).
        if self.capture_old_state(cx).is_err() {
            self.skip_view_transition_with_error(
                cx,
                Error::InvalidState(Some(
                    "the rendering pipeline cannot capture view transition snapshots yet"
                        .to_owned(),
                )),
            );
            return;
        }

        // Step 4. Rendering is suppressed for the duration of the capture
        // window. The flag is a real document slot; the renderer's use of it
        // is part of the registered capture seam.
        document.set_rendering_suppression_for_view_transitions(true);

        // Step 5. Queue a global task on the DOM manipulation task source: the
        // texture read back in `capture the image` may be async.
        let transition = Trusted::new(self);
        document
            .task_manager()
            .dom_manipulation_task_source()
            .queue(task!(view_transition_update_callback: move |cx| {
                let transition = transition.root();
                if transition.phase.get() == ViewTransitionPhase::Done {
                    // Skipped before the task ran.
                    return;
                }
                let document = transition.document();
                document.push_view_transition_update_callback(&transition);
                document.flush_view_transition_update_callback_queue(cx);
            }));
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#capture-old-state-algorithm>
    fn capture_old_state(&self, _cx: &mut JSContext) -> Result<(), ViewTransitionCaptureError> {
        // Steps 1-4 build `namedElements` by walking every connected element of
        // the document in paint order, reading each element's document-scoped
        // `view-transition-name` and rejecting duplicates. Step 7 then maps each
        // captured element to a snapshot image of its ink overflow rectangle.
        //
        // None of that is reachable from script today: paint order, border
        // boxes and the image of an element's ink overflow all come from the
        // layout/paint pipeline. The layout-side half of that interface is
        // `layout::view_transition` (the pseudo tree model, the UA stylesheet
        // and the per-name dynamic style generation); wiring it is the
        // remaining step. Until then this fails closed.
        Err(ViewTransitionCaptureError::Unsupported)
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#capture-new-state-algorithm>
    fn capture_new_state(&self, _cx: &mut JSContext) -> Result<(), ViewTransitionCaptureError> {
        Err(ViewTransitionCaptureError::Unsupported)
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#call-dom-update-callback-algorithm>
    pub(crate) fn call_update_callback(&self, cx: &mut JSContext) {
        // Step 2. The phase moves forward unless the transition is already done.
        if self.phase.get() != ViewTransitionPhase::Done {
            self.phase.set(ViewTransitionPhase::UpdateCallbackCalled);
        }

        let global = self.global();
        let callback_promise = match self.update_callback.borrow().clone() {
            // Steps 4-5: no callback means "a promise resolved with undefined".
            None => Promise::new_resolved(cx, &global, ()),
            // Steps 4-5: otherwise invoke the callback. `Call__` invokes with an
            // undefined `this`, which is what "invoke a callback function" does
            // for a callback with no callback-this value.
            Some(callback) => {
                let callback = callback.root(cx);
                callback
                    .Call__(cx, ExceptionHandling::Rethrow)
                    .unwrap_or_else(|error| {
                        let promise = Promise::new(cx, &global);
                        promise.reject_error(cx, error);
                        promise
                    })
            },
        };

        // Steps 6-9: react with the fulfill and reject steps.
        let handler = PromiseNativeHandler::new(
            cx,
            &global,
            Some(Box::new(UpdateCallbackFulfilledHandler {
                transition: Dom::from_ref(self),
            })),
            Some(Box::new(UpdateCallbackRejectedHandler {
                transition: Dom::from_ref(self),
            })),
        );
        let mut realm = CurrentRealm::assert(cx);
        callback_promise.append_native_handler(&mut realm, &handler);
    }

    /// The `fulfillSteps` of "call the update callback".
    fn update_callback_fulfilled(&self, cx: &mut JSContext) {
        let promise = self.update_callback_done_promise.root(cx);
        promise.resolve_native(cx, &());
        self.activate_view_transition(cx);
    }

    /// The `rejectSteps` of "call the update callback".
    fn update_callback_rejected(&self, cx: &mut JSContext, reason: HandleValue) {
        let update_callback_done = self.update_callback_done_promise.root(cx);
        update_callback_done.reject(cx, reason);
        if self.phase.get() == ViewTransitionPhase::Done {
            return;
        }
        // `updateCallbackDone` provides the `unhandledrejection`; mark `ready`
        // handled so the same rejection is not reported twice.
        let ready = self.ready_promise.root(cx);
        ready.set_promise_is_handled(cx);
        self.skip_view_transition_with_value(cx, reason);
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#activate-view-transition>
    fn activate_view_transition(&self, cx: &mut JSContext) {
        if self.phase.get() == ViewTransitionPhase::Done {
            return;
        }

        let document = self.document();
        // Step 2. Rendering is no longer suppressed.
        document.set_rendering_suppression_for_view_transitions(false);

        // Step 3 compares the initial snapshot containing block size with the
        // current one. Both come from layout and are part of the registered
        // capture seam.

        // Step 4. Capture the new state.
        if self.capture_new_state(cx).is_err() {
            self.skip_view_transition_with_error(
                cx,
                Error::InvalidState(Some(
                    "the rendering pipeline cannot capture view transition snapshots yet"
                        .to_owned(),
                )),
            );
            return;
        }

        // Steps 5-7 (mark captured elements, set up the pseudo-element tree,
        // update the pseudo-element styles) need the stylo pseudo-element
        // entries listed in `layout::view_transition`; they are unreachable
        // while `capture_new_state` fails closed.

        // Step 8. Phase = "animating".
        self.phase.set(ViewTransitionPhase::Animating);

        // Step 9. Resolve the ready promise.
        let ready = self.ready_promise.root(cx);
        ready.resolve_native(cx, &());
    }

    /// <https://drafts.csswg.org/css-view-transitions-2/#handle-transition-frame-algorithm>
    pub(crate) fn handle_transition_frame(&self, cx: &mut JSContext) {
        // Step 2. `hasActiveAnimations` walks the inclusive descendants of the
        // `::view-transition` pseudo-element. This landing has no
        // pseudo-element tree (its stylo registration is pending), hence no
        // animations, hence `hasActiveAnimations` is false.
        let has_active_animations = false;

        // Step 4. Finish when nothing is animating and no `waitUntil()`
        // promise remains.
        if !has_active_animations && self.wait_until_promise_count.get() == 0 {
            // Steps 4.1-4.3.
            self.phase.set(ViewTransitionPhase::Done);
            self.document().clear_active_view_transition(self);
            let finished = self.finished_promise.root(cx);
            finished.resolve_native(cx, &());
            return;
        }

        // Step 6 (snapshot containing block size change) and step 8 (update
        // pseudo-element styles) need layout data; they belong to the
        // registered capture seam and are not approximated here.
    }

    /// Skip this transition from a document-side algorithm (the page-visibility
    /// change steps), rejecting `ready` with `error`.
    pub(crate) fn skip_from_document(&self, cx: &mut JSContext, error: Error) {
        if self.phase.get() != ViewTransitionPhase::Done {
            self.skip_view_transition_with_error(cx, error);
        }
    }

    /// Skip the view transition, rejecting `ready` with a DOM exception.
    ///
    /// <https://drafts.csswg.org/css-view-transitions-1/#skip-the-view-transition-algorithm>
    fn skip_view_transition_with_error(&self, cx: &mut JSContext, error: Error) {
        self.schedule_update_callback_if_needed();
        self.finish_skip_view_transition(cx, |ready, cx| ready.reject_error(cx, error));
    }

    /// Skip the view transition, rejecting `ready` with an arbitrary JS value
    /// (the rejection reason of the update callback).
    fn skip_view_transition_with_value(&self, cx: &mut JSContext, reason: HandleValue) {
        self.schedule_update_callback_if_needed();
        self.finish_skip_view_transition(cx, |ready, cx| ready.reject(cx, reason));
    }

    /// Step 3 of "skip the view transition": the update callback always runs,
    /// so schedule it if it has not been called yet.
    fn schedule_update_callback_if_needed(&self) {
        if self.phase.get() < ViewTransitionPhase::UpdateCallbackCalled {
            self.schedule_update_callback();
        }
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#schedule-dom-update-callback>
    fn schedule_update_callback(&self) {
        let document = self.document();
        document.push_view_transition_update_callback(self);
        let document = Trusted::new(&*document);
        // Step 2: queue a global task on the DOM manipulation task source to
        // flush the update callback queue.
        self.document()
            .task_manager()
            .dom_manipulation_task_source()
            .queue(
                task!(flush_view_transition_update_callback_queue: move |cx| {
                    document.root().flush_view_transition_update_callback_queue(cx);
                }),
            );
    }

    /// Steps 4-8 of "skip the view transition".
    fn finish_skip_view_transition(
        &self,
        cx: &mut JSContext,
        reject_ready: impl FnOnce(&RootedPromise, &mut JSContext),
    ) {
        let document = self.document();
        // Step 4.
        document.set_rendering_suppression_for_view_transitions(false);
        // Step 5.
        document.clear_active_view_transition(self);
        // Step 6.
        self.phase.set(ViewTransitionPhase::Done);
        // Step 7. (A no-op if `ready` was already resolved.)
        let ready = self.ready_promise.root(cx);
        reject_ready(&ready, cx);
        // Step 8. Resolve `finished` with the result of reacting to
        // `updateCallbackDone`: a rejection propagates unchanged.
        self.attach_finished_reaction(cx);
    }

    /// Wire `finished` to react to `updateCallbackDone` (once per transition).
    fn attach_finished_reaction(&self, cx: &mut JSContext) {
        if self.finished_reaction_attached.replace(true) {
            return;
        }
        let global = self.global();
        let handler = PromiseNativeHandler::new(
            cx,
            &global,
            Some(Box::new(FinishedFulfilledHandler {
                transition: Dom::from_ref(self),
            })),
            Some(Box::new(FinishedRejectedHandler {
                transition: Dom::from_ref(self),
            })),
        );
        let update_callback_done = self.update_callback_done_promise.root(cx);
        let mut realm = CurrentRealm::assert(cx);
        update_callback_done.append_native_handler(&mut realm, &handler);
    }
}

/// `fulfillSteps` of "call the update callback".
#[derive(Clone, JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct UpdateCallbackFulfilledHandler {
    transition: Dom<ViewTransition>,
}

impl Callback for UpdateCallbackFulfilledHandler {
    fn callback(&self, cx: &mut CurrentRealm, _value: HandleValue) {
        let transition = self.transition.as_rooted();
        transition.update_callback_fulfilled(cx.deref_mut());
    }
}

/// `rejectSteps` of "call the update callback".
#[derive(Clone, JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct UpdateCallbackRejectedHandler {
    transition: Dom<ViewTransition>,
}

impl Callback for UpdateCallbackRejectedHandler {
    fn callback(&self, cx: &mut CurrentRealm, value: HandleValue) {
        let transition = self.transition.as_rooted();
        transition.update_callback_rejected(cx.deref_mut(), value);
    }
}

/// Reaction attached to `updateCallbackDone` that fulfills `finished`.
#[derive(Clone, JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct FinishedFulfilledHandler {
    transition: Dom<ViewTransition>,
}

impl Callback for FinishedFulfilledHandler {
    fn callback(&self, cx: &mut CurrentRealm, _value: HandleValue) {
        let transition = self.transition.as_rooted();
        let cx = cx.deref_mut();
        let finished = transition.finished_promise.root(cx);
        finished.resolve_native(cx, &());
    }
}

/// Reaction attached to `updateCallbackDone` that rejects `finished` with the
/// same reason.
#[derive(Clone, JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct FinishedRejectedHandler {
    transition: Dom<ViewTransition>,
}

impl Callback for FinishedRejectedHandler {
    fn callback(&self, cx: &mut CurrentRealm, value: HandleValue) {
        let transition = self.transition.as_rooted();
        let cx = cx.deref_mut();
        let finished = transition.finished_promise.root(cx);
        finished.reject(cx, value);
    }
}

/// Settlement reaction for `waitUntil()` promises: decrement the wait-until
/// promise count and ask for another rendering opportunity when it reaches
/// zero while the transition is animating.
#[derive(Clone, JSTraceable, MallocSizeOf)]
#[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
struct WaitUntilPromiseSettledHandler {
    transition: Dom<ViewTransition>,
}

impl Callback for WaitUntilPromiseSettledHandler {
    fn callback(&self, _cx: &mut CurrentRealm, _value: HandleValue) {
        let transition = self.transition.as_rooted();
        let count = transition.wait_until_promise_count.get().saturating_sub(1);
        transition.wait_until_promise_count.set(count);
        if count == 0 && transition.phase.get() == ViewTransitionPhase::Animating {
            // "If the wait until promise count is 0, the transition will
            // finish at the next rendering opportunity."
            transition.document().add_rendering_update_reason(
                RenderingUpdateReason::ViewTransitionPendingOperations,
            );
        }
    }
}

/// Resolve the `startViewTransition()` argument, which the spec types as
/// `(ViewTransitionUpdateCallback or StartViewTransitionOptions)`.
///
/// Union conversion order: a callable converts to the callback member, an
/// object that is not callable converts to the dictionary member, and
/// `undefined`/`null` become empty options. Returns the traced update callback
/// (if any) and the `types` sequence (if any).
#[expect(unsafe_code)]
pub(crate) fn parse_start_view_transition_options(
    cx: &mut JSContext,
    value: HandleValue,
) -> Fallible<(
    Option<TracedCallback<ViewTransitionUpdateCallback>>,
    Option<Vec<DOMString>>,
)> {
    if value.is_undefined() || value.is_null() {
        return Ok((None, None));
    }
    if !value.is_object() {
        return Err(Error::Type(
            c"View transition options must be a callback function or an object".to_owned(),
        ));
    }

    rooted!(&in(cx) let object = value.to_object());
    if unsafe { IsCallable(object.get()) } {
        // SAFETY: the object was just verified to be callable; the callback
        // container installs the permanent root that `to_traced()` reuses.
        let callback = unsafe { ViewTransitionUpdateCallback::new(cx, object.get()) };
        return Ok((Some(callback.to_traced()), None));
    }

    // Dictionary conversion: both members are optional, and `undefined` (or an
    // absent property) leaves them at their defaults.
    let mut update_callback = None;
    rooted!(&in(cx) let mut update = UndefinedValue());
    get_property_jsval(cx, object.handle(), c"update", update.handle_mut())?;
    if !update.is_undefined() && !update.is_null() {
        if !update.is_object() || !unsafe { IsCallable(update.to_object()) } {
            return Err(Error::Type(
                c"StartViewTransitionOptions.update is not callable".to_owned(),
            ));
        }
        // SAFETY: see above; `update` is a callable object.
        let callback = unsafe { ViewTransitionUpdateCallback::new(cx, update.to_object()) };
        update_callback = Some(callback.to_traced());
    }

    let mut types = None;
    rooted!(&in(cx) let mut types_value = UndefinedValue());
    get_property_jsval(cx, object.handle(), c"types", types_value.handle_mut())?;
    if !types_value.is_undefined() && !types_value.is_null() {
        match Vec::<DOMString>::from_jsval(
            cx,
            types_value.handle(),
            StringificationBehavior::Default,
        ) {
            Ok(ConversionResult::Success(values)) => types = Some(values),
            Ok(ConversionResult::Failure(error)) => {
                return Err(Error::Type(error.into_owned()));
            },
            Err(()) => return Err(Error::JSFailed),
        }
    }

    Ok((update_callback, types))
}

impl ViewTransitionMethods<crate::DomTypeHolder> for ViewTransition {
    /// <https://drafts.csswg.org/css-view-transitions-1/#dom-viewtransition-updatecallbackdone>
    fn UpdateCallbackDone(&self, cx: &mut JSContext) -> RootedPromise {
        self.update_callback_done_promise.root(cx)
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#dom-viewtransition-ready>
    fn Ready(&self, cx: &mut JSContext) -> RootedPromise {
        self.ready_promise.root(cx)
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#dom-viewtransition-finished>
    fn Finished(&self, cx: &mut JSContext) -> RootedPromise {
        self.finished_promise.root(cx)
    }

    /// <https://drafts.csswg.org/css-view-transitions-1/#dom-viewtransition-skiptransition>
    fn SkipTransition(&self, cx: &mut JSContext) {
        // "If this's phase is not 'done', then skip the view transition for
        // this with an AbortError DOMException."
        if self.phase.get() != ViewTransitionPhase::Done {
            self.skip_view_transition_with_error(cx, Error::Abort(None));
        }
    }

    /// <https://drafts.csswg.org/css-view-transitions-2/#dom-viewtransition-types>
    ///
    /// `[SameObject]` attributes use the bare native name (no `Get` prefix),
    /// matching the generated trait (`fn Types(&self)`).
    fn Types(&self) -> DomRoot<ViewTransitionTypeSet> {
        DomRoot::from_ref(&*self.types)
    }

    /// <https://drafts.csswg.org/css-view-transitions-2/#dom-viewtransition-waituntil>
    fn WaitUntil(&self, cx: &mut CurrentRealm, promise: &Promise) {
        // Delay finish for promise:
        // Step 1. A finished transition is not delayed.
        if self.phase.get() == ViewTransitionPhase::Done {
            return;
        }
        // Step 2. An already-settled promise does not delay anything.
        if !promise.is_pending() {
            return;
        }
        // Step 3. Increment the wait-until promise count.
        self.wait_until_promise_count
            .set(self.wait_until_promise_count.get() + 1);

        let cx = cx.deref_mut();
        // The promise is awaited internally, so its rejection must not surface
        // as `unhandledrejection` (Chromium behavior).
        promise.set_promise_is_handled(cx);
        let global = self.global();
        let handler = PromiseNativeHandler::new(
            cx,
            &global,
            Some(Box::new(WaitUntilPromiseSettledHandler {
                transition: Dom::from_ref(self),
            })),
            Some(Box::new(WaitUntilPromiseSettledHandler {
                transition: Dom::from_ref(self),
            })),
        );
        let mut realm = CurrentRealm::assert(cx);
        promise.append_native_handler(&mut realm, &handler);
    }
}
