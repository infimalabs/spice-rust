//! Closed command spellings and serialization.

use std::{fmt, str::FromStr};

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use super::{CommandKind, CommandKindError};

impl CommandKind {
    /// Every command in the closed registry, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::BoardRead,
        Self::ReportRead,
        Self::TaskDetail,
        Self::TaskRelations,
        Self::TaskTimeline,
        Self::BoardGraph,
        Self::BoardHandout,
        Self::DocLedger,
        Self::RoomPost,
        Self::RoomConversationText,
        Self::RoomLeave,
        Self::RoomContribute,
        Self::RoomContributionRead,
        Self::RoomControl,
        Self::WorkingGroupRead,
        Self::WorkingGroupMove,
        Self::WorkingGroupArrange,
        Self::WorkingGroupDisband,
        Self::WorkingGroupFast,
        Self::DesignPropose,
        Self::CandidateCritiqueRecord,
        Self::CandidateCoverageRecord,
        Self::PreparationDecide,
        Self::CandidateWithdraw,
        Self::OutputAdopt,
        Self::DesignNudge,
        Self::GlobalSettingsRead,
        Self::GlobalSettingsSet,
        Self::VendorCatalog,
        Self::TaskAdd,
        Self::TaskOopsAdd,
        Self::TaskEdit,
        Self::TaskDelete,
        Self::TaskClose,
        Self::TaskConclude,
        Self::TaskNote,
        Self::TaskDefer,
        Self::TaskWake,
        Self::TaskFile,
        Self::TaskDepends,
        Self::TaskPhase,
        Self::TaskDone,
        Self::TaskReview,
        Self::TaskAssign,
        Self::ClaimWin,
        Self::ClaimCarry,
        Self::ClaimRefresh,
        Self::ClaimRenew,
        Self::ClaimFree,
        Self::ClaimLapse,
        Self::DocApply,
        Self::SteerSend,
        Self::SteerInbox,
        Self::SteerHistory,
        Self::SteerResend,
        Self::SteerRecover,
        Self::SteerDispose,
        Self::SteerCancel,
        Self::QuoteRead,
        Self::ConsentRead,
        Self::ConsentPolicy,
        Self::ConsentDecide,
        Self::TrustPend,
        Self::TrustGrant,
        Self::TrustRevoke,
        Self::PrincipalDirectoryRead,
        Self::PrincipalRead,
        Self::PrincipalAdmit,
        Self::PrincipalEdit,
        Self::PrincipalGrant,
        Self::PrincipalRevoke,
        Self::PrincipalDeauthorize,
        Self::LaneRead,
        Self::LaneJobsRead,
        Self::LaneJobRead,
        Self::LaneJobStop,
        Self::LaneSeat,
        Self::LaneArrange,
        Self::LaneState,
        Self::LaneRenewal,
        Self::LaneRetune,
        Self::LaneEffortRetune,
        Self::LaneFastRetune,
        Self::TeamConfigure,
        Self::TeamArrange,
        Self::LaneRecall,
        Self::TasteRecord,
        Self::CanaryRecognize,
        Self::VendorRecognize,
        Self::CommitRecognize,
        Self::ProofRecognize,
        Self::ReleasePublish,
        Self::UpChange,
    ];

    /// The wire spelling of this command.
    ///
    /// The outer partition is exhaustive, so a new variant must state which
    /// closed registry section owns its spelling. The section matches remain
    /// short enough to review as bounded tables.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BoardRead
            | Self::ReportRead
            | Self::TaskDetail
            | Self::TaskRelations
            | Self::TaskTimeline
            | Self::BoardGraph
            | Self::BoardHandout
            | Self::DocLedger
            | Self::RoomPost
            | Self::RoomConversationText
            | Self::RoomLeave
            | Self::RoomContribute
            | Self::RoomContributionRead
            | Self::RoomControl
            | Self::WorkingGroupRead
            | Self::WorkingGroupMove
            | Self::WorkingGroupArrange
            | Self::WorkingGroupDisband
            | Self::WorkingGroupFast
            | Self::DesignPropose
            | Self::CandidateCritiqueRecord
            | Self::CandidateCoverageRecord
            | Self::PreparationDecide
            | Self::CandidateWithdraw
            | Self::OutputAdopt
            | Self::DesignNudge
            | Self::TaskAdd
            | Self::TaskOopsAdd
            | Self::TaskEdit
            | Self::TaskDelete
            | Self::TaskClose
            | Self::TaskConclude
            | Self::TaskNote
            | Self::TaskDefer
            | Self::TaskWake
            | Self::TaskFile
            | Self::TaskDepends
            | Self::TaskPhase
            | Self::TaskDone
            | Self::TaskReview
            | Self::TaskAssign => self.board_spelling(),
            Self::ClaimWin
            | Self::ClaimCarry
            | Self::ClaimRefresh
            | Self::ClaimRenew
            | Self::ClaimFree
            | Self::ClaimLapse
            | Self::DocApply
            | Self::SteerSend
            | Self::SteerInbox
            | Self::SteerHistory
            | Self::SteerResend
            | Self::SteerRecover
            | Self::SteerDispose
            | Self::SteerCancel
            | Self::QuoteRead
            | Self::ConsentRead
            | Self::ConsentPolicy
            | Self::ConsentDecide => self.coordination_spelling(),
            Self::TrustPend
            | Self::TrustGrant
            | Self::TrustRevoke
            | Self::LaneRead
            | Self::LaneJobsRead
            | Self::LaneJobRead
            | Self::LaneJobStop
            | Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneState
            | Self::LaneRenewal
            | Self::LaneRetune
            | Self::LaneEffortRetune
            | Self::LaneFastRetune
            | Self::TeamConfigure
            | Self::TeamArrange
            | Self::LaneRecall
            | Self::GlobalSettingsRead
            | Self::GlobalSettingsSet
            | Self::VendorCatalog
            | Self::TasteRecord
            | Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize
            | Self::ReleasePublish
            | Self::UpChange => self.runtime_spelling(),
            Self::PrincipalDirectoryRead
            | Self::PrincipalRead
            | Self::PrincipalAdmit
            | Self::PrincipalEdit
            | Self::PrincipalGrant
            | Self::PrincipalRevoke
            | Self::PrincipalDeauthorize => self.principal_spelling(),
        }
    }

    const fn board_spelling(self) -> &'static str {
        match self {
            Self::BoardRead => "board::read",
            Self::ReportRead => "report::read",
            Self::TaskDetail => "task::detail",
            Self::TaskRelations => "task::relations",
            Self::TaskTimeline => "task::timeline",
            Self::BoardGraph => "board::graph",
            Self::BoardHandout => "board::handout",
            Self::DocLedger => "doc::ledger",
            Self::RoomPost
            | Self::RoomConversationText
            | Self::RoomLeave
            | Self::RoomContribute
            | Self::RoomContributionRead
            | Self::RoomControl
            | Self::WorkingGroupRead
            | Self::WorkingGroupMove
            | Self::WorkingGroupArrange
            | Self::WorkingGroupDisband
            | Self::WorkingGroupFast => self.room_spelling(),
            Self::DesignPropose
            | Self::CandidateCritiqueRecord
            | Self::CandidateCoverageRecord
            | Self::PreparationDecide
            | Self::CandidateWithdraw
            | Self::OutputAdopt
            | Self::DesignNudge => self.preparation_spelling(),
            Self::TaskAdd => "task::add",
            Self::TaskOopsAdd => "task::oops-add",
            Self::TaskEdit => "task::edit",
            Self::TaskDelete => "task::delete",
            Self::TaskClose => "task::close",
            Self::TaskConclude => "task::conclude",
            Self::TaskNote => "task::note",
            Self::TaskDefer => "task::defer",
            Self::TaskWake => "task::wake",
            Self::TaskFile => "task::file",
            Self::TaskDepends => "task::depends",
            Self::TaskPhase => "task::phase",
            Self::TaskDone => "task::done",
            Self::TaskReview => "task::review",
            Self::TaskAssign => "task::assign",
            _ => panic!("command spelling partition is closed"),
        }
    }
    const fn room_spelling(self) -> &'static str {
        match self {
            Self::RoomPost => "room::post",
            Self::RoomConversationText => "room::conversation-text",
            Self::RoomLeave => "room::leave",
            Self::RoomContribute => "room::contribute",
            Self::RoomContributionRead => "room::contribution-read",
            Self::RoomControl => "room::control",
            Self::WorkingGroupRead => "group::read",
            Self::WorkingGroupMove => "group::move",
            Self::WorkingGroupArrange => "group::arrange",
            Self::WorkingGroupDisband => "group::disband",
            Self::WorkingGroupFast => "group::fast",
            _ => panic!("room command spelling partition is closed"),
        }
    }
    const fn preparation_spelling(self) -> &'static str {
        match self {
            Self::DesignPropose => "preparation::retain",
            Self::CandidateCritiqueRecord => "preparation::critique",
            Self::CandidateCoverageRecord => "preparation::cover",
            Self::PreparationDecide => "preparation::decide",
            Self::CandidateWithdraw => "preparation::withdraw",
            Self::OutputAdopt => "preparation::adopt",
            Self::DesignNudge => "preparation::nudge",
            _ => panic!("preparation command spelling partition is closed"),
        }
    }

    const fn coordination_spelling(self) -> &'static str {
        match self {
            Self::ClaimWin => "claim::win",
            Self::ClaimCarry => "claim::carry",
            Self::ClaimRefresh => "claim::refresh",
            Self::ClaimRenew => "claim::renew",
            Self::ClaimFree => "claim::free",
            Self::ClaimLapse => "claim::lapse",
            Self::DocApply => "doc::apply",
            Self::SteerSend => "steer::send",
            Self::SteerInbox => "steer::inbox",
            Self::SteerHistory => "steer::history",
            Self::SteerResend => "steer::resend",
            Self::SteerRecover => "steer::recover",
            Self::SteerDispose => "steer::dispose",
            Self::SteerCancel => "steer::cancel",
            Self::QuoteRead => "quote::read",
            Self::ConsentRead => "consent::read",
            Self::ConsentPolicy => "consent::policy",
            Self::ConsentDecide => "consent::decide",
            _ => panic!("command spelling partition is closed"),
        }
    }

    const fn principal_spelling(self) -> &'static str {
        match self {
            Self::PrincipalRead => "principal::read",
            Self::PrincipalAdmit => "principal::admit",
            Self::PrincipalEdit => "principal::edit",
            Self::PrincipalGrant => "principal::grant",
            Self::PrincipalRevoke => "principal::revoke",
            Self::PrincipalDirectoryRead => "principal::directory-read",
            Self::PrincipalDeauthorize => "principal::deauthorize",
            _ => panic!("command spelling partition is closed"),
        }
    }

    const fn runtime_spelling(self) -> &'static str {
        match self {
            Self::TrustPend => "trust::pend",
            Self::TrustGrant => "trust::grant",
            Self::TrustRevoke => "trust::revoke",
            Self::LaneRead => "lane::read",
            Self::LaneJobsRead => "lane::jobs-read",
            Self::LaneJobRead => "lane::job-read",
            Self::LaneJobStop => "lane::job-stop",
            Self::LaneSeat => "lane::seat",
            Self::LaneArrange => "lane::arrange",
            Self::LaneState => "lane::state",
            Self::LaneRenewal => "lane::renewal",
            Self::LaneRetune => "lane::retune",
            Self::LaneEffortRetune => "lane::effort-retune",
            Self::LaneFastRetune => "lane::fast-retune",
            Self::TeamConfigure => "team::configure",
            Self::TeamArrange => "team::arrange",
            Self::LaneRecall => "lane::recall",
            Self::GlobalSettingsRead => "settings::read",
            Self::GlobalSettingsSet => "settings::set",
            Self::VendorCatalog => "vendor::catalog",
            Self::TasteRecord => "taste::record",
            Self::CanaryRecognize => "canary::recognize",
            Self::VendorRecognize => "vendor::recognize",
            Self::CommitRecognize => "commit::recognize",
            Self::ProofRecognize => "proof::recognize",
            Self::ReleasePublish => "release::publish",
            Self::UpChange => "up::change",
            _ => panic!("command spelling partition is closed"),
        }
    }

    /// Whether this request reads one snapshot and commits no authority fact.
    #[must_use]
    pub const fn is_read(self) -> bool {
        matches!(
            self,
            Self::BoardRead
                | Self::ReportRead
                | Self::TaskDetail
                | Self::TaskRelations
                | Self::TaskTimeline
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::GlobalSettingsRead
                | Self::VendorCatalog
                | Self::SteerInbox
                | Self::QuoteRead
                | Self::SteerHistory
                | Self::ConsentRead
                | Self::ConsentPolicy
                | Self::LaneRead
                | Self::LaneJobsRead
                | Self::LaneJobRead
                | Self::WorkingGroupRead
                | Self::RoomConversationText
                | Self::RoomContributionRead
                | Self::LaneRecall
                | Self::PrincipalRead
        )
    }
}

impl FromStr for CommandKind {
    type Err = CommandKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|kind| kind.as_str() == text)
            .copied()
            .ok_or_else(|| CommandKindError(text.to_owned()))
    }
}

impl fmt::Display for CommandKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CommandKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CommandKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}
