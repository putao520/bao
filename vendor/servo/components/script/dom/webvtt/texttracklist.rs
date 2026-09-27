/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// BAO patch (fork-maintained, 2026-09-27): resynced to upstream 7ca99fe3f —
// the list again owns its `HTMLMediaElement` and enumerates text tracks in
// spec order (track element children in tree order, then addTextTrack tracks)
// through `TextTrackListIterator`; `Length`/`IndexedGetter`/`GetTrackById`
// take the no-GC token and `add` seeds the media element's newly-introduced
// cues (REQ-BRW-047). Fork form: unrooted views are built with
// `UnrootedDom::from_dom(.., no_gc)` (no `Dom::as_unrooted` in this fork).

use dom_struct::dom_struct;
use js::context::{JSContext, NoGC};
use script_bindings::cell::DomRefCell;
use script_bindings::reflector::reflect_dom_object_with_cx;

use crate::dom::bindings::codegen::Bindings::TextTrackListBinding::TextTrackListMethods;
use crate::dom::bindings::codegen::UnionTypes::VideoTrackOrAudioTrackOrTextTrack;
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::refcounted::Trusted;
use crate::dom::bindings::reflector::DomGlobal;
use crate::dom::bindings::root::{Dom, DomRoot, UnrootedDom};
use crate::dom::bindings::str::DOMString;
use crate::dom::event::Event;
use crate::dom::eventtarget::EventTarget;
use crate::dom::html::htmlmediaelement::HTMLMediaElement;
use crate::dom::html::htmltrackelement::HTMLTrackElement;
use crate::dom::node::node::Node;
use crate::dom::texttrack::TextTrack;
use crate::dom::texttrackcue::TextTrackCue;
use crate::dom::trackevent::TrackEvent;
use crate::dom::window::Window;

#[dom_struct]
pub(crate) struct TextTrackList {
    eventtarget: EventTarget,
    /// <https://html.spec.whatwg.org/multipage/#list-of-text-tracks>
    media_element: Dom<HTMLMediaElement>,
    dom_tracks: DomRefCell<Vec<Dom<TextTrack>>>,
}

impl TextTrackList {
    pub(crate) fn new_inherited(
        media_element: &HTMLMediaElement,
        tracks: &[&TextTrack],
    ) -> TextTrackList {
        TextTrackList {
            eventtarget: EventTarget::new_inherited(),
            media_element: Dom::from_ref(media_element),
            dom_tracks: DomRefCell::new(tracks.iter().map(|g| Dom::from_ref(&**g)).collect()),
        }
    }

    pub(crate) fn new(
        cx: &mut JSContext,
        media_element: &HTMLMediaElement,
        window: &Window,
        tracks: &[&TextTrack],
    ) -> DomRoot<TextTrackList> {
        reflect_dom_object_with_cx(
            Box::new(TextTrackList::new_inherited(media_element, tracks)),
            window,
            cx,
        )
    }

    pub(crate) fn notify_media_element_for_added_cue(
        &self,
        cx: &mut JSContext,
        cue: &TextTrackCue,
    ) {
        self.media_element.add_newly_added_cue(cx, cue);
    }

    pub(crate) fn add(&self, cx: &mut JSContext, track: &TextTrack) {
        // We should only store the tracks created by `addTextTrack`
        // since the iterator already traverses children of a media
        // element for <track> elements. Otherwise, we would be double
        // counting.
        if track.associated_track().is_none() {
            self.dom_tracks.borrow_mut().push(Dom::from_ref(track));
        }

        track.add_track_list(self);
        self.media_element
            .was_added_to_list_of_text_tracks(cx, track);

        let this = Trusted::new(self);
        let track = Trusted::new(track);
        self.global()
            .task_manager()
            .media_element_task_source()
            .queue(task!(track_event_queue: move |cx| {
                let this = this.root();
                let track = track.root();

                let event = TrackEvent::new(
                    cx,
                    this.global().as_window(),
                    atom!("addtrack"),
                    false,
                    false,
                    &Some(VideoTrackOrAudioTrackOrTextTrack::TextTrack(
                        DomRoot::from_ref(&track)
                    )),
                );

                event.upcast::<Event>().fire(cx, this.upcast::<EventTarget>());
            }));
    }

    pub(crate) fn remove(&self, track: &TextTrack) {
        if let Some(idx) = self
            .dom_tracks
            .borrow()
            .iter()
            .position(|dom_track| &**dom_track == track)
        {
            self.dom_tracks.borrow_mut().remove(idx);
        };
        track.remove_track_list();

        let this = Trusted::new(self);
        let track = Trusted::new(track);
        self.global()
            .task_manager()
            .media_element_task_source()
            .queue(task!(track_event_queue: move |cx| {
                let this = this.root();
                let track = track.root();

                let event = TrackEvent::new(
                    cx,
                    this.global().as_window(),
                    atom!("removetrack"),
                    false,
                    false,
                    &Some(VideoTrackOrAudioTrackOrTextTrack::TextTrack(
                        DomRoot::from_ref(&track)
                    )),
                );

                event.upcast::<Event>().fire(cx, this.upcast::<EventTarget>());
            }));
    }

    // BAO patch (fork-maintained, 2026-09-27): the fork's codegen generates
    // the flat (no `no_gc` parameter) `TextTrackListMethods` signatures, so
    // the trait methods below walk the spec-ordered track list through this
    // rooted helper instead of the unrooted `iter`.
    fn tracks_in_spec_order(&self) -> Vec<DomRoot<TextTrack>> {
        // Step 1. The text tracks corresponding to track element children of
        // the media element, in tree order, then Step 2. any text tracks
        // added using the addTextTrack() method, in the order they were
        // added, oldest first. (Step 3, media-resource-specific tracks, is a
        // TODO, as upstream.)
        let track_element_tracks: Vec<DomRoot<TextTrack>> = self
            .media_element
            .upcast::<Node>()
            .children()
            .filter_map(|child| DomRoot::downcast::<HTMLTrackElement>(child))
            .map(|track_element| track_element.track_rooted())
            .collect();
        let dom_tracks: Vec<DomRoot<TextTrack>> = self
            .dom_tracks
            .borrow()
            .clone()
            .into_iter()
            .map(|track| DomRoot::from_ref(&*track))
            .collect();
        track_element_tracks.into_iter().chain(dom_tracks).collect()
    }

    pub(crate) fn iter<'a>(&'a self, no_gc: &'a NoGC) -> TextTrackListIterator<'a> {
        TextTrackListIterator {
            no_gc,
            track_elements: Box::new(
                self.media_element
                    .upcast::<Node>()
                    .children_unrooted(no_gc)
                    .filter_map(UnrootedDom::downcast::<HTMLTrackElement>),
            ),
            dom_tracks: Box::new(
                self.dom_tracks
                    .borrow()
                    .clone()
                    .into_iter()
                    .map(|track| UnrootedDom::from_dom(track, no_gc)),
            ),
        }
    }
}

impl TextTrackListMethods<crate::DomTypeHolder> for TextTrackList {
    /// <https://html.spec.whatwg.org/multipage/#dom-texttracklist-length>
    fn Length(&self) -> u32 {
        // > The length attribute of a TextTrackList object must return
        // > the number of text tracks in the list represented by the TextTrackList object.
        self.tracks_in_spec_order().len() as u32
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-texttracklist-item>
    fn IndexedGetter(&self, idx: u32) -> Option<DomRoot<TextTrack>> {
        // > To determine the value of an indexed property of a TextTrackList object
        // > for a given index index, the user agent must return the indexth
        // > text track in the list represented by the TextTrackList object.
        self.tracks_in_spec_order().into_iter().nth(idx as usize)
    }

    /// <https://html.spec.whatwg.org/multipage/#dom-texttracklist-gettrackbyid>
    fn GetTrackById(&self, id: DOMString) -> Option<DomRoot<TextTrack>> {
        // > The getTrackById(id) method must return the first TextTrack in
        // > the TextTrackList object whose id IDL attribute would return
        // > a value equal to the value of the id argument.
        // > When no tracks match the given argument, the method must return null.
        let id_str = String::from(id);
        self.tracks_in_spec_order()
            .into_iter()
            .find(|track| *track.id() == id_str)
    }

    // https://html.spec.whatwg.org/multipage/#handler-texttracklist-onchange
    event_handler!(change, GetOnchange, SetOnchange);

    // https://html.spec.whatwg.org/multipage/#handler-texttracklist-onaddtrack
    event_handler!(addtrack, GetOnaddtrack, SetOnaddtrack);

    // https://html.spec.whatwg.org/multipage/#handler-texttracklist-onremovetrack
    event_handler!(removetrack, GetOnremovetrack, SetOnremovetrack);
}

/// <https://html.spec.whatwg.org/multipage/#list-of-text-tracks>
pub(crate) struct TextTrackListIterator<'a> {
    no_gc: &'a NoGC,
    track_elements: Box<dyn Iterator<Item = UnrootedDom<'a, HTMLTrackElement>> + 'a>,
    dom_tracks: Box<dyn Iterator<Item = UnrootedDom<'a, TextTrack>> + 'a>,
}

impl<'a> Iterator for TextTrackListIterator<'a> {
    type Item = UnrootedDom<'a, TextTrack>;

    fn next(&mut self) -> Option<Self::Item> {
        // > The text tracks are sorted as follows:
        // Step 1. The text tracks corresponding to track element children of the media element, in tree order.
        if let Some(track_element) = self.track_elements.next() {
            return Some(track_element.track(self.no_gc));
        }
        // Step 2. Any text tracks added using the addTextTrack() method,
        // in the order they were added, oldest first.
        if let Some(dom_track) = self.dom_tracks.next() {
            return Some(dom_track);
        }
        // Step 3. Any media-resource-specific text tracks
        // (text tracks corresponding to data in the media resource),
        // in the order defined by the media resource's format specification.
        // TODO
        None
    }
}
