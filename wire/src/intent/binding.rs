//! Closed command body and answer bindings.

use serde::Serialize;

use crate::command::CommandKind;

use super::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, ClaimAnswer, ClaimCarry, ClaimRenew,
    ClaimWin, ConsentAnswer, ConsentDecide, ConsentPolicy, ConsentPolicyAnswer, ConsentQueueAnswer,
    ConsentRead, DirectionHistoryAnswer, DirectiveAnswer, DirectiveRef, DocAnswer, DocApply,
    DocLedger, GlobalSettingsAnswer, GlobalSettingsRead, GlobalSettingsSet, InboxAnswer,
    LaneAnswer, LaneArrange, LaneChangeAnswer, LaneEffortRetune, LaneRead, LaneRecall, LaneRenewal,
    LaneRenewalAnswer, LaneRetune, LaneSeat, LaneState, RecallAnswer, RecognitionAnswer, Recognize,
    ReleasePublish, ReportAnswer, ReportRead, StateAnswer, SteerDispose, SteerHistory, SteerInbox,
    SteerSend, SteerTell, TaskAdd, TaskAnswer, TaskAssign, TaskClose, TaskDefer, TaskDelete,
    TaskDepends, TaskDetail, TaskDetailAnswer, TaskDone, TaskEdit, TaskNote, TaskOopsAdd,
    TaskPhase, TaskRef, TaskRelations, TaskRelationsAnswer, TaskReview, TaskTimeline,
    TaskTimelineAnswer, TaskWake, TasteAnswer, TasteProposalAdd, TasteRecord, TeamArrange,
    TeamConfigure, TextAnswer, TrustAnswer, TrustRef, UpChange, VendorCatalog, VendorCatalogAnswer,
};

/// Which intent DTO a command kind's body decodes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyShape {
    /// [`BoardRead`]
    BoardRead,
    /// [`ReportRead`]
    ReportRead,
    /// [`TaskDetail`]
    TaskDetail,
    /// [`TaskRelations`]
    TaskRelations,
    /// [`TaskTimeline`]
    TaskTimeline,
    /// [`BoardGraph`]
    BoardGraph,
    /// [`BoardHandout`]
    BoardHandout,
    /// [`DocLedger`]
    DocLedger,
    /// [`GlobalSettingsRead`]
    GlobalSettingsRead,
    /// [`GlobalSettingsSet`]
    GlobalSettingsSet,
    /// [`VendorCatalog`]
    VendorCatalog,
    /// [`TaskAdd`]
    TaskAdd,
    /// [`TaskOopsAdd`], the strict body for the server-owned `.oops` route.
    TaskOopsAdd,
    /// [`TasteProposalAdd`], the strict body for the hidden taste-proposal route.
    TasteProposalAdd,
    /// [`TaskEdit`]
    TaskEdit,
    /// [`TaskDelete`]
    TaskDelete,
    /// [`TaskClose`]
    TaskClose,
    /// [`TaskNote`]
    TaskNote,
    /// [`TaskDefer`]
    TaskDefer,
    /// [`TaskWake`]
    TaskWake,
    /// [`TaskRef`]
    TaskRef,
    /// [`TaskDepends`]
    TaskDepends,
    /// [`TaskPhase`]
    TaskPhase,
    /// [`TaskDone`]
    TaskDone,
    /// [`TaskReview`]
    TaskReview,
    /// [`TaskAssign`]
    TaskAssign,
    /// [`ClaimWin`]
    ClaimWin,
    /// [`ClaimCarry`]
    ClaimCarry,
    /// [`ClaimRenew`], which carries a lease duration beyond a plain task reference.
    ClaimRenew,
    /// [`DocApply`]
    DocApply,
    /// [`SteerSend`]
    SteerSend,
    /// [`SteerTell`]
    SteerTell,
    /// [`SteerInbox`]
    SteerInbox,
    /// [`SteerHistory`]
    SteerHistory,
    /// [`DirectiveRef`]
    DirectiveRef,
    /// [`SteerDispose`]
    SteerDispose,
    /// [`ConsentDecide`]
    ConsentDecide,
    /// [`ConsentRead`]
    ConsentRead,
    /// [`ConsentPolicy`]
    ConsentPolicy,
    /// [`TrustRef`]
    TrustRef,
    /// [`LaneRead`]
    LaneRead,
    /// [`LaneSeat`]
    LaneSeat,
    /// [`LaneArrange`]
    LaneArrange,
    /// [`LaneState`]
    LaneState,
    /// [`LaneRenewal`]
    LaneRenewal,
    /// [`LaneRetune`]
    LaneRetune,
    /// [`LaneEffortRetune`]
    LaneEffortRetune,
    /// [`TeamConfigure`]
    TeamConfigure,
    /// [`TeamArrange`]
    TeamArrange,
    /// [`LaneRecall`]
    LaneRecall,
    /// [`TasteRecord`]
    TasteRecord,
    /// [`Recognize`]
    Recognize,
    /// [`ReleasePublish`]
    ReleasePublish,
    /// [`UpChange`]
    UpChange,
}

/// Which answer DTO a command kind's reply carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnswerShape {
    /// [`BoardAnswer`]
    Board,
    /// [`ReportAnswer`]
    Report,
    /// [`TaskDetailAnswer`]
    TaskDetail,
    /// [`TaskRelationsAnswer`]
    TaskRelations,
    /// [`TaskTimelineAnswer`]
    TaskTimeline,
    /// [`TextAnswer`]
    Text,
    /// [`TaskAnswer`]
    Task,
    /// [`ClaimAnswer`]
    Claim,
    /// [`DocAnswer`]
    Doc,
    /// [`DirectiveAnswer`]
    Directive,
    /// [`InboxAnswer`]
    Inbox,
    /// [`DirectionHistoryAnswer`]
    DirectionHistory,
    /// [`ConsentAnswer`]
    Consent,
    /// [`ConsentQueueAnswer`]
    ConsentQueue,
    /// [`ConsentPolicyAnswer`]
    ConsentPolicy,
    /// [`GlobalSettingsAnswer`]
    GlobalSettings,
    /// [`VendorCatalogAnswer`]
    VendorCatalog,
    /// [`TrustAnswer`]
    Trust,
    /// [`LaneAnswer`]
    Lane,
    /// [`LaneChangeAnswer`]
    LaneChange,
    /// [`LaneRenewalAnswer`]
    LaneRenewal,
    /// [`StateAnswer`]
    State,
    /// [`RecallAnswer`]
    Recall,
    /// [`TasteAnswer`]
    Taste,
    /// [`RecognitionAnswer`]
    Recognition,
}

/// One decoded successful reply body, tagged by the shape its command selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CommandAnswer {
    /// [`AnswerShape::Board`]
    Board(Box<BoardAnswer>),
    /// [`AnswerShape::Report`]
    Report(Box<ReportAnswer>),
    /// [`AnswerShape::TaskDetail`]
    TaskDetail(Box<TaskDetailAnswer>),
    /// [`AnswerShape::TaskRelations`]
    TaskRelations(Box<TaskRelationsAnswer>),
    /// [`AnswerShape::TaskTimeline`]
    TaskTimeline(Box<TaskTimelineAnswer>),
    /// [`AnswerShape::Text`]
    Text(TextAnswer),
    /// [`AnswerShape::Task`]
    Task(TaskAnswer),
    /// [`AnswerShape::Claim`]
    Claim(Box<ClaimAnswer>),
    /// [`AnswerShape::Doc`]
    Doc(DocAnswer),
    /// [`AnswerShape::Directive`]
    Directive(DirectiveAnswer),
    /// [`AnswerShape::Inbox`]
    Inbox(InboxAnswer),
    /// [`AnswerShape::DirectionHistory`]
    DirectionHistory(Box<DirectionHistoryAnswer>),
    /// [`AnswerShape::Consent`]
    Consent(ConsentAnswer),
    /// [`AnswerShape::ConsentQueue`]
    ConsentQueue(ConsentQueueAnswer),
    /// [`AnswerShape::ConsentPolicy`]
    ConsentPolicy(ConsentPolicyAnswer),
    /// [`AnswerShape::GlobalSettings`]
    GlobalSettings(GlobalSettingsAnswer),
    /// [`AnswerShape::VendorCatalog`]
    VendorCatalog(VendorCatalogAnswer),
    /// [`AnswerShape::Trust`]
    Trust(TrustAnswer),
    /// [`AnswerShape::Lane`]
    Lane(Box<LaneAnswer>),
    /// [`AnswerShape::LaneChange`]
    LaneChange(Box<LaneChangeAnswer>),
    /// [`AnswerShape::LaneRenewal`]
    LaneRenewal(Box<LaneRenewalAnswer>),
    /// [`AnswerShape::State`]
    State(StateAnswer),
    /// [`AnswerShape::Recall`]
    Recall(Box<RecallAnswer>),
    /// [`AnswerShape::Taste`]
    Taste(TasteAnswer),
    /// [`AnswerShape::Recognition`]
    Recognition(RecognitionAnswer),
}

impl CommandAnswer {
    /// The shape this decoded answer carries.
    #[must_use]
    pub const fn shape(&self) -> AnswerShape {
        match self {
            Self::Board(_) => AnswerShape::Board,
            Self::Report(_) => AnswerShape::Report,
            Self::TaskDetail(_) => AnswerShape::TaskDetail,
            Self::TaskRelations(_) => AnswerShape::TaskRelations,
            Self::TaskTimeline(_) => AnswerShape::TaskTimeline,
            Self::Text(_) => AnswerShape::Text,
            Self::Task(_) => AnswerShape::Task,
            Self::Claim(_) => AnswerShape::Claim,
            Self::Doc(_) => AnswerShape::Doc,
            Self::Directive(_) => AnswerShape::Directive,
            Self::Inbox(_) => AnswerShape::Inbox,
            Self::DirectionHistory(_) => AnswerShape::DirectionHistory,
            Self::Consent(_) => AnswerShape::Consent,
            Self::ConsentQueue(_) => AnswerShape::ConsentQueue,
            Self::ConsentPolicy(_) => AnswerShape::ConsentPolicy,
            Self::GlobalSettings(_) => AnswerShape::GlobalSettings,
            Self::VendorCatalog(_) => AnswerShape::VendorCatalog,
            Self::Trust(_) => AnswerShape::Trust,
            Self::Lane(_) => AnswerShape::Lane,
            Self::LaneChange(_) => AnswerShape::LaneChange,
            Self::LaneRenewal(_) => AnswerShape::LaneRenewal,
            Self::State(_) => AnswerShape::State,
            Self::Recall(_) => AnswerShape::Recall,
            Self::Taste(_) => AnswerShape::Taste,
            Self::Recognition(_) => AnswerShape::Recognition,
        }
    }
}

impl AnswerShape {
    /// Decode a successful reply body into the answer this shape names.
    ///
    /// # Errors
    ///
    /// Returns the decoder error when the bytes are not that shape. Answers
    /// refuse unknown fields, so a body spelled for the wrong command is a
    /// decode failure rather than a silently truncated value.
    pub fn decode(self, bytes: &[u8]) -> Result<CommandAnswer, serde_json::Error> {
        match self {
            Self::Board
            | Self::Report
            | Self::TaskDetail
            | Self::TaskRelations
            | Self::TaskTimeline
            | Self::Text
            | Self::Task
            | Self::Claim
            | Self::Doc
            | Self::Directive
            | Self::Inbox
            | Self::Consent
            | Self::ConsentQueue => self.decode_primary(bytes),
            Self::ConsentPolicy
            | Self::GlobalSettings
            | Self::VendorCatalog
            | Self::Trust
            | Self::Lane
            | Self::LaneChange
            | Self::LaneRenewal
            | Self::State
            | Self::Recall
            | Self::Taste
            | Self::Recognition
            | Self::DirectionHistory => self.decode_secondary(bytes),
        }
    }

    fn decode_primary(self, bytes: &[u8]) -> Result<CommandAnswer, serde_json::Error> {
        Ok(match self {
            Self::Board => CommandAnswer::Board(Box::new(crate::decode(bytes)?)),
            Self::Report => CommandAnswer::Report(Box::new(crate::decode(bytes)?)),
            Self::TaskDetail => CommandAnswer::TaskDetail(Box::new(crate::decode(bytes)?)),
            Self::TaskRelations => CommandAnswer::TaskRelations(Box::new(crate::decode(bytes)?)),
            Self::TaskTimeline => CommandAnswer::TaskTimeline(Box::new(crate::decode(bytes)?)),
            Self::Text => CommandAnswer::Text(crate::decode(bytes)?),
            Self::Task => CommandAnswer::Task(crate::decode(bytes)?),
            Self::Claim => CommandAnswer::Claim(Box::new(crate::decode(bytes)?)),
            Self::Doc => CommandAnswer::Doc(crate::decode(bytes)?),
            Self::Directive => CommandAnswer::Directive(crate::decode(bytes)?),
            Self::Inbox => CommandAnswer::Inbox(crate::decode(bytes)?),
            Self::Consent => CommandAnswer::Consent(crate::decode(bytes)?),
            Self::ConsentQueue => CommandAnswer::ConsentQueue(crate::decode(bytes)?),
            _ => unreachable!("answer-shape decode partition is closed"),
        })
    }

    fn decode_secondary(self, bytes: &[u8]) -> Result<CommandAnswer, serde_json::Error> {
        Ok(match self {
            Self::ConsentPolicy => CommandAnswer::ConsentPolicy(crate::decode(bytes)?),
            Self::GlobalSettings => CommandAnswer::GlobalSettings(crate::decode(bytes)?),
            Self::VendorCatalog => CommandAnswer::VendorCatalog(crate::decode(bytes)?),
            Self::Trust => CommandAnswer::Trust(crate::decode(bytes)?),
            Self::Lane => CommandAnswer::Lane(Box::new(crate::decode(bytes)?)),
            Self::LaneChange => CommandAnswer::LaneChange(Box::new(crate::decode(bytes)?)),
            Self::LaneRenewal => CommandAnswer::LaneRenewal(Box::new(crate::decode(bytes)?)),
            Self::State => CommandAnswer::State(crate::decode(bytes)?),
            Self::Recall => CommandAnswer::Recall(Box::new(crate::decode(bytes)?)),
            Self::Taste => CommandAnswer::Taste(crate::decode(bytes)?),
            Self::Recognition => CommandAnswer::Recognition(crate::decode(bytes)?),
            Self::DirectionHistory => {
                CommandAnswer::DirectionHistory(Box::new(crate::decode(bytes)?))
            }
            _ => unreachable!("answer-shape decode partition is closed"),
        })
    }
}

/// The body and answer a command kind selects.
///
/// WIRE binds these together with the witness schema and the transition, so
/// they are read off one discriminant rather than three independent lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Intent DTO the caller sends.
    pub body: BodyShape,
    /// Answer DTO a successful reply carries.
    pub answer: AnswerShape,
}

impl Binding {
    /// Bind a body to an answer.
    const fn of(body: BodyShape, answer: AnswerShape) -> Self {
        Self { body, answer }
    }
}

impl CommandKind {
    /// The body and answer this kind selects.
    ///
    /// Exhaustive by construction: a kind added without a row here does not
    /// compile, which is the same law the witness registry carries.
    #[must_use]
    pub const fn binding(self) -> Binding {
        match self {
            Self::BoardRead
            | Self::ReportRead
            | Self::TaskDetail
            | Self::TaskRelations
            | Self::TaskTimeline
            | Self::BoardGraph
            | Self::BoardHandout
            | Self::DocLedger
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
            | Self::TaskAssign
            | Self::ClaimWin
            | Self::ClaimCarry
            | Self::ClaimRenew
            | Self::ClaimFree
            | Self::ClaimLapse
            | Self::DocApply => self.board_binding(),
            Self::SteerSend
            | Self::SteerTell
            | Self::SteerInbox
            | Self::SteerHistory
            | Self::SteerResend
            | Self::SteerRecover
            | Self::SteerDispose
            | Self::SteerCancel
            | Self::ConsentRead
            | Self::ConsentPolicy
            | Self::ConsentDecide
            | Self::TrustPend
            | Self::TrustGrant
            | Self::TrustRevoke
            | Self::LaneRead
            | Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneState
            | Self::LaneRenewal
            | Self::LaneRetune
            | Self::LaneEffortRetune
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
            | Self::UpChange => self.runtime_binding(),
        }
    }

    const fn board_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::BoardRead => Binding::of(B::BoardRead, A::Board),
            Self::ReportRead => Binding::of(B::ReportRead, A::Report),
            Self::TaskDetail => Binding::of(B::TaskDetail, A::TaskDetail),
            Self::TaskRelations => Binding::of(B::TaskRelations, A::TaskRelations),
            Self::TaskTimeline => Binding::of(B::TaskTimeline, A::TaskTimeline),
            Self::BoardGraph => Binding::of(B::BoardGraph, A::Text),
            Self::BoardHandout => Binding::of(B::BoardHandout, A::Text),
            Self::DocLedger => Binding::of(B::DocLedger, A::Text),
            Self::TaskAdd => Binding::of(B::TaskAdd, A::Task),
            Self::TaskOopsAdd => Binding::of(B::TaskOopsAdd, A::Task),
            Self::TasteProposalAdd => Binding::of(B::TasteProposalAdd, A::Task),
            Self::TaskEdit => Binding::of(B::TaskEdit, A::Task),
            Self::TaskDelete => Binding::of(B::TaskDelete, A::Task),
            Self::TaskClose => Binding::of(B::TaskClose, A::Task),
            Self::TaskNote => Binding::of(B::TaskNote, A::Task),
            Self::TaskDefer => Binding::of(B::TaskDefer, A::Task),
            Self::TaskWake => Binding::of(B::TaskWake, A::Task),
            Self::TaskDepends => Binding::of(B::TaskDepends, A::Task),
            Self::TaskPhase => Binding::of(B::TaskPhase, A::Task),
            Self::TaskDone => Binding::of(B::TaskDone, A::Task),
            Self::TaskReview => Binding::of(B::TaskReview, A::Task),
            Self::TaskAssign => Binding::of(B::TaskAssign, A::Task),
            Self::ClaimWin => Binding::of(B::ClaimWin, A::Claim),
            Self::ClaimCarry => Binding::of(B::ClaimCarry, A::Claim),
            Self::ClaimRenew => Binding::of(B::ClaimRenew, A::Claim),
            Self::ClaimFree | Self::ClaimLapse => Binding::of(B::TaskRef, A::Claim),
            Self::DocApply => Binding::of(B::DocApply, A::Doc),
            _ => panic!("command binding partition is closed"),
        }
    }

    const fn runtime_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::SteerSend => Binding::of(B::SteerSend, A::Directive),
            Self::SteerTell => Binding::of(B::SteerTell, A::Directive),
            Self::SteerInbox => Binding::of(B::SteerInbox, A::Inbox),
            Self::SteerHistory => Binding::of(B::SteerHistory, A::DirectionHistory),
            Self::SteerResend | Self::SteerRecover | Self::SteerCancel => {
                Binding::of(B::DirectiveRef, A::Directive)
            }
            Self::SteerDispose => Binding::of(B::SteerDispose, A::Directive),
            Self::ConsentDecide => Binding::of(B::ConsentDecide, A::Consent),
            Self::ConsentRead => Binding::of(B::ConsentRead, A::ConsentQueue),
            Self::ConsentPolicy => Binding::of(B::ConsentPolicy, A::ConsentPolicy),
            Self::TrustPend | Self::TrustGrant | Self::TrustRevoke => {
                Binding::of(B::TrustRef, A::Trust)
            }
            Self::LaneRead => Binding::of(B::LaneRead, A::Lane),
            Self::LaneSeat => Binding::of(B::LaneSeat, A::LaneChange),
            Self::LaneArrange => Binding::of(B::LaneArrange, A::Lane),
            Self::LaneState => Binding::of(B::LaneState, A::LaneChange),
            Self::LaneRenewal => Binding::of(B::LaneRenewal, A::LaneRenewal),
            Self::LaneRetune => Binding::of(B::LaneRetune, A::Lane),
            Self::LaneEffortRetune => Binding::of(B::LaneEffortRetune, A::Lane),
            Self::TeamConfigure => Binding::of(B::TeamConfigure, A::Lane),
            Self::TeamArrange => Binding::of(B::TeamArrange, A::Lane),
            Self::LaneRecall => Binding::of(B::LaneRecall, A::Recall),
            Self::GlobalSettingsRead | Self::GlobalSettingsSet => Binding::of(
                if matches!(self, Self::GlobalSettingsRead) {
                    B::GlobalSettingsRead
                } else {
                    B::GlobalSettingsSet
                },
                A::GlobalSettings,
            ),
            Self::VendorCatalog => Binding::of(B::VendorCatalog, A::VendorCatalog),
            Self::TasteRecord => Binding::of(B::TasteRecord, A::Taste),
            Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize => Binding::of(B::Recognize, A::Recognition),
            Self::ReleasePublish => Binding::of(B::ReleasePublish, A::Recognition),
            Self::UpChange => Binding::of(B::UpChange, A::State),
            _ => panic!("command binding partition is closed"),
        }
    }
}

/// One decoded command body, tagged by the shape its kind selected.
///
/// The resident decodes a frame before it knows which handler runs, so the
/// union exists to carry a body that is already typed but not yet dispatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Intent {
    /// [`BodyShape::BoardRead`]
    BoardRead(BoardRead),
    /// [`BodyShape::ReportRead`]
    ReportRead(ReportRead),
    /// [`BodyShape::TaskDetail`]
    TaskDetail(TaskDetail),
    /// [`BodyShape::TaskRelations`]
    TaskRelations(TaskRelations),
    /// [`BodyShape::TaskTimeline`]
    TaskTimeline(TaskTimeline),
    /// [`BodyShape::BoardGraph`]
    BoardGraph(BoardGraph),
    /// [`BodyShape::BoardHandout`]
    BoardHandout(BoardHandout),
    /// [`BodyShape::DocLedger`]
    DocLedger(DocLedger),
    /// [`BodyShape::GlobalSettingsRead`]
    GlobalSettingsRead(GlobalSettingsRead),
    /// [`BodyShape::GlobalSettingsSet`]
    GlobalSettingsSet(GlobalSettingsSet),
    /// [`BodyShape::VendorCatalog`]
    VendorCatalog(VendorCatalog),
    /// [`BodyShape::TaskAdd`]
    TaskAdd(TaskAdd),
    /// [`BodyShape::TaskOopsAdd`]
    TaskOopsAdd(TaskOopsAdd),
    /// [`BodyShape::TasteProposalAdd`]
    TasteProposalAdd(TasteProposalAdd),
    /// [`BodyShape::TaskEdit`]
    TaskEdit(TaskEdit),
    /// [`BodyShape::TaskDelete`]
    TaskDelete(TaskDelete),
    /// [`BodyShape::TaskClose`]
    TaskClose(TaskClose),
    /// [`BodyShape::TaskNote`]
    TaskNote(TaskNote),
    /// [`BodyShape::TaskDefer`]
    TaskDefer(TaskDefer),
    /// [`BodyShape::TaskWake`]
    TaskWake(TaskWake),
    /// [`BodyShape::TaskRef`]
    TaskRef(TaskRef),
    /// [`BodyShape::TaskDepends`]
    TaskDepends(TaskDepends),
    /// [`BodyShape::TaskPhase`]
    TaskPhase(TaskPhase),
    /// [`BodyShape::TaskDone`]
    TaskDone(TaskDone),
    /// [`BodyShape::TaskReview`]
    TaskReview(TaskReview),
    /// [`BodyShape::TaskAssign`]
    TaskAssign(TaskAssign),
    /// [`BodyShape::ClaimWin`]
    ClaimWin(ClaimWin),
    /// [`BodyShape::ClaimCarry`]
    ClaimCarry(ClaimCarry),
    /// [`BodyShape::ClaimRenew`]
    ClaimRenew(ClaimRenew),
    /// [`BodyShape::DocApply`]
    DocApply(DocApply),
    /// [`BodyShape::SteerSend`]
    SteerSend(SteerSend),
    /// [`BodyShape::SteerTell`]
    SteerTell(SteerTell),
    /// [`BodyShape::SteerInbox`]
    SteerInbox(SteerInbox),
    /// [`BodyShape::SteerHistory`]
    SteerHistory(SteerHistory),
    /// [`BodyShape::DirectiveRef`]
    DirectiveRef(DirectiveRef),
    /// [`BodyShape::SteerDispose`]
    SteerDispose(SteerDispose),
    /// [`BodyShape::ConsentDecide`]
    ConsentDecide(ConsentDecide),
    /// [`BodyShape::ConsentRead`]
    ConsentRead(ConsentRead),
    /// [`BodyShape::ConsentPolicy`]
    ConsentPolicy(ConsentPolicy),
    /// [`BodyShape::TrustRef`]
    TrustRef(TrustRef),
    /// [`BodyShape::LaneRead`]
    LaneRead(LaneRead),
    /// [`BodyShape::LaneSeat`]
    LaneSeat(LaneSeat),
    /// [`BodyShape::LaneArrange`]
    LaneArrange(LaneArrange),
    /// [`BodyShape::LaneState`]
    LaneState(LaneState),
    /// [`BodyShape::LaneRenewal`]
    LaneRenewal(LaneRenewal),
    /// [`BodyShape::LaneRetune`]
    LaneRetune(LaneRetune),
    /// [`BodyShape::LaneEffortRetune`]
    LaneEffortRetune(LaneEffortRetune),
    /// [`BodyShape::TeamConfigure`]
    TeamConfigure(TeamConfigure),
    /// [`BodyShape::TeamArrange`]
    TeamArrange(TeamArrange),
    /// [`BodyShape::LaneRecall`]
    LaneRecall(LaneRecall),
    /// [`BodyShape::TasteRecord`]
    TasteRecord(TasteRecord),
    /// [`BodyShape::Recognize`]
    Recognize(Recognize),
    /// [`BodyShape::ReleasePublish`]
    ReleasePublish(ReleasePublish),
    /// [`BodyShape::UpChange`]
    UpChange(UpChange),
}

impl Intent {
    /// The shape this decoded body carries.
    #[must_use]
    pub const fn shape(&self) -> BodyShape {
        match self {
            Self::BoardRead(_)
            | Self::ReportRead(_)
            | Self::TaskDetail(_)
            | Self::TaskRelations(_)
            | Self::TaskTimeline(_)
            | Self::BoardGraph(_)
            | Self::BoardHandout(_)
            | Self::DocLedger(_)
            | Self::TaskAdd(_)
            | Self::TaskOopsAdd(_)
            | Self::TasteProposalAdd(_)
            | Self::TaskEdit(_)
            | Self::TaskDelete(_)
            | Self::TaskClose(_)
            | Self::TaskNote(_)
            | Self::TaskDefer(_)
            | Self::TaskWake(_)
            | Self::TaskRef(_)
            | Self::TaskDepends(_)
            | Self::TaskPhase(_)
            | Self::TaskDone(_)
            | Self::TaskReview(_)
            | Self::TaskAssign(_)
            | Self::ClaimWin(_)
            | Self::ClaimCarry(_)
            | Self::ClaimRenew(_)
            | Self::DocApply(_) => self.board_shape(),
            Self::SteerSend(_)
            | Self::SteerTell(_)
            | Self::SteerInbox(_)
            | Self::SteerHistory(_)
            | Self::DirectiveRef(_)
            | Self::SteerDispose(_)
            | Self::ConsentDecide(_)
            | Self::ConsentRead(_)
            | Self::ConsentPolicy(_)
            | Self::TrustRef(_)
            | Self::LaneRead(_)
            | Self::LaneSeat(_)
            | Self::LaneArrange(_)
            | Self::LaneState(_)
            | Self::LaneRenewal(_)
            | Self::LaneRetune(_)
            | Self::LaneEffortRetune(_)
            | Self::TeamConfigure(_)
            | Self::TeamArrange(_)
            | Self::LaneRecall(_)
            | Self::GlobalSettingsRead(_)
            | Self::GlobalSettingsSet(_)
            | Self::VendorCatalog(_)
            | Self::TasteRecord(_)
            | Self::Recognize(_)
            | Self::ReleasePublish(_)
            | Self::UpChange(_) => self.runtime_shape(),
        }
    }

    const fn board_shape(&self) -> BodyShape {
        match self {
            Self::BoardRead(_) => BodyShape::BoardRead,
            Self::ReportRead(_) => BodyShape::ReportRead,
            Self::TaskDetail(_) => BodyShape::TaskDetail,
            Self::TaskRelations(_) => BodyShape::TaskRelations,
            Self::TaskTimeline(_) => BodyShape::TaskTimeline,
            Self::BoardGraph(_) => BodyShape::BoardGraph,
            Self::BoardHandout(_) => BodyShape::BoardHandout,
            Self::DocLedger(_) => BodyShape::DocLedger,
            Self::TaskAdd(_) => BodyShape::TaskAdd,
            Self::TaskOopsAdd(_) => BodyShape::TaskOopsAdd,
            Self::TasteProposalAdd(_) => BodyShape::TasteProposalAdd,
            Self::TaskEdit(_) => BodyShape::TaskEdit,
            Self::TaskDelete(_) => BodyShape::TaskDelete,
            Self::TaskClose(_) => BodyShape::TaskClose,
            Self::TaskNote(_) => BodyShape::TaskNote,
            Self::TaskDefer(_) => BodyShape::TaskDefer,
            Self::TaskWake(_) => BodyShape::TaskWake,
            Self::TaskRef(_) => BodyShape::TaskRef,
            Self::TaskDepends(_) => BodyShape::TaskDepends,
            Self::TaskPhase(_) => BodyShape::TaskPhase,
            Self::TaskDone(_) => BodyShape::TaskDone,
            Self::TaskReview(_) => BodyShape::TaskReview,
            Self::TaskAssign(_) => BodyShape::TaskAssign,
            Self::ClaimWin(_) => BodyShape::ClaimWin,
            Self::ClaimCarry(_) => BodyShape::ClaimCarry,
            Self::ClaimRenew(_) => BodyShape::ClaimRenew,
            Self::DocApply(_) => BodyShape::DocApply,
            _ => panic!("intent shape partition is closed"),
        }
    }

    const fn runtime_shape(&self) -> BodyShape {
        match self {
            Self::SteerSend(_) => BodyShape::SteerSend,
            Self::SteerTell(_) => BodyShape::SteerTell,
            Self::SteerInbox(_) => BodyShape::SteerInbox,
            Self::SteerHistory(_) => BodyShape::SteerHistory,
            Self::DirectiveRef(_) => BodyShape::DirectiveRef,
            Self::SteerDispose(_) => BodyShape::SteerDispose,
            Self::ConsentDecide(_) => BodyShape::ConsentDecide,
            Self::ConsentRead(_) => BodyShape::ConsentRead,
            Self::ConsentPolicy(_) => BodyShape::ConsentPolicy,
            Self::TrustRef(_) => BodyShape::TrustRef,
            Self::LaneRead(_) => BodyShape::LaneRead,
            Self::LaneSeat(_) => BodyShape::LaneSeat,
            Self::LaneArrange(_) => BodyShape::LaneArrange,
            Self::LaneState(_) => BodyShape::LaneState,
            Self::LaneRenewal(_) => BodyShape::LaneRenewal,
            Self::LaneRetune(_) => BodyShape::LaneRetune,
            Self::LaneEffortRetune(_) => BodyShape::LaneEffortRetune,
            Self::TeamConfigure(_) => BodyShape::TeamConfigure,
            Self::TeamArrange(_) => BodyShape::TeamArrange,
            Self::LaneRecall(_) => BodyShape::LaneRecall,
            Self::GlobalSettingsRead(_) => BodyShape::GlobalSettingsRead,
            Self::GlobalSettingsSet(_) => BodyShape::GlobalSettingsSet,
            Self::VendorCatalog(_) => BodyShape::VendorCatalog,
            Self::TasteRecord(_) => BodyShape::TasteRecord,
            Self::Recognize(_) => BodyShape::Recognize,
            Self::ReleasePublish(_) => BodyShape::ReleasePublish,
            Self::UpChange(_) => BodyShape::UpChange,
            _ => panic!("intent shape partition is closed"),
        }
    }
}

impl BodyShape {
    /// Decode a command body into the intent this shape names.
    ///
    /// # Errors
    ///
    /// Returns the decoder error when the bytes are not that shape. Bodies
    /// refuse unknown fields, so a body spelled for the wrong kind is a
    /// decode failure rather than a silently truncated value.
    pub fn decode(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        if self.is_board_body() {
            self.decode_board_body(bytes)
        } else {
            self.decode_runtime_body(bytes)
        }
    }

    const fn is_board_body(self) -> bool {
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
                | Self::TaskAdd
                | Self::TaskOopsAdd
                | Self::TasteProposalAdd
                | Self::TaskEdit
                | Self::TaskDelete
                | Self::TaskClose
                | Self::TaskNote
                | Self::TaskDefer
                | Self::TaskWake
                | Self::TaskRef
                | Self::TaskDepends
                | Self::TaskPhase
                | Self::TaskDone
                | Self::TaskReview
                | Self::TaskAssign
                | Self::ClaimWin
                | Self::ClaimCarry
                | Self::ClaimRenew
                | Self::DocApply
        )
    }

    fn decode_board_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        if matches!(
            self,
            Self::BoardRead
                | Self::ReportRead
                | Self::TaskDetail
                | Self::TaskRelations
                | Self::TaskTimeline
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::TaskAdd
                | Self::TaskOopsAdd
                | Self::TasteProposalAdd
                | Self::TaskEdit
                | Self::TaskDelete
                | Self::TaskClose
                | Self::TaskNote
        ) {
            self.decode_board_query_body(bytes)
        } else {
            self.decode_board_transition_body(bytes)
        }
    }

    fn decode_board_query_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::BoardRead => Intent::BoardRead(crate::decode(bytes)?),
            Self::ReportRead => Intent::ReportRead(crate::decode(bytes)?),
            Self::TaskDetail => Intent::TaskDetail(crate::decode(bytes)?),
            Self::TaskRelations => Intent::TaskRelations(crate::decode(bytes)?),
            Self::TaskTimeline => Intent::TaskTimeline(crate::decode(bytes)?),
            Self::BoardGraph => Intent::BoardGraph(crate::decode(bytes)?),
            Self::BoardHandout => Intent::BoardHandout(crate::decode(bytes)?),
            Self::DocLedger => Intent::DocLedger(crate::decode(bytes)?),
            _ => return self.decode_task_statement_body(bytes),
        })
    }

    fn decode_task_statement_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::TaskAdd => Intent::TaskAdd(crate::decode(bytes)?),
            Self::TaskOopsAdd => Intent::TaskOopsAdd(crate::decode(bytes)?),
            Self::TasteProposalAdd => Intent::TasteProposalAdd(crate::decode(bytes)?),
            Self::TaskEdit => Intent::TaskEdit(crate::decode(bytes)?),
            Self::TaskDelete => Intent::TaskDelete(crate::decode(bytes)?),
            Self::TaskClose => Intent::TaskClose(crate::decode(bytes)?),
            Self::TaskNote => Intent::TaskNote(crate::decode(bytes)?),
            _ => unreachable!("board body partition is closed"),
        })
    }

    fn decode_board_transition_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::TaskDefer => Intent::TaskDefer(crate::decode(bytes)?),
            Self::TaskWake => Intent::TaskWake(crate::decode(bytes)?),
            Self::TaskRef => Intent::TaskRef(crate::decode(bytes)?),
            Self::TaskDepends => Intent::TaskDepends(crate::decode(bytes)?),
            Self::TaskPhase => Intent::TaskPhase(crate::decode(bytes)?),
            Self::TaskDone => Intent::TaskDone(crate::decode(bytes)?),
            Self::TaskReview => Intent::TaskReview(crate::decode(bytes)?),
            Self::TaskAssign => Intent::TaskAssign(crate::decode(bytes)?),
            Self::ClaimWin => Intent::ClaimWin(crate::decode(bytes)?),
            Self::ClaimCarry => Intent::ClaimCarry(crate::decode(bytes)?),
            Self::ClaimRenew => Intent::ClaimRenew(crate::decode(bytes)?),
            Self::DocApply => Intent::DocApply(crate::decode(bytes)?),
            _ => unreachable!("board body partition is closed"),
        })
    }

    fn decode_runtime_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        if matches!(
            self,
            Self::SteerSend
                | Self::SteerTell
                | Self::SteerInbox
                | Self::SteerHistory
                | Self::DirectiveRef
                | Self::SteerDispose
                | Self::ConsentDecide
                | Self::ConsentRead
                | Self::ConsentPolicy
                | Self::GlobalSettingsRead
                | Self::GlobalSettingsSet
                | Self::VendorCatalog
                | Self::TrustRef
        ) {
            self.decode_coordination_body(bytes)
        } else {
            self.decode_runtime_control_body(bytes)
        }
    }

    fn decode_coordination_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::SteerSend => Intent::SteerSend(crate::decode(bytes)?),
            Self::SteerTell => Intent::SteerTell(crate::decode(bytes)?),
            Self::SteerInbox => Intent::SteerInbox(crate::decode(bytes)?),
            Self::SteerHistory => Intent::SteerHistory(crate::decode(bytes)?),
            Self::DirectiveRef => Intent::DirectiveRef(crate::decode(bytes)?),
            Self::SteerDispose => Intent::SteerDispose(crate::decode(bytes)?),
            Self::ConsentDecide => Intent::ConsentDecide(crate::decode(bytes)?),
            Self::ConsentRead => Intent::ConsentRead(crate::decode(bytes)?),
            Self::ConsentPolicy => Intent::ConsentPolicy(crate::decode(bytes)?),
            Self::GlobalSettingsRead => Intent::GlobalSettingsRead(crate::decode(bytes)?),
            Self::GlobalSettingsSet => Intent::GlobalSettingsSet(crate::decode(bytes)?),
            Self::VendorCatalog => Intent::VendorCatalog(crate::decode(bytes)?),
            Self::TrustRef => Intent::TrustRef(crate::decode(bytes)?),
            _ => unreachable!("runtime body partition is closed"),
        })
    }

    fn decode_runtime_control_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::LaneRead => Intent::LaneRead(crate::decode(bytes)?),
            Self::LaneSeat => Intent::LaneSeat(crate::decode(bytes)?),
            Self::LaneArrange => Intent::LaneArrange(crate::decode(bytes)?),
            Self::LaneState => Intent::LaneState(crate::decode(bytes)?),
            Self::LaneRenewal => Intent::LaneRenewal(crate::decode(bytes)?),
            Self::LaneRetune => Intent::LaneRetune(crate::decode(bytes)?),
            Self::LaneEffortRetune => Intent::LaneEffortRetune(crate::decode(bytes)?),
            Self::TeamConfigure => Intent::TeamConfigure(crate::decode(bytes)?),
            Self::TeamArrange => Intent::TeamArrange(crate::decode(bytes)?),
            Self::LaneRecall => Intent::LaneRecall(crate::decode(bytes)?),
            Self::TasteRecord => Intent::TasteRecord(crate::decode(bytes)?),
            Self::Recognize => Intent::Recognize(crate::decode(bytes)?),
            Self::ReleasePublish => Intent::ReleasePublish(crate::decode(bytes)?),
            Self::UpChange => Intent::UpChange(crate::decode(bytes)?),
            _ => unreachable!("runtime body partition is closed"),
        })
    }
}
