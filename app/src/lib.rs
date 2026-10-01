//! The browser body.
//!
//! This crate compiles to wasm and is embedded in the appliance that serves
//! it. It depends only on `wire`, so the socket is its complete system
//! boundary: journal, board, lane, and trust machinery are unreachable
//! from browser code, and a server field the app needs but wire lacks is a
//! compile-time contract gap rather than an internal import.
//!
//! The inbound path is three floors. [`socket`] admits bytes against one
//! current opening, [`state`] holds what was admitted, and [`pane`] presents
//! it. The join between the first two is a type: admission yields
//! [`Admitted`], and state accepts nothing else, so "the browser believes only
//! what crossed the door" is checked at compile time. The join to the third is
//! the same one: a [`Principal`] comes from an admitted acceptance and from
//! nowhere else, so a pane's authority class is the resident's answer rather
//! than the app's assertion. [`view`] names the closed [`Axis`] vocabulary the
//! Canvas destinations stand on.
//!
//! Those three all face inward, holding or presenting what the resident said.
//! [`compose`] is the one floor facing the other way, turning text an operator
//! typed into the fields a request carries. It is therefore the only floor
//! that refuses, and its refusals are sentences rather than log lines: the
//! form they came from has a button, and a button that does nothing and says
//! nothing reads as a dead control. [`mosaic`] faces neither way: it is the
//! lattice that decides where a card lands, given only creation order and
//! measured widths. Both are reachable from an ordinary native test, which
//! the module owning the forms and the rendering is not, being wasm-only.
//!
//! No floor constructs authority. Read panes append nothing, and steer and
//! grant panes build ordinary commands the resident authenticates for itself.
//! No floor constructs provenance either: a fold-backed answer is held as
//! [`Answer::Derived`] wearing the fold, generation, and cursor it arrived
//! with, a head-direct answer is held as [`Answer::HeadDirect`] with nowhere
//! to keep them, and that difference survives every floor above.

#[cfg(any(target_arch = "wasm32", test))]
#[path = "browser/action_mailbox.rs"]
mod action_mailbox;
#[cfg(any(target_arch = "wasm32", test))]
mod client_action;
pub mod compose;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_drafts;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_markdown;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_submissions;
#[cfg(any(target_arch = "wasm32", test))]
mod image_annotation;
#[cfg(any(target_arch = "wasm32", test))]
mod mermaid_geometry;
#[cfg(any(target_arch = "wasm32", test))]
mod mermaid_label;
pub mod mosaic;
pub mod mosaic_plane;
pub mod narration;
pub mod pane;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod pip_layout;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(test, allow(dead_code))]
mod quota;
#[cfg(any(target_arch = "wasm32", test))]
mod reconnect;
pub mod runtime;
pub mod socket;
pub mod speech_owner;
pub mod speech_queue;
pub mod state;
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
mod task_pill;
pub mod view;
pub mod watchband;
#[cfg(any(target_arch = "wasm32", test))]
pub mod work;

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
mod browser;

#[derive(Clone)]
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum PendingKind {
    Board,
    Lanes {
        wakes: Vec<wire::CanonicalUuid>,
    },
    Inbox {
        covered: std::collections::BTreeMap<wire::CanonicalUuid, String>,
    },
    Settings,
    Vendors,
    /// The operator's pending consent queue.
    Consent,
    /// One operator ruling on a queued consent request.
    ConsentRuling {
        request: wire::CanonicalUuid,
    },
    TaskDetail {
        task: wire::TaskStamp,
    },
    TaskRelations {
        key: runtime::TaskRelationsKey,
    },
    TaskTimeline {
        request: runtime::TaskTimelineRequest,
    },
    DirectionHistory {
        request: runtime::DirectionHistoryRequest,
    },
    WorkHistory {
        request: runtime::WorkHistoryRequest,
    },
    TeamSettled {
        request: runtime::TeamSettledRequest,
    },
    LaneSeat,
    LaneRenewal {
        team: wire::CanonicalUuid,
        lane: wire::CanonicalUuid,
        requested: bool,
        member: Option<runtime::MemberRequest>,
    },
    /// A Dash member's wake, drain, or remove.
    MemberLane {
        request: runtime::MemberRequest,
    },
    /// A Dash member's launch-model change.
    LaneRetune {
        request: runtime::MemberRequest,
    },
    WakeSubmission {
        lane: wire::CanonicalUuid,
        submission: wire::CanonicalUuid,
    },
    TeamConfigure {
        team: wire::CanonicalUuid,
        life: Option<wire::intent::TeamLife>,
    },
    TeamArrange,
    Steer,
    SteerSubmission {
        lane: wire::CanonicalUuid,
    },
    Note {
        task: wire::TaskStamp,
        text: String,
        after: Option<wire::Seq>,
    },
    Close {
        task: wire::TaskStamp,
    },
}
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
mod speech;

pub use pane::{Declined, GrantPane, Principal, ReadPane, SteerPane, settled};
pub use runtime::{
    BoardView, CanvasRuntime, LaneActivity, LaneMember, LaneRow, LaneStatus, LaneView, Stale,
    StreamApplication, StreamDirective, StreamTerminal, TaskDetailState, TaskRelationsKey,
    TaskRelationsState, TaskTimelineKey, TaskTimelineState, stale_reads,
};
pub use socket::{Admitted, Session};
pub use state::{Answer, Applied, BrowserState, CAUSAL_HEADERS, CausalLink, ChainEnd, ChainWalk};
pub use view::{Axis, CanvasDestination, TeamDestination};
