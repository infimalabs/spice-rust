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
//! than the app's assertion.
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
mod browser_branding;
#[cfg(any(target_arch = "wasm32", test))]
mod client_action;
pub mod compose;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_drafts;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_history;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_markdown;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_reading_order;
#[cfg(any(target_arch = "wasm32", test))]
mod composer_submissions;
#[cfg(any(target_arch = "wasm32", test))]
mod draft_form;
#[cfg(any(target_arch = "wasm32", test))]
mod image_annotation;
#[cfg(any(target_arch = "wasm32", test))]
mod image_navigation;
#[cfg(any(target_arch = "wasm32", test))]
mod mermaid_geometry;
#[cfg(any(target_arch = "wasm32", test))]
mod mermaid_label;
pub mod mosaic;
#[cfg(any(target_arch = "wasm32", test))]
mod mosaic_crystal;
pub mod mosaic_plane;
pub mod narration;
#[cfg(any(target_arch = "wasm32", test))]
mod operator_inbox;
pub mod pane;
#[cfg(any(target_arch = "wasm32", test))]
mod preparation_decision;
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(test, allow(dead_code))]
mod quota;
#[cfg(any(target_arch = "wasm32", test))]
mod reconnect;
#[cfg(any(target_arch = "wasm32", test))]
mod room_views;
pub mod roster;
pub mod runtime;
pub mod socket;
pub mod speech_owner;
pub mod speech_queue;
#[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
mod speech_rate;
pub mod state;
#[cfg(any(target_arch = "wasm32", test))]
mod task_sheet;
#[cfg(any(target_arch = "wasm32", test, rust_analyzer))]
mod toast_state;
#[cfg(any(target_arch = "wasm32", test))]
mod unsent_work;
pub mod view;
pub mod voice_spread;
pub mod watchband;
#[cfg(any(target_arch = "wasm32", test))]
pub mod work;

#[cfg(any(target_arch = "wasm32", rust_analyzer))]
mod browser;

#[derive(Clone)]
#[cfg(any(target_arch = "wasm32", test))]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum PendingKind {
    Jobs {
        respond: client_action::JobsResponse,
    },
    Job {
        target: wire::intent::LaneJobRef,
        stop: bool,
        respond: client_action::JobResponse,
    },
    WorkingGroup {
        respond: client_action::GroupResponse,
    },
    RoomPost {
        respond: client_action::RoomPostResponse,
    },
    RoomControl {
        respond: client_action::RoomControlResponse,
    },
    RoomCoordination {
        respond: client_action::RoomCoordinationResponse,
    },
    Preparation {
        respond: client_action::PreparationResponse,
    },
    Draft {
        respond: client_action::TaskResponse,
    },
    Conclude {
        respond: client_action::TaskResponse,
    },
    Deferral {
        respond: client_action::TaskResponse,
    },
    Board,
    Lanes {
        rebases: Vec<wire::CanonicalUuid>,
    },
    Inbox {
        covered: std::collections::BTreeMap<wire::CanonicalUuid, String>,
    },
    /// Every principal's display identity and this connection's roles.
    PrincipalDirectory,
    Settings,
    Vendors,
    /// The operator's pending consent queue.
    Consent,
    /// Every born principal, read by an operator-role principal.
    Principals,
    /// One principal write.
    PrincipalWrite {
        respond: client_action::PrincipalResponse,
    },
    /// One operator ruling on a queued consent request.
    ConsentRuling {
        request: wire::CanonicalUuid,
    },
    /// One exact trust approval initiated from a current task requirement.
    TrustGrant {
        digest: wire::Digest,
    },
    TaskDetail {
        task: wire::TaskStamp,
    },
    /// One lane-authored conversation publication's text.
    ConversationText {
        publication: wire::rooms::RoomAuthorityStamp,
    },
    /// One Design contribution's resolved sources and question item.
    ContributionRead {
        publication: wire::rooms::RoomAuthorityStamp,
    },
    TaskRelations {
        key: runtime::TaskRelationsKey,
    },
    LaneRenewal {
        team: wire::CanonicalUuid,
        lane: wire::CanonicalUuid,
        requested: bool,
        member: Option<runtime::MemberRequest>,
    },
    /// A resident member's wake, drain, or remove.
    MemberLane {
        request: runtime::MemberRequest,
    },
    /// A resident member's launch-model change.
    LaneRetune {
        request: runtime::MemberRequest,
    },
    TeamConfigure {
        team: wire::CanonicalUuid,
        life: Option<wire::intent::TeamLife>,
        fast_mode: Option<bool>,
    },
    TeamArrange {
        respond: Option<client_action::ArrangementResponse>,
    },
    Steer,
    SteerSubmission {
        lane: wire::CanonicalUuid,
    },
    Close {
        task: wire::TaskStamp,
    },
    Delete {
        task: wire::TaskStamp,
    },
}
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
mod speech;

pub use pane::{Declined, GrantPane, Principal, ReadPane, SteerPane, settled};
pub use runtime::{
    AppRuntime, AppSurface, BlockerActionState, BoardView, CandidateWithdrawalRecord, Contribution,
    ContributionRead, Conversation, ConversationProjection, ConversationText, CoordinationStanding,
    Delivery, ItemConcern, LaneActivity, LaneMember, LaneRow, LaneStatus, LaneView, OperatorItem,
    ParticipantReason, PreparationProjection, Relevance, RoomAnchorAuthority, RoomAnchorSettlement,
    RoomEvent, RoomEventKind, RoomParticipant, RoomPositioned, RoomProjection,
    RoomProjectionStatus, RoomState, Stale, StreamApplication, StreamDirective, StreamTerminal,
    TaskDetailState, TaskRelationsKey, TaskRelationsState, stale_reads,
};
pub use socket::{Admitted, Session};
pub use state::{Answer, Applied, BrowserState, CAUSAL_HEADERS, CausalLink, ChainEnd, ChainWalk};
