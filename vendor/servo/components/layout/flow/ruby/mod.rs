/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

// @trace REQ-BRW-051 (GOLDEN B / Ruby W1a): ruby formatting module root.
//
// W1a ships the ruby faces that are free of the box tree:
//  - [`plan`]: the pure box-generation planner (CSS Ruby §2.2 anonymous box
//    generation + §2.3 segmentation/pairing + HTML Ruby Extensions §2.1.1),
//  - [`builder`]: the DOM-side classification glue that feeds the planner,
//  - [`layout`]: the pure segment geometry (CSS Ruby §3.1/§3.6/§4.6).
//
// The box-tree integration (ruby inline items, annotation fragments, line
// breaking) is the next wave of the W1 plan and consumes exactly these three
// surfaces. Until then the transitional layout interpretation of the ruby
// display values (W0 bridge in `style_ext.rs`) stays authoritative for
// styling; nothing here changes any behavior of the surrounding engine yet.

pub mod builder;
pub mod layout;
pub mod plan;

pub use plan::{
    RubyChildKind, RubyPlan, RubyPlanCell, RubyPlanChild, RubyPlanContainer, RubyPlanLevel,
    RubyPlanSegment, RubyPlanUnit, RubyPlanUnitSource, build_plan,
};
