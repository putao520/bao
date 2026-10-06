/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// https://webaudio.github.io/web-audio-api/#AudioWorkletNode
//
// (Bao) Upstream servo: zero AudioWorklet runtime (see AudioWorklet.webidl).
// Authored in-tree per the fork-self-maintenance ruling (user ruling
// 2026-10-05). Section (2) (servo-media bridge) adds the servo-media
// AudioWorklet node type; until that lands the node constructs as an INERT
// AudioNode (node_id = None — the engine's own "no backend" form, mirroring
// how every servo AudioNode degrades when servo-media cannot host it,
// audionode.rs new_inherited), so construction is real, `instanceof
// AudioNode` holds, and connect() no-ops honestly instead of faking graph
// participation.
[Exposed=Window]
interface AudioWorkletNode : AudioNode {
    [Throws] constructor(BaseAudioContext context,
                         DOMString name,
                         optional AudioWorkletNodeOptions options = {});
    readonly attribute MessagePort port;
    // (Bao 段(3) wiring): the processor's declared AudioParams, keyed in
    // `parameterDescriptors` order; each AudioParam automates the media-side
    // `ParamType::WorkletParam(index)` of the node's servo-media graph node.
    readonly attribute AudioParamMap parameters;
    attribute EventHandler onprocessorerror;
};

dictionary AudioWorkletNodeOptions : AudioNodeOptions {
    unsigned long numberOfInputs = 1;
    unsigned long numberOfOutputs = 1;
    sequence<unsigned long>? outputChannelCount = null;
    // (Bao e114) spec §AudioWorkletNode-constructors step 9-10: the options
    // dictionary is converted to a JS object and StructuredSerialize'd for
    // the processor constructor's single argument (the webaudio spec's
    // AudioWorkletNodeOptions processorOptions member).
    any processorOptions = null;
};
