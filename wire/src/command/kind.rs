//! Closed command spellings and witness-law dispatch.

use std::{fmt, str::FromStr};

use serde::de::{Deserializer, Error as _};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use super::{CommandKind, CommandKindError, Law, LawError};
use crate::intent::Intent;
use crate::witness::{Witness, WitnessKind};

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
        Self::RoomCoordinationRead,
        Self::RoomCoordinate,
        Self::RoomPutAside,
        Self::WorkingGroupRead,
        Self::WorkingGroupPut,
        Self::WorkingGroupRemove,
        Self::GlobalSettingsRead,
        Self::GlobalSettingsSet,
        Self::VendorCatalog,
        Self::TaskAdd,
        Self::TaskOopsAdd,
        Self::TasteProposalAdd,
        Self::TaskEdit,
        Self::TaskDelete,
        Self::TaskClose,
        Self::TaskNote,
        Self::TaskDefer,
        Self::TaskWake,
        Self::TaskDepends,
        Self::TaskPhase,
        Self::TaskDone,
        Self::TaskReview,
        Self::TaskAssign,
        Self::ClaimWin,
        Self::ClaimCarry,
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
            | Self::RoomCoordinationRead
            | Self::RoomCoordinate
            | Self::RoomPutAside
            | Self::WorkingGroupRead
            | Self::WorkingGroupPut
            | Self::WorkingGroupRemove
            | Self::TaskAdd
            | Self::TaskOopsAdd
            | Self::TasteProposalAdd
            | Self::TaskEdit
            | Self::TaskDelete
            | Self::TaskClose
            | Self::TaskNote
            | Self::TaskDefer
            | Self::TaskWake
            | Self::TaskDepends
            | Self::TaskPhase
            | Self::TaskDone
            | Self::TaskReview
            | Self::TaskAssign => self.board_spelling(),
            Self::ClaimWin
            | Self::ClaimCarry
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
            | Self::RoomCoordinationRead
            | Self::RoomCoordinate
            | Self::RoomPutAside
            | Self::WorkingGroupRead
            | Self::WorkingGroupPut
            | Self::WorkingGroupRemove => self.room_spelling(),
            Self::TaskAdd => "task::add",
            Self::TaskOopsAdd => "task::oops-add",
            Self::TasteProposalAdd => "taste::proposal-add",
            Self::TaskEdit => "task::edit",
            Self::TaskDelete => "task::delete",
            Self::TaskClose => "task::close",
            Self::TaskNote => "task::note",
            Self::TaskDefer => "task::defer",
            Self::TaskWake => "task::wake",
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
            Self::RoomCoordinationRead => "room::coordination-read",
            Self::RoomCoordinate => "room::coordinate",
            Self::RoomPutAside => "room::put-aside",
            Self::WorkingGroupRead => "group::read",
            Self::WorkingGroupPut => "group::put",
            Self::WorkingGroupRemove => "group::remove",
            _ => panic!("room command spelling partition is closed"),
        }
    }

    const fn coordination_spelling(self) -> &'static str {
        match self {
            Self::ClaimWin => "claim::win",
            Self::ClaimCarry => "claim::carry",
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

    /// The witness law this command carries.
    ///
    /// This is the registry. The match is exhaustive, so a command added
    /// without an explicit row fails to compile — the law cannot be forgotten
    /// or defaulted, only stated. Each row it names states its predicates in
    /// full rather than deriving them from a neighbour, so the law governing
    /// one command is read in one place.
    #[must_use]
    pub const fn law(self) -> Law {
        match self {
            Self::BoardRead
            | Self::ReportRead
            | Self::TaskDetail
            | Self::TaskRelations
            | Self::TaskTimeline
            | Self::BoardGraph
            | Self::DocLedger => Law::READ,
            Self::WorkingGroupRead => Law::OPERATOR_READ,
            Self::WorkingGroupPut | Self::WorkingGroupRemove => Law::GROUP,
            Self::GlobalSettingsRead | Self::VendorCatalog => Law::OPERATOR_READ,
            Self::GlobalSettingsSet => Law::SETTINGS,
            Self::BoardHandout => Law::OPERATOR_READ,
            Self::RoomPost => Law::ROOM_POST,
            Self::RoomCoordinationRead => Law::OPERATOR_READ,
            Self::RoomCoordinate => Law::ROOM_COORDINATE,
            Self::RoomPutAside => Law::ROOM_PUT_ASIDE,
            Self::TaskAdd => Law::TASK_ADD,
            Self::TaskOopsAdd | Self::TasteProposalAdd => Law::ADD,
            Self::TaskEdit => Law::EDIT,
            Self::TaskDelete => Law::DELETE,
            Self::TaskClose => Law::CLOSE,
            Self::TaskNote => Law::NOTE,
            Self::TaskDefer => Law::DEFER,
            Self::TaskWake => Law::WAKE,
            Self::TaskDepends => Law::DEPENDS,
            Self::TaskPhase | Self::TaskDone => Law::PROGRESS,
            Self::TaskReview => Law::REVIEW,
            Self::TaskAssign => Law::ASSIGN,
            Self::ClaimWin => Law::WIN,
            Self::ClaimFree | Self::ClaimLapse => Law::FREE,
            Self::ClaimCarry | Self::ClaimRenew => Law::POSSESSION,
            _ => self.non_task_law(),
        }
    }

    pub(super) fn required_for_intent(self, body: &Intent) -> &'static [WitnessKind] {
        if matches!(
            (self, body),
            (Self::TaskAdd, Intent::TaskAdd(add)) if add.claim
        ) {
            return &[
                WitnessKind::Absence,
                WitnessKind::Eligibility,
                WitnessKind::Residency,
            ];
        }
        if let (Self::TaskWake, Intent::TaskWake(wake)) = (self, body) {
            return match (wake.into.is_some(), wake.claim) {
                (true, true) => &[
                    WitnessKind::Workflow,
                    WitnessKind::Deferral,
                    WitnessKind::Task,
                    WitnessKind::Eligibility,
                    WitnessKind::Residency,
                ],
                (true, false) => &[
                    WitnessKind::Workflow,
                    WitnessKind::Deferral,
                    WitnessKind::Task,
                ],
                (false, true) => &[
                    WitnessKind::Workflow,
                    WitnessKind::Deferral,
                    WitnessKind::Eligibility,
                    WitnessKind::Residency,
                ],
                (false, false) => Law::WAKE.required,
            };
        }
        if matches!(
            (self, body),
            (Self::TaskEdit, Intent::TaskEdit(edit)) if edit.requirements.is_some()
        ) {
            return &[WitnessKind::Task, WitnessKind::Claim];
        }
        self.law().required
    }

    pub(super) fn witnessed_intent(
        self,
        expected: &[Witness],
        body: &Intent,
    ) -> Result<(), LawError> {
        for required in self.required_for_intent(body) {
            if !expected.iter().any(|witness| witness.kind() == *required) {
                return Err(LawError::Missing {
                    command: self,
                    missing: *required,
                });
            }
        }
        if let (
            Self::TaskWake,
            Intent::TaskWake(wake @ crate::intent::TaskWake { into: Some(_), .. }),
        ) = (self, body)
        {
            let target = wake.task.born().ok();
            let target_is_witnessed = expected.iter().any(|witness| {
                matches!(
                    (witness, target.as_ref()),
                    (Witness::Task { task, .. }, Some(target)) if task == target
                )
            });
            if !target_is_witnessed {
                return Err(LawError::SubjectMismatch {
                    command: self,
                    kind: WitnessKind::Task,
                    target: wake.task.to_string(),
                });
            }
        }
        self.witnessed_room(expected, body)?;
        // Requirement and intent edits are guarded authority changes, so the
        // statement witness they carry must name the edited task itself.
        if let (Self::TaskEdit, Intent::TaskEdit(edit)) = (self, body)
            && (edit.requirements.is_some() || edit.intent.is_some())
        {
            let target = edit.task.born().ok();
            let kinds: &[WitnessKind] = if edit.requirements.is_some() {
                &[WitnessKind::Task, WitnessKind::Claim]
            } else {
                &[WitnessKind::Task]
            };
            for &kind in kinds {
                let target_is_witnessed = expected.iter().any(|witness| {
                    matches!(
                        (kind, witness, target.as_ref()),
                        (WitnessKind::Task, Witness::Task { task, .. }, Some(target))
                            | (WitnessKind::Claim, Witness::Claim { task, .. }, Some(target))
                            if task == target
                    )
                });
                if !target_is_witnessed {
                    return Err(LawError::SubjectMismatch {
                        command: self,
                        kind,
                        target: edit.task.to_string(),
                    });
                }
            }
        }
        self.witnessed_group(expected, body)?;
        Ok(())
    }

    fn witnessed_room(self, expected: &[Witness], body: &Intent) -> Result<(), LawError> {
        let room = match (self, body) {
            (Self::RoomPost, Intent::RoomPost(post)) => Some(&post.room),
            (Self::RoomCoordinate, Intent::RoomCoordinate(request)) => Some(&request.room),
            (Self::RoomPutAside, Intent::RoomPutAside(request)) => Some(&request.room),
            _ => None,
        };
        let Some(room) = room else {
            return Ok(());
        };
        let target = room.born().ok();
        let kinds: &[WitnessKind] = if self == Self::RoomPost {
            &[WitnessKind::Task]
        } else {
            &[WitnessKind::Task, WitnessKind::Workflow, WitnessKind::Claim]
        };
        for &kind in kinds {
            let matched = expected.iter().any(|witness| {
                matches!(
                    (kind, witness, target.as_ref()),
                    (WitnessKind::Task, Witness::Task { task, .. }, Some(target))
                        | (WitnessKind::Workflow, Witness::Workflow { task, .. }, Some(target))
                        | (WitnessKind::Claim, Witness::Claim { task, .. }, Some(target))
                        if task == target
                )
            });
            if !matched {
                return Err(LawError::SubjectMismatch {
                    command: self,
                    kind,
                    target: room.to_string(),
                });
            }
        }
        Ok(())
    }

    fn witnessed_group(self, expected: &[Witness], body: &Intent) -> Result<(), LawError> {
        let group = match (self, body) {
            (Self::WorkingGroupPut, Intent::WorkingGroupPut(put)) => Some(&put.group.group),
            (Self::WorkingGroupRemove, Intent::WorkingGroupRemove(remove)) => Some(&remove.group),
            _ => None,
        };
        if let Some(group) = group
            && !expected.iter().any(
                |witness| matches!(witness, Witness::Group { group: witnessed, .. } if witnessed == group),
            )
        {
            return Err(LawError::SubjectMismatch {
                command: self,
                kind: WitnessKind::Group,
                target: group.to_string(),
            });
        }
        let (Self::WorkingGroupPut, Intent::WorkingGroupPut(put)) = (self, body) else {
            return Ok(());
        };
        for anchor in &put.group.anchors {
            if !expected
                .iter()
                .any(|witness| matches!(witness, Witness::Existence { task } if task == anchor))
            {
                return Err(LawError::SubjectMismatch {
                    command: self,
                    kind: WitnessKind::Existence,
                    target: anchor.to_string(),
                });
            }
        }
        if expected.iter().any(|witness| {
            matches!(witness, Witness::Existence { task } if !put.group.anchors.contains(task))
        }) {
            return Err(LawError::SubjectMismatch {
                command: self,
                kind: WitnessKind::Existence,
                target: put.group.group.to_string(),
            });
        }
        Ok(())
    }

    const fn non_task_law(self) -> Law {
        match self {
            Self::RoomPost => Law::ROOM_POST,
            Self::RoomCoordinationRead => Law::OPERATOR_READ,
            Self::RoomCoordinate => Law::ROOM_COORDINATE,
            Self::RoomPutAside => Law::ROOM_PUT_ASIDE,
            Self::DocApply => Law::APPLY,
            Self::SteerSend => Law::SEND,
            Self::SteerInbox | Self::QuoteRead => Law::READ,
            Self::SteerHistory => Law::OPERATOR_READ,
            Self::SteerResend => Law::RESEND,
            Self::SteerRecover => Law::RECOVER,
            Self::SteerDispose => Law::DISPOSE,
            Self::SteerCancel => Law::CANCEL,
            Self::ConsentRead | Self::ConsentPolicy => Law::INSPECT,
            Self::ConsentDecide => Law::DECIDE,
            Self::TrustPend => Law::PEND,
            Self::TrustGrant | Self::TrustRevoke => Law::TRUST,
            Self::LaneRead => Law::LANE_READ,
            Self::LaneJobsRead | Self::LaneJobRead => Law::OPERATOR_READ,
            Self::LaneJobStop => Law::JOB_STOP,
            Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneState
            | Self::LaneRenewal
            | Self::LaneRetune
            | Self::LaneEffortRetune
            | Self::LaneFastRetune
            | Self::TeamConfigure
            | Self::TeamArrange => Law::LANE,
            Self::LaneRecall => Law::LANE_RECALL,
            Self::TasteRecord => Law::TASTE,
            Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize => Law::RECOGNIZE,
            Self::ReleasePublish => Law::PUBLISH,
            Self::UpChange => Law::UP,
            _ => panic!("command law partition is closed"),
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
                | Self::LaneRecall
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
