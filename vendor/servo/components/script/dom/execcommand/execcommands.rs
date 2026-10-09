/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use cssparser::match_ignore_ascii_case;
use embedder_traits::{ClipboardAction, InputEventResult};
use js::context::JSContext;
use script_bindings::codegen::GenericBindings::EventBinding::EventMethods;
use script_bindings::inheritance::Castable;

use crate::dom::bindings::codegen::Bindings::DocumentBinding::DocumentMethods;
use crate::dom::bindings::codegen::Bindings::HTMLElementBinding::HTMLElementMethods;
use crate::dom::bindings::codegen::Bindings::NodeBinding::NodeMethods;
use crate::dom::bindings::codegen::Bindings::RangeBinding::RangeMethods;
use crate::dom::bindings::root::DomRoot;
use crate::dom::bindings::str::DOMString;
use crate::dom::clipboardevent::ClipboardEventType;
use crate::dom::comment::Comment;
use crate::dom::document::Document;
use crate::dom::document::editing::EditingContext;
use crate::dom::event::Event;
use crate::dom::eventtarget::EventTarget;
use crate::dom::event::inputevent::InputEvent;
use crate::dom::execcommand::basecommand::CommandName;
use crate::dom::execcommand::commands::fontsize::maybe_normalize_pixels;
use crate::dom::html::htmlelement::HTMLElement;
use crate::dom::node::Node;
use crate::dom::processinginstruction::ProcessingInstruction;
use crate::dom::selection::Selection;

/// <https://w3c.github.io/editing/docs/execCommand/#miscellaneous-commands>
fn is_command_listed_in_miscellaneous_section(command_name: CommandName) -> bool {
    matches!(
        command_name,
        CommandName::DefaultParagraphSeparator |
            CommandName::Redo |
            CommandName::SelectAll |
            CommandName::StyleWithCss |
            CommandName::Undo |
            CommandName::Usecss
    )
}

fn bump_selection_out_of_invalid_node(cx: &mut JSContext, selection: &Selection) -> Result<(), ()> {
    // Note: Here we make sure that if the selection range starts or ends inside of an HTML
    //       comment or PI, we get it out of there before trying to edit things. Trying to
    //       perform text editing inside of these nodes doesn't make any sense anyways and
    //       some commands aren't prepared to handle that. Picking the boundary point right
    //       before the problematic node is vaguely consistent with other browsers.
    let active_range = selection
        .active_range(cx)
        .expect("Must always have an active range");
    if let start_container = active_range.start_container() &&
        (start_container.is::<Comment>() || start_container.is::<ProcessingInstruction>())
    {
        let Some(parent) = start_container.GetParentNode() else {
            return Err(());
        };
        let _ = active_range.SetStart(cx.no_gc(), &parent, start_container.index());
    }
    if let end_container = active_range.end_container() &&
        (end_container.is::<Comment>() || end_container.is::<ProcessingInstruction>())
    {
        let Some(parent) = end_container.GetParentNode() else {
            return Err(());
        };
        let _ = active_range.SetEnd(cx.no_gc(), &parent, end_container.index());
    }
    Ok(())
}

/// <https://w3c.github.io/editing/docs/execCommand/#dfn-map-an-edit-command-to-input-type-value>
fn mapped_value_of_command(command: CommandName) -> DOMString {
    match command {
        CommandName::BackColor => "formatBackColor",
        CommandName::Bold => "formatBold",
        CommandName::CreateLink => "insertLink",
        CommandName::Cut => "deleteByCut",
        CommandName::Delete => "deleteContentBackward",
        CommandName::FontName => "formatFontName",
        CommandName::ForeColor => "formatFontColor",
        CommandName::ForwardDelete => "deleteContentForward",
        CommandName::Indent => "formatIndent",
        CommandName::InsertHorizontalRule => "insertHorizontalRule",
        CommandName::InsertLineBreak => "insertLineBreak",
        CommandName::InsertOrderedList => "insertOrderedList",
        CommandName::InsertParagraph => "insertParagraph",
        CommandName::InsertText => "insertText",
        CommandName::InsertUnorderedList => "insertUnorderedList",
        CommandName::JustifyCenter => "formatJustifyCenter",
        CommandName::JustifyFull => "formatJustifyFull",
        CommandName::JustifyLeft => "formatJustifyLeft",
        CommandName::JustifyRight => "formatJustifyRight",
        CommandName::Outdent => "formatOutdent",
        CommandName::Paste => "insertFromPaste",
        CommandName::Redo => "historyRedo",
        CommandName::Strikethrough => "formatStrikeThrough",
        CommandName::Superscript => "formatSuperscript",
        CommandName::Undo => "historyUndo",
        _ => "",
    }
    .into()
}

impl Node {
    fn is_in_plaintext_only_state(&self) -> bool {
        self.downcast::<HTMLElement>()
            .is_some_and(|el| el.ContentEditable().str() == "plaintext-only")
    }
}

impl Document {
    /// Whether the active range sits inside an EditContext editing host:
    /// editing commands are disabled there and their queries always report
    /// false / the empty string (Chromium parity,
    /// `edit-context/edit-context-execCommand.tentative.https.html`).
    fn active_range_is_in_edit_context_host(&self, cx: &mut JSContext) -> bool {
        self.GetSelection(cx)
            .and_then(|selection| selection.active_range(cx))
            .and_then(|range| range.start_container().editing_host_of())
            .is_some_and(|host| {
                host.downcast::<HTMLElement>()
                    .is_some_and(|host| host.attached_edit_context().is_some())
            })
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#enabled>
    fn selection_if_command_is_enabled(
        &self,
        cx: &mut JSContext,
        command_name: CommandName,
    ) -> Option<DomRoot<Selection>> {
        let selection = self.GetSelection(cx)?;
        // > Among commands defined in this specification, those listed in Miscellaneous commands are always enabled,
        // > except for the cut command and the paste command.
        //
        // Note: cut and paste are listed in the "clipboard commands" section, not the miscellaneous section
        if is_command_listed_in_miscellaneous_section(command_name) {
            return Some(selection);
        }
        // The clipboard commands are additionally enabled for text controls
        // with an uncollapsed selection: their selection is internal to the
        // control and has no DOM active range
        // (exec-command-with-text-editor contract).
        if matches!(command_name, CommandName::Copy | CommandName::Cut) {
            let focused = self.event_handler().target_for_events_following_focus();
            if let Some(node) = focused.downcast::<Node>() &&
                EditingContext::try_from(&*node)
                    .is_ok_and(|context| context.has_uncollapsed_selection())
            {
                return Some(selection);
            }
        }
        // > The other commands defined here are enabled if the active range is not null,
        let range = selection.active_range(cx)?;
        // > its start node is either editable or an editing host,
        let start_container_editing_host = range.start_container().editing_host_of()?;
        // > the editing host of its start node is not an EditContext editing host,
        if start_container_editing_host
            .downcast::<HTMLElement>()
            .is_some_and(|host| host.attached_edit_context().is_some())
        {
            return None;
        }
        // > its end node is either editable or an editing host,
        let end_container_editing_host = range.end_container().editing_host_of()?;
        // > the editing host of its end node is not an EditContext editing host,
        if end_container_editing_host
            .downcast::<HTMLElement>()
            .is_some_and(|host| host.attached_edit_context().is_some())
        {
            return None;
        }
        // > and there is some editing host that is an inclusive ancestor of both its start node and its end node.
        // TODO

        if !command_name.is_enabled(cx, &range, &start_container_editing_host) {
            return None;
        }

        // Some commands are only enabled if the editing host is *not* in plaintext-only state.
        if !command_name.is_enabled_in_plaintext_only_state() &&
            (start_container_editing_host.is_in_plaintext_only_state() ||
                end_container_editing_host.is_in_plaintext_only_state())
        {
            None
        } else {
            Some(selection)
        }
    }

    /// Execute the `copy`/`cut` clipboard commands
    /// (<https://w3c.github.io/editing/docs/execCommand/#clipboard-commands>).
    /// Both dispatch the trusted clipboard event; inside an EditContext
    /// editing host `copy` still writes the DOM selection to the clipboard
    /// while `cut` changes neither the DOM nor the clipboard and reports
    /// success (Chromium: "cut always returns true regardless of whether it
    /// did anything").
    fn exec_clipboard_command(&self, cx: &mut JSContext, command_id: &DOMString) -> bool {
        let is_cut = command_id.str() == "cut";
        let event_target = self.event_handler().target_for_events_following_focus();
        if is_cut && self.active_range_is_in_edit_context_host(cx) {
            self.fire_clipboard_event(cx, &event_target, ClipboardEventType::Cut);
            return true;
        }
        let action = if is_cut {
            ClipboardAction::Cut
        } else {
            ClipboardAction::Copy
        };
        let Some(node) = event_target.downcast::<Node>() else {
            return false;
        };
        let editing_context = self.editing_context(cx.no_gc(), node);
        // The clipboard commands are only enabled for editable content: a
        // selection outside any editing region reports failure
        // (exec-command-without-editable-element contract), while a selection
        // in a text control, a contenteditable or an EditContext editing
        // host runs the clipboard event machinery.
        let selection_is_editable = matches!(editing_context, EditingContext::TextControl(..)) ||
            self.active_range_is_in_edit_context_host(cx) ||
            self.GetSelection(cx)
                .and_then(|selection| selection.active_range(cx))
                .and_then(|range| range.start_container().editing_host_of())
                .is_some();
        if !selection_is_editable {
            // The trusted clipboard event still fires on the executed
            // document, but the command reports failure
            // (exec-command-without-editable-element: the event fires while
            // execCommand returns false).
            let event_type = if is_cut {
                ClipboardEventType::Cut
            } else {
                ClipboardEventType::Copy
            };
            self.fire_clipboard_event(cx, &event_target, event_type);
            return false;
        }
        if let EditingContext::TextControl(..) = editing_context {
            return self.exec_clipboard_command_on_text_control(cx, &event_target, is_cut);
        }
        self.handle_clipboard_action(cx, &editing_context, action)
            .contains(InputEventResult::Consumed)
    }

    /// The execCommand form of copy/cut for text controls: unlike the
    /// keyboard shortcut it fires *no* `beforeinput` — only the clipboard
    /// event, the selection removal (cut) and the trailing `input`
    /// (`deleteByCut`); a password field cuts its selection and masks it
    /// from the clipboard while `copy` still reports success
    /// (edit-context/exec-command-with-text-editor contract).
    fn exec_clipboard_command_on_text_control(
        &self,
        cx: &mut JSContext,
        event_target: &EventTarget,
        is_cut: bool,
    ) -> bool {
        let event_type = if is_cut {
            ClipboardEventType::Cut
        } else {
            ClipboardEventType::Copy
        };
        let clipboard_event = self.fire_clipboard_event(cx, event_target, event_type);
        let event = clipboard_event.upcast::<Event>();
        if event.DefaultPrevented() {
            return false;
        }
        let Some(node) = event_target.downcast::<Node>() else {
            return false;
        };
        let editing_context = self.editing_context(cx.no_gc(), node);
        // A password field's selection content is masked (not observable),
        // so emptiness is decided by the selection itself, not its text; its
        // `copy` still reports success while writing nothing.
        if !editing_context.has_uncollapsed_selection() {
            return false;
        }
        if editing_context.copying_enabled() &&
            let Some(selection) = editing_context.selection_content(cx)
        {
            self.send_to_embedder(embedder_traits::EmbedderMsg::SetClipboardText(
                self.webview_id(),
                selection,
            ));
        }
        if is_cut {
            editing_context.remove_the_contents_of_the_selection(cx);
            // The execCommand form fires the trailing `input` synchronously
            // (the keyboard shortcut queues it instead): the editor contract
            // asserts `input.inputType` right after execCommand returns.
            let input_event = InputEvent::new(
                cx,
                &self.window(),
                None,
                atom!("input"),
                true,
                false,
                Some(&self.window()),
                0,
                None,
                false,
                DOMString::from_static("deleteByCut"),
            );
            let input_event = input_event.upcast::<Event>();
            input_event.set_trusted(true);
            input_event.fire(cx, event_target);
        }
        true
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#supported>
    fn command_if_command_is_supported(&self, command_id: &DOMString) -> Option<CommandName> {
        // https://w3c.github.io/editing/docs/execCommand/#methods-to-query-and-execute-commands
        // > All of these methods must treat their command argument ASCII case-insensitively.
        Some(match_ignore_ascii_case! { &command_id.str(),
            "backcolor" => CommandName::BackColor,
            "bold" => CommandName::Bold,
            "copy" => CommandName::Copy,
            "createlink" => CommandName::CreateLink,
            "cut" => CommandName::Cut,
            "delete" => CommandName::Delete,
            "defaultparagraphseparator" => CommandName::DefaultParagraphSeparator,
            "fontname" => CommandName::FontName,
            "fontsize" => CommandName::FontSize,
            "forecolor" => CommandName::ForeColor,
            "forwarddelete" => CommandName::ForwardDelete,
            "hilitecolor" => CommandName::HiliteColor,
            "indent" => CommandName::Indent,
            "inserthorizontalrule" => CommandName::InsertHorizontalRule,
            "insertimage" => CommandName::InsertImage,
            "insertlinebreak" => CommandName::InsertLineBreak,
            "insertparagraph" => CommandName::InsertParagraph,
            "inserttext" => CommandName::InsertText,
            "italic" => CommandName::Italic,
            "removeformat" => CommandName::RemoveFormat,
            "strikethrough" => CommandName::Strikethrough,
            "stylewithcss" => CommandName::StyleWithCss,
            "subscript" => CommandName::Subscript,
            "superscript" => CommandName::Superscript,
            "underline" => CommandName::Underline,
            "unlink" => CommandName::Unlink,
            _ => return None,
        })
    }
}

pub(crate) trait DocumentExecCommandSupport {
    fn is_command_supported(&self, command_id: DOMString) -> bool;
    fn is_command_indeterminate(&self, cx: &mut JSContext, command_id: DOMString) -> bool;
    fn command_state_for_command(&self, cx: &mut JSContext, command_id: DOMString) -> bool;
    fn command_value_for_command(&self, cx: &mut JSContext, command_id: DOMString) -> DOMString;
    fn check_support_and_enabled(
        &self,
        cx: &mut JSContext,
        command_id: &DOMString,
    ) -> Option<(CommandName, DomRoot<Selection>)>;
    fn exec_command_for_command_id(
        &self,
        cx: &mut JSContext,
        command_id: DOMString,
        value: DOMString,
    ) -> bool;
}

impl DocumentExecCommandSupport for Document {
    /// <https://w3c.github.io/editing/docs/execCommand/#querycommandsupported()>
    fn is_command_supported(&self, command_id: DOMString) -> bool {
        self.command_if_command_is_supported(&command_id).is_some()
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#querycommandindeterm()>
    fn is_command_indeterminate(&self, cx: &mut JSContext, command_id: DOMString) -> bool {
        // Step 1. If command is not supported or has no indeterminacy, return false.
        // Step 2. Return true if command is indeterminate, otherwise false.
        // Inside an EditContext editing host queries always report false.
        if self.active_range_is_in_edit_context_host(cx) {
            return false;
        }
        self.command_if_command_is_supported(&command_id)
            .is_some_and(|command| command.is_indeterminate(cx, self))
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#querycommandstate()>
    fn command_state_for_command(&self, cx: &mut JSContext, command_id: DOMString) -> bool {
        // Step 1. If command is not supported or has no state, return false.
        let Some(command) = self.command_if_command_is_supported(&command_id) else {
            return false;
        };
        // Inside an EditContext editing host queries always report false.
        if self.active_range_is_in_edit_context_host(cx) {
            return false;
        }
        let Some(state) = command.current_state(cx, self) else {
            return false;
        };
        // Step 2. If the state override for command is set, return it.
        // Step 3. Return true if command's state is true, otherwise false.
        self.state_override(&command).unwrap_or(state)
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#querycommandvalue()>
    fn command_value_for_command(&self, cx: &mut JSContext, command_id: DOMString) -> DOMString {
        // Step 1. If command is not supported or has no value, return the empty string.
        let Some(command) = self.command_if_command_is_supported(&command_id) else {
            return DOMString::new();
        };
        // Inside an EditContext editing host queries always report the empty
        // string.
        if self.active_range_is_in_edit_context_host(cx) {
            return DOMString::new();
        }
        let Some(value) = command.current_value(cx, self) else {
            return DOMString::new();
        };
        // Step 3. If the value override for command is set, return it.
        self.value_override(&command)
            .map(|value_override| {
                // Step 2. If command is "fontSize" and its value override is set,
                // convert the value override to an integer number of pixels and return the legacy font size for the result.
                if command == CommandName::FontSize {
                    maybe_normalize_pixels(&value_override, self).unwrap_or(value_override)
                } else {
                    value_override
                }
            })
            // Step 4. Return command's value.
            .unwrap_or(value)
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#querycommandenabled()>
    fn check_support_and_enabled(
        &self,
        cx: &mut JSContext,
        command_id: &DOMString,
    ) -> Option<(CommandName, DomRoot<Selection>)> {
        // Step 2. Return true if command is both supported and enabled, false otherwise.
        let command = self.command_if_command_is_supported(command_id)?;
        let selection = self.selection_if_command_is_enabled(cx, command)?;
        Some((command, selection))
    }

    /// <https://w3c.github.io/editing/docs/execCommand/#execcommand()>
    fn exec_command_for_command_id(
        &self,
        cx: &mut JSContext,
        command_id: DOMString,
        value: DOMString,
    ) -> bool {
        // The clipboard commands run the clipboard event machinery instead of
        // the editing-command pipeline.
        if command_id.str() == "copy" || command_id.str() == "cut" {
            return self.exec_clipboard_command(cx, &command_id);
        }
        let window = self.window();
        // Step 3. If command is not supported or not enabled, return false.
        let Some((command, mut selection)) = self.check_support_and_enabled(cx, &command_id) else {
            return false;
        };
        // Step 4. If command is not in the Miscellaneous commands section:
        let affected_editing_host = if !is_command_listed_in_miscellaneous_section(command) {
            // Step 4.1. Let affected editing host be the editing host that is an inclusive ancestor
            // of the active range's start node and end node, and is not the ancestor of any editing host
            // that is an inclusive ancestor of the active range's start node and end node.
            let Some(affected_editing_host) = selection
                .active_range(cx)
                .expect("Must always have an active range")
                .CommonAncestorContainer()
                .editing_host_of()
            else {
                return false;
            };

            // Step 4.2. Fire an event named "beforeinput" at affected editing host using InputEvent,
            // with its bubbles and cancelable attributes initialized to true, and its data attribute initialized to null
            let event = InputEvent::new(
                cx,
                window,
                None,
                atom!("beforeinput"),
                true,
                true,
                Some(window),
                0,
                None,
                false,
                mapped_value_of_command(command),
            );
            let event = event.upcast::<Event>();
            // Step 4.3. If the value returned by the previous step is false, return false.
            if !event.fire(cx, affected_editing_host.upcast()) {
                return false;
            }

            // Step 4.4. If command is not enabled, return false.
            let Some(new_selection) = self.selection_if_command_is_enabled(cx, command) else {
                return false;
            };
            selection = new_selection;

            // Step 4.5. Let affected editing host be the editing host that is an inclusive ancestor
            // of the active range's start node and end node, and is not the ancestor of any editing host
            // that is an inclusive ancestor of the active range's start node and end node.
            selection
                .active_range(cx)
                .expect("Must always have an active range")
                .CommonAncestorContainer()
                .editing_host_of()
        } else {
            None
        };

        if affected_editing_host.is_some() &&
            bump_selection_out_of_invalid_node(cx, &selection).is_err()
        {
            return false;
        }

        // Step 5. Take the action for command, passing value to the instructions as an argument.
        let result = command.execute(cx, self, &selection, value);
        // Step 6. If the previous step returned false, return false.
        if !result {
            return false;
        }
        // Step 7. If the action modified DOM tree, then fire an event named "input" at affected editing
        // host using InputEvent, with its isTrusted and bubbles attributes initialized to true,
        // inputType attribute initialized to the mapped value of command, and its data attribute initialized to null.
        if let Some(affected_editing_host) = affected_editing_host {
            let event = InputEvent::new(
                cx,
                window,
                None,
                atom!("input"),
                true,
                false,
                Some(window),
                0,
                None,
                false,
                mapped_value_of_command(command),
            );
            let event = event.upcast::<Event>();
            event.set_trusted(true);
            event.fire(cx, affected_editing_host.upcast());
        }

        // Step 8. Return true.
        true
    }
}
