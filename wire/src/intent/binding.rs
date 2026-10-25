//! Closed command body and answer bindings.

mod answer;
mod principal;
mod rooms;

use serde::Serialize;

use crate::QuoteResolution;
use crate::command::CommandKind;
use crate::principal::{
    PrincipalAdmit, PrincipalDeauthorize, PrincipalDirectoryAnswer, PrincipalDirectoryRead,
    PrincipalEdit, PrincipalRead, PrincipalRoleRef, PrincipalStanding, PrincipalsAnswer,
};
use crate::rooms::{
    CandidateCoverageRecord, CandidateCritiqueRecord, CandidateWithdraw, DesignNudge,
    DesignPropose, OutputAdopt, PreparationDecide, RoomPost, RoomPostAnswer,
};

use answer::AnswerSource;

use super::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, ClaimAnswer, ClaimCarry, ClaimRenew,
    ClaimWin, ConsentAnswer, ConsentDecide, ConsentPolicy, ConsentPolicyAnswer, ConsentQueueAnswer,
    ConsentRead, DirectionHistoryAnswer, DirectiveAnswer, DirectiveRef, DocAnswer, DocApply,
    DocLedger, GlobalSettingsAnswer, GlobalSettingsRead, GlobalSettingsSet, InboxAnswer,
    LaneAnswer, LaneArrange, LaneChangeAnswer, LaneEffortRetune, LaneFastRetune, LaneJobAnswer,
    LaneJobRef, LaneJobsAnswer, LaneJobsRead, LaneRead, LaneRecall, LaneRenewal, LaneRenewalAnswer,
    LaneRetune, LaneSeat, LaneState, QuoteRead, RecallAnswer, RecognitionAnswer, Recognize,
    ReleasePublish, ReportAnswer, ReportRead, RoomContribute, RoomContributeAnswer,
    RoomContributionRead, RoomContributionReadAnswer, RoomControl, RoomControlAnswer,
    RoomConversationText, RoomConversationTextAnswer, RoomLeave, RoomLeaveAnswer, StateAnswer,
    SteerDispose, SteerHistory, SteerInbox, SteerSend, TaskAdd, TaskAnswer, TaskAssign, TaskClose,
    TaskConclude, TaskDefer, TaskDelete, TaskDepends, TaskDetail, TaskDetailAnswer, TaskDone,
    TaskEdit, TaskFile, TaskNote, TaskOopsAdd, TaskPhase, TaskRef, TaskRelations,
    TaskRelationsAnswer, TaskReview, TaskTimeline, TaskTimelineAnswer, TaskWake, TasteAnswer,
    TasteRecord, TeamArrange, TeamConfigure, TextAnswer, TrustAnswer, TrustRef, UpChange,
    VendorCatalog, VendorCatalogAnswer, WorkingGroupArrange, WorkingGroupChange,
    WorkingGroupDisband, WorkingGroupFast, WorkingGroupMove, WorkingGroupPage, WorkingGroupRead,
    WorkingGroupStanceSet,
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
    /// [`RoomPost`]
    RoomPost,
    /// [`RoomConversationText`]
    RoomConversationText,
    /// [`RoomLeave`]
    RoomLeave,
    /// [`RoomContribute`]
    RoomContribute,
    /// [`RoomContributionRead`]
    RoomContributionRead,
    /// [`RoomControl`]
    RoomControl,
    /// [`WorkingGroupRead`]
    WorkingGroupRead,
    /// [`WorkingGroupMove`]
    WorkingGroupMove,
    /// [`WorkingGroupArrange`]
    WorkingGroupArrange,
    /// [`WorkingGroupDisband`]
    WorkingGroupDisband,
    /// [`WorkingGroupFast`]
    WorkingGroupFast,
    /// [`WorkingGroupStanceSet`]
    WorkingGroupStance,
    /// [`DesignPropose`]
    DesignPropose,
    /// [`CandidateCritiqueRecord`]
    CandidateCritiqueRecord,
    /// [`CandidateCoverageRecord`]
    CandidateCoverageRecord,
    /// [`PreparationDecide`]
    PreparationDecide,
    /// [`CandidateWithdraw`]
    CandidateWithdraw,
    /// [`OutputAdopt`]
    OutputAdopt,
    /// [`DesignNudge`]
    DesignNudge,
    /// [`GlobalSettingsRead`]
    GlobalSettingsRead,
    /// [`GlobalSettingsSet`]
    GlobalSettingsSet,
    /// [`VendorCatalog`]
    VendorCatalog,
    /// [`TaskAdd`]
    TaskAdd,
    /// [`TaskOopsAdd`], the strict body for OOPS Design line-item intake.
    TaskOopsAdd,
    /// [`TaskEdit`]
    TaskEdit,
    /// [`TaskDelete`]
    TaskDelete,
    /// [`TaskClose`]
    TaskClose,
    /// [`TaskConclude`]
    TaskConclude,
    /// [`TaskNote`]
    TaskNote,
    /// [`TaskDefer`]
    TaskDefer,
    /// [`TaskFile`]
    TaskFile,
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
    /// [`SteerInbox`]
    SteerInbox,
    /// [`SteerHistory`]
    SteerHistory,
    /// [`DirectiveRef`]
    DirectiveRef,
    /// [`SteerDispose`]
    SteerDispose,
    /// [`QuoteRead`]
    QuoteRead,
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
    /// [`LaneJobsRead`]
    LaneJobs,
    /// [`LaneJobRef`]
    LaneJob,
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
    /// [`LaneFastRetune`]
    LaneFastRetune,
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
    /// [`PrincipalDirectoryRead`]
    PrincipalDirectoryRead,
    /// [`PrincipalRead`]
    PrincipalRead,
    /// [`PrincipalAdmit`]
    PrincipalAdmit,
    /// [`PrincipalEdit`]
    PrincipalEdit,
    /// [`PrincipalRoleRef`]
    PrincipalRoleRef,
    /// [`PrincipalDeauthorize`]
    PrincipalDeauthorize,
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
    /// [`WorkingGroupPage`]
    WorkingGroupPage,
    /// [`WorkingGroupChange`]
    WorkingGroupChange,
    /// [`TextAnswer`]
    Text,
    /// [`RoomPostAnswer`]
    RoomPost,
    /// [`RoomLeaveAnswer`]
    RoomLeave,
    /// [`RoomContributeAnswer`]
    RoomContribute,
    /// [`RoomContributionReadAnswer`]
    RoomContributionRead,
    /// [`RoomControlAnswer`]
    RoomControl,
    /// [`RoomConversationTextAnswer`]
    RoomConversationText,
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
    /// [`LaneJobsAnswer`]
    LaneJobs,
    /// [`LaneJobAnswer`]
    LaneJob,
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
    /// [`QuoteResolution`]
    Quote,
    /// [`PrincipalDirectoryAnswer`]
    PrincipalDirectory,
    /// [`PrincipalsAnswer`]
    Principals,
    /// [`PrincipalStanding`]
    Principal,
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
    /// [`AnswerShape::WorkingGroupPage`]
    WorkingGroupPage(WorkingGroupPage),
    /// [`AnswerShape::WorkingGroupChange`]
    WorkingGroupChange(WorkingGroupChange),
    /// [`AnswerShape::Text`]
    Text(TextAnswer),
    /// [`AnswerShape::RoomPost`]
    RoomPost(RoomPostAnswer),
    /// [`AnswerShape::RoomLeave`]
    RoomLeave(RoomLeaveAnswer),
    /// [`AnswerShape::RoomContribute`]
    RoomContribute(Box<RoomContributeAnswer>),
    /// [`AnswerShape::RoomContributionRead`]
    RoomContributionRead(Box<RoomContributionReadAnswer>),
    /// [`AnswerShape::RoomControl`]
    RoomControl(Box<RoomControlAnswer>),
    /// [`AnswerShape::RoomConversationText`]
    RoomConversationText(Box<RoomConversationTextAnswer>),
    /// [`AnswerShape::Task`]
    Task(Box<TaskAnswer>),
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
    /// [`AnswerShape::LaneJobs`]
    LaneJobs(Box<LaneJobsAnswer>),
    /// [`AnswerShape::LaneJob`]
    LaneJob(Box<LaneJobAnswer>),
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
    /// [`AnswerShape::Quote`]
    Quote(Box<QuoteResolution>),
    /// [`AnswerShape::PrincipalDirectory`]
    PrincipalDirectory(PrincipalDirectoryAnswer),
    /// [`AnswerShape::Principals`]
    Principals(PrincipalsAnswer),
    /// [`AnswerShape::Principal`]
    Principal(Box<PrincipalStanding>),
}

impl CommandAnswer {
    /// The shape this decoded answer carries.
    #[must_use]
    pub const fn shape(&self) -> AnswerShape {
        match self {
            Self::WorkingGroupPage(_) | Self::WorkingGroupChange(_) => self.working_group_shape(),
            Self::Board(_)
            | Self::Report(_)
            | Self::TaskDetail(_)
            | Self::TaskRelations(_)
            | Self::TaskTimeline(_)
            | Self::Text(_)
            | Self::RoomPost(_)
            | Self::RoomLeave(_)
            | Self::RoomContribute(_)
            | Self::RoomContributionRead(_)
            | Self::RoomControl(_)
            | Self::RoomConversationText(_)
            | Self::Task(_)
            | Self::Claim(_)
            | Self::Doc(_) => self.board_answer_shape(),
            Self::Directive(_) => AnswerShape::Directive,
            Self::Inbox(_) => AnswerShape::Inbox,
            Self::DirectionHistory(_) => AnswerShape::DirectionHistory,
            Self::Consent(_) => AnswerShape::Consent,
            Self::ConsentQueue(_) => AnswerShape::ConsentQueue,
            Self::ConsentPolicy(_) => AnswerShape::ConsentPolicy,
            Self::GlobalSettings(_) => AnswerShape::GlobalSettings,
            Self::VendorCatalog(_) => AnswerShape::VendorCatalog,
            Self::Trust(_) => AnswerShape::Trust,
            Self::Lane(_)
            | Self::LaneJobs(_)
            | Self::LaneJob(_)
            | Self::LaneChange(_)
            | Self::LaneRenewal(_) => self.lane_shape(),
            Self::State(_) => AnswerShape::State,
            Self::Recall(_) => AnswerShape::Recall,
            Self::Taste(_) => AnswerShape::Taste,
            Self::Recognition(_) => AnswerShape::Recognition,
            Self::Quote(_) => AnswerShape::Quote,
            Self::PrincipalDirectory(_) => AnswerShape::PrincipalDirectory,
            Self::Principals(_) => AnswerShape::Principals,
            Self::Principal(_) => AnswerShape::Principal,
        }
    }
    const fn board_answer_shape(&self) -> AnswerShape {
        match self {
            Self::Board(_) => AnswerShape::Board,
            Self::Report(_) => AnswerShape::Report,
            Self::TaskDetail(_) => AnswerShape::TaskDetail,
            Self::TaskRelations(_) => AnswerShape::TaskRelations,
            Self::TaskTimeline(_) => AnswerShape::TaskTimeline,
            Self::Text(_) => AnswerShape::Text,
            Self::RoomPost(_) => AnswerShape::RoomPost,
            Self::RoomLeave(_) => AnswerShape::RoomLeave,
            Self::RoomContribute(_) => AnswerShape::RoomContribute,
            Self::RoomContributionRead(_) => AnswerShape::RoomContributionRead,
            Self::RoomControl(_) => AnswerShape::RoomControl,
            Self::RoomConversationText(_) => AnswerShape::RoomConversationText,
            Self::Task(_) => AnswerShape::Task,
            Self::Claim(_) => AnswerShape::Claim,
            Self::Doc(_) => AnswerShape::Doc,
            _ => panic!("board answer shape partition is closed"),
        }
    }

    const fn working_group_shape(&self) -> AnswerShape {
        match self {
            Self::WorkingGroupPage(_) => AnswerShape::WorkingGroupPage,
            Self::WorkingGroupChange(_) => AnswerShape::WorkingGroupChange,
            _ => panic!("working-group answer shape partition is closed"),
        }
    }
    const fn lane_shape(&self) -> AnswerShape {
        match self {
            Self::Lane(_) => AnswerShape::Lane,
            Self::LaneJobs(_) => AnswerShape::LaneJobs,
            Self::LaneJob(_) => AnswerShape::LaneJob,
            Self::LaneChange(_) => AnswerShape::LaneChange,
            Self::LaneRenewal(_) => AnswerShape::LaneRenewal,
            _ => panic!("lane answer shape partition is closed"),
        }
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
            Self::LaneJobsRead => Binding::of(BodyShape::LaneJobs, AnswerShape::LaneJobs),
            Self::LaneJobRead | Self::LaneJobStop => {
                Binding::of(BodyShape::LaneJob, AnswerShape::LaneJob)
            }
            Self::WorkingGroupRead
            | Self::WorkingGroupMove
            | Self::WorkingGroupArrange
            | Self::WorkingGroupDisband
            | Self::WorkingGroupFast
            | Self::WorkingGroupStance => self.working_group_binding(),
            Self::PrincipalDirectoryRead
            | Self::PrincipalRead
            | Self::PrincipalAdmit
            | Self::PrincipalEdit
            | Self::PrincipalGrant
            | Self::PrincipalRevoke
            | Self::PrincipalDeauthorize => self.principal_binding(),
            Self::RoomPost
            | Self::RoomConversationText
            | Self::RoomLeave
            | Self::RoomContribute
            | Self::RoomContributionRead
            | Self::RoomControl => self.room_binding(),
            Self::DesignPropose
            | Self::CandidateCritiqueRecord
            | Self::CandidateCoverageRecord
            | Self::PreparationDecide
            | Self::CandidateWithdraw
            | Self::OutputAdopt
            | Self::DesignNudge
            | Self::BoardRead
            | Self::ReportRead
            | Self::TaskDetail
            | Self::TaskRelations
            | Self::TaskTimeline
            | Self::BoardGraph
            | Self::BoardHandout
            | Self::DocLedger
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
            | Self::TaskAssign
            | Self::ClaimWin
            | Self::ClaimCarry
            | Self::ClaimRenew
            | Self::ClaimFree
            | Self::ClaimLapse
            | Self::DocApply => self.board_binding(),
            Self::SteerSend
            | Self::SteerInbox
            | Self::SteerHistory
            | Self::SteerResend
            | Self::SteerRecover
            | Self::SteerDispose
            | Self::SteerCancel
            | Self::QuoteRead
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
            Self::DesignPropose
            | Self::CandidateCritiqueRecord
            | Self::CandidateCoverageRecord
            | Self::PreparationDecide
            | Self::CandidateWithdraw
            | Self::OutputAdopt
            | Self::DesignNudge => self.preparation_binding(),
            Self::TaskAdd => Binding::of(B::TaskAdd, A::Task),
            Self::TaskOopsAdd => Binding::of(B::TaskOopsAdd, A::Task),
            Self::TaskEdit => Binding::of(B::TaskEdit, A::Task),
            Self::TaskDelete => Binding::of(B::TaskDelete, A::Task),
            Self::TaskClose => Binding::of(B::TaskClose, A::Task),
            Self::TaskConclude => Binding::of(B::TaskConclude, A::Task),
            Self::TaskNote => Binding::of(B::TaskNote, A::Task),
            Self::TaskDefer => Binding::of(B::TaskDefer, A::Task),
            Self::TaskWake => Binding::of(B::TaskWake, A::Task),
            Self::TaskFile => Binding::of(B::TaskFile, A::Task),
            Self::TaskDepends => Binding::of(B::TaskDepends, A::Task),
            Self::TaskPhase => Binding::of(B::TaskPhase, A::Task),
            Self::TaskDone => Binding::of(B::TaskDone, A::Task),
            Self::TaskReview => Binding::of(B::TaskReview, A::Task),
            Self::TaskAssign => Binding::of(B::TaskAssign, A::Task),
            Self::ClaimWin
            | Self::ClaimCarry
            | Self::ClaimRenew
            | Self::ClaimFree
            | Self::ClaimLapse => self.claim_binding(),
            Self::DocApply => Binding::of(B::DocApply, A::Doc),
            _ => panic!("command binding partition is closed"),
        }
    }
    const fn preparation_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::DesignPropose => Binding::of(B::DesignPropose, A::Text),
            Self::CandidateCritiqueRecord => Binding::of(B::CandidateCritiqueRecord, A::Text),
            Self::CandidateCoverageRecord => Binding::of(B::CandidateCoverageRecord, A::Text),
            Self::PreparationDecide => Binding::of(B::PreparationDecide, A::Text),
            Self::CandidateWithdraw => Binding::of(B::CandidateWithdraw, A::Text),
            Self::OutputAdopt => Binding::of(B::OutputAdopt, A::Text),
            Self::DesignNudge => Binding::of(B::DesignNudge, A::Text),
            _ => panic!("preparation command binding partition is closed"),
        }
    }

    const fn claim_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::ClaimWin => Binding::of(B::ClaimWin, A::Claim),
            Self::ClaimCarry => Binding::of(B::ClaimCarry, A::Claim),
            Self::ClaimRenew => Binding::of(B::ClaimRenew, A::Claim),
            Self::ClaimFree | Self::ClaimLapse => Binding::of(B::TaskRef, A::Claim),
            _ => panic!("claim command binding partition is closed"),
        }
    }

    const fn working_group_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::WorkingGroupRead => Binding::of(B::WorkingGroupRead, A::WorkingGroupPage),
            Self::WorkingGroupMove => Binding::of(B::WorkingGroupMove, A::WorkingGroupChange),
            Self::WorkingGroupArrange => Binding::of(B::WorkingGroupArrange, A::WorkingGroupChange),
            Self::WorkingGroupDisband => Binding::of(B::WorkingGroupDisband, A::WorkingGroupChange),
            Self::WorkingGroupFast => Binding::of(B::WorkingGroupFast, A::WorkingGroupChange),
            Self::WorkingGroupStance => Binding::of(B::WorkingGroupStance, A::WorkingGroupChange),
            _ => panic!("working-group command binding partition is closed"),
        }
    }

    const fn runtime_binding(self) -> Binding {
        use AnswerShape as A;
        use BodyShape as B;

        match self {
            Self::SteerSend => Binding::of(B::SteerSend, A::Directive),
            Self::SteerInbox => Binding::of(B::SteerInbox, A::Inbox),
            Self::SteerHistory => Binding::of(B::SteerHistory, A::DirectionHistory),
            Self::SteerResend | Self::SteerRecover | Self::SteerCancel => {
                Binding::of(B::DirectiveRef, A::Directive)
            }
            Self::SteerDispose => Binding::of(B::SteerDispose, A::Directive),
            Self::QuoteRead => Binding::of(B::QuoteRead, A::Quote),
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
            Self::LaneFastRetune => Binding::of(B::LaneFastRetune, A::Lane),
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
    /// [`BodyShape::RoomPost`]
    RoomPost(RoomPost),
    /// [`BodyShape::RoomConversationText`]
    RoomConversationText(RoomConversationText),
    /// [`BodyShape::RoomLeave`]
    RoomLeave(RoomLeave),
    /// [`BodyShape::RoomContribute`]
    RoomContribute(RoomContribute),
    /// [`BodyShape::RoomContributionRead`]
    RoomContributionRead(RoomContributionRead),
    /// [`BodyShape::RoomControl`]
    RoomControl(RoomControl),
    /// [`BodyShape::WorkingGroupRead`]
    WorkingGroupRead(WorkingGroupRead),
    /// [`BodyShape::WorkingGroupMove`]
    WorkingGroupMove(WorkingGroupMove),
    /// [`BodyShape::WorkingGroupArrange`]
    WorkingGroupArrange(WorkingGroupArrange),
    /// [`BodyShape::WorkingGroupDisband`]
    WorkingGroupDisband(WorkingGroupDisband),
    /// [`BodyShape::WorkingGroupFast`]
    WorkingGroupFast(WorkingGroupFast),
    /// [`BodyShape::WorkingGroupStance`]
    WorkingGroupStance(WorkingGroupStanceSet),
    /// [`BodyShape::DesignPropose`]
    DesignPropose(DesignPropose),
    /// [`BodyShape::CandidateCritiqueRecord`]
    CandidateCritiqueRecord(CandidateCritiqueRecord),
    /// [`BodyShape::CandidateCoverageRecord`]
    CandidateCoverageRecord(CandidateCoverageRecord),
    /// [`BodyShape::PreparationDecide`]
    PreparationDecide(PreparationDecide),
    /// [`BodyShape::CandidateWithdraw`]
    CandidateWithdraw(CandidateWithdraw),
    /// [`BodyShape::OutputAdopt`]
    OutputAdopt(OutputAdopt),
    /// [`BodyShape::DesignNudge`]
    DesignNudge(DesignNudge),
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
    /// [`BodyShape::TaskEdit`]
    TaskEdit(TaskEdit),
    /// [`BodyShape::TaskDelete`]
    TaskDelete(TaskDelete),
    /// [`BodyShape::TaskClose`]
    TaskClose(TaskClose),
    /// [`BodyShape::TaskConclude`]
    TaskConclude(TaskConclude),
    /// [`BodyShape::TaskNote`]
    TaskNote(TaskNote),
    /// [`BodyShape::TaskDefer`]
    TaskDefer(TaskDefer),
    /// [`BodyShape::TaskWake`]
    TaskWake(TaskWake),
    /// [`BodyShape::TaskFile`]
    TaskFile(TaskFile),
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
    /// [`BodyShape::SteerInbox`]
    SteerInbox(SteerInbox),
    /// [`BodyShape::SteerHistory`]
    SteerHistory(SteerHistory),
    /// [`BodyShape::DirectiveRef`]
    DirectiveRef(DirectiveRef),
    /// [`BodyShape::SteerDispose`]
    SteerDispose(SteerDispose),
    /// [`BodyShape::QuoteRead`]
    QuoteRead(QuoteRead),
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
    /// [`BodyShape::LaneJobs`]
    LaneJobs(LaneJobsRead),
    /// [`BodyShape::LaneJob`]
    LaneJob(LaneJobRef),
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
    /// [`BodyShape::LaneFastRetune`]
    LaneFastRetune(LaneFastRetune),
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
    /// [`BodyShape::PrincipalDirectoryRead`]
    PrincipalDirectoryRead(PrincipalDirectoryRead),
    /// [`BodyShape::PrincipalRead`]
    PrincipalRead(PrincipalRead),
    /// [`BodyShape::PrincipalAdmit`]
    PrincipalAdmit(PrincipalAdmit),
    /// [`BodyShape::PrincipalEdit`]
    PrincipalEdit(PrincipalEdit),
    /// [`BodyShape::PrincipalRoleRef`]
    PrincipalRoleRef(PrincipalRoleRef),
    /// [`BodyShape::PrincipalDeauthorize`]
    PrincipalDeauthorize(PrincipalDeauthorize),
}

impl Intent {
    /// The shape this decoded body carries.
    #[must_use]
    pub const fn shape(&self) -> BodyShape {
        match self {
            Self::LaneJobs(_) => BodyShape::LaneJobs,
            Self::LaneJob(_) => BodyShape::LaneJob,
            Self::WorkingGroupRead(_)
            | Self::WorkingGroupMove(_)
            | Self::WorkingGroupArrange(_)
            | Self::WorkingGroupDisband(_)
            | Self::WorkingGroupFast(_)
            | Self::WorkingGroupStance(_) => self.working_group_shape(),
            Self::PrincipalDirectoryRead(_)
            | Self::PrincipalRead(_)
            | Self::PrincipalAdmit(_)
            | Self::PrincipalEdit(_)
            | Self::PrincipalRoleRef(_)
            | Self::PrincipalDeauthorize(_) => self.principal_shape(),
            Self::RoomPost(_)
            | Self::RoomConversationText(_)
            | Self::RoomLeave(_)
            | Self::RoomContribute(_)
            | Self::RoomContributionRead(_)
            | Self::RoomControl(_) => self.room_shape(),
            Self::DesignPropose(_)
            | Self::CandidateCritiqueRecord(_)
            | Self::CandidateCoverageRecord(_)
            | Self::PreparationDecide(_)
            | Self::CandidateWithdraw(_)
            | Self::OutputAdopt(_)
            | Self::DesignNudge(_)
            | Self::BoardRead(_)
            | Self::ReportRead(_)
            | Self::TaskDetail(_)
            | Self::TaskRelations(_)
            | Self::TaskTimeline(_)
            | Self::BoardGraph(_)
            | Self::BoardHandout(_)
            | Self::DocLedger(_)
            | Self::TaskAdd(_)
            | Self::TaskOopsAdd(_)
            | Self::TaskEdit(_)
            | Self::TaskDelete(_)
            | Self::TaskClose(_)
            | Self::TaskConclude(_)
            | Self::TaskNote(_)
            | Self::TaskDefer(_)
            | Self::TaskWake(_)
            | Self::TaskFile(_)
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
            | Self::SteerInbox(_)
            | Self::SteerHistory(_)
            | Self::DirectiveRef(_)
            | Self::SteerDispose(_)
            | Self::QuoteRead(_)
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
            | Self::LaneFastRetune(_)
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
            Self::DesignPropose(_)
            | Self::CandidateCritiqueRecord(_)
            | Self::CandidateCoverageRecord(_)
            | Self::PreparationDecide(_)
            | Self::CandidateWithdraw(_)
            | Self::OutputAdopt(_)
            | Self::DesignNudge(_) => self.preparation_shape(),
            Self::TaskAdd(_) => BodyShape::TaskAdd,
            Self::TaskOopsAdd(_) => BodyShape::TaskOopsAdd,
            Self::TaskEdit(_) => BodyShape::TaskEdit,
            Self::TaskDelete(_) => BodyShape::TaskDelete,
            Self::TaskClose(_) => BodyShape::TaskClose,
            Self::TaskConclude(_) => BodyShape::TaskConclude,
            Self::TaskNote(_) => BodyShape::TaskNote,
            Self::TaskDefer(_) => BodyShape::TaskDefer,
            Self::TaskWake(_) => BodyShape::TaskWake,
            Self::TaskFile(_) => BodyShape::TaskFile,
            Self::TaskRef(_) => BodyShape::TaskRef,
            Self::TaskDepends(_) => BodyShape::TaskDepends,
            Self::TaskPhase(_) => BodyShape::TaskPhase,
            Self::TaskDone(_) => BodyShape::TaskDone,
            Self::TaskReview(_) => BodyShape::TaskReview,
            Self::TaskAssign(_) => BodyShape::TaskAssign,
            Self::ClaimWin(_) | Self::ClaimCarry(_) | Self::ClaimRenew(_) => self.claim_shape(),
            Self::DocApply(_) => BodyShape::DocApply,
            _ => panic!("intent shape partition is closed"),
        }
    }
    const fn preparation_shape(&self) -> BodyShape {
        match self {
            Self::DesignPropose(_) => BodyShape::DesignPropose,
            Self::CandidateCritiqueRecord(_) => BodyShape::CandidateCritiqueRecord,
            Self::CandidateCoverageRecord(_) => BodyShape::CandidateCoverageRecord,
            Self::PreparationDecide(_) => BodyShape::PreparationDecide,
            Self::CandidateWithdraw(_) => BodyShape::CandidateWithdraw,
            Self::OutputAdopt(_) => BodyShape::OutputAdopt,
            Self::DesignNudge(_) => BodyShape::DesignNudge,
            _ => panic!("preparation intent shape partition is closed"),
        }
    }

    const fn claim_shape(&self) -> BodyShape {
        match self {
            Self::ClaimWin(_) => BodyShape::ClaimWin,
            Self::ClaimCarry(_) => BodyShape::ClaimCarry,
            Self::ClaimRenew(_) => BodyShape::ClaimRenew,
            _ => panic!("claim intent shape partition is closed"),
        }
    }

    const fn working_group_shape(&self) -> BodyShape {
        match self {
            Self::WorkingGroupRead(_) => BodyShape::WorkingGroupRead,
            Self::WorkingGroupMove(_) => BodyShape::WorkingGroupMove,
            Self::WorkingGroupArrange(_) => BodyShape::WorkingGroupArrange,
            Self::WorkingGroupDisband(_) => BodyShape::WorkingGroupDisband,
            Self::WorkingGroupFast(_) => BodyShape::WorkingGroupFast,
            Self::WorkingGroupStance(_) => BodyShape::WorkingGroupStance,
            _ => panic!("working-group intent shape partition is closed"),
        }
    }

    const fn runtime_shape(&self) -> BodyShape {
        match self {
            Self::SteerSend(_) => BodyShape::SteerSend,
            Self::SteerInbox(_) => BodyShape::SteerInbox,
            Self::SteerHistory(_) => BodyShape::SteerHistory,
            Self::DirectiveRef(_) => BodyShape::DirectiveRef,
            Self::SteerDispose(_) => BodyShape::SteerDispose,
            Self::QuoteRead(_) => BodyShape::QuoteRead,
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
            Self::LaneFastRetune(_) => BodyShape::LaneFastRetune,
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
                | Self::WorkingGroupStance
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
                | Self::WorkingGroupStance
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
            Self::RoomPost
            | Self::RoomConversationText
            | Self::RoomLeave
            | Self::RoomContribute
            | Self::RoomContributionRead
            | Self::RoomControl => return self.decode_room_body(bytes),
            Self::WorkingGroupRead
            | Self::WorkingGroupMove
            | Self::WorkingGroupArrange
            | Self::WorkingGroupDisband
            | Self::WorkingGroupFast
            | Self::WorkingGroupStance => return self.decode_working_group_body(bytes),
            Self::DesignPropose
            | Self::CandidateCritiqueRecord
            | Self::CandidateCoverageRecord
            | Self::PreparationDecide
            | Self::CandidateWithdraw
            | Self::OutputAdopt
            | Self::DesignNudge => return self.decode_preparation_body(bytes),
            _ => return self.decode_task_statement_body(bytes),
        })
    }

    fn decode_working_group_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::WorkingGroupRead => Intent::WorkingGroupRead(crate::decode(bytes)?),
            Self::WorkingGroupMove => Intent::WorkingGroupMove(crate::decode(bytes)?),
            Self::WorkingGroupArrange => Intent::WorkingGroupArrange(crate::decode(bytes)?),
            Self::WorkingGroupDisband => Intent::WorkingGroupDisband(crate::decode(bytes)?),
            Self::WorkingGroupFast => Intent::WorkingGroupFast(crate::decode(bytes)?),
            Self::WorkingGroupStance => Intent::WorkingGroupStance(crate::decode(bytes)?),
            _ => panic!("working-group body decoding partition is closed"),
        })
    }

    fn decode_preparation_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::DesignPropose => Intent::DesignPropose(crate::decode(bytes)?),
            Self::CandidateCritiqueRecord => Intent::CandidateCritiqueRecord(crate::decode(bytes)?),
            Self::CandidateCoverageRecord => Intent::CandidateCoverageRecord(crate::decode(bytes)?),
            Self::PreparationDecide => Intent::PreparationDecide(crate::decode(bytes)?),
            Self::CandidateWithdraw => Intent::CandidateWithdraw(crate::decode(bytes)?),
            Self::OutputAdopt => Intent::OutputAdopt(crate::decode(bytes)?),
            Self::DesignNudge => Intent::DesignNudge(crate::decode(bytes)?),
            _ => unreachable!("preparation body partition is closed"),
        })
    }

    fn decode_task_statement_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::TaskAdd => Intent::TaskAdd(crate::decode(bytes)?),
            Self::TaskOopsAdd => Intent::TaskOopsAdd(crate::decode(bytes)?),
            Self::TaskEdit => Intent::TaskEdit(crate::decode(bytes)?),
            Self::TaskDelete => Intent::TaskDelete(crate::decode(bytes)?),
            Self::TaskClose => Intent::TaskClose(crate::decode(bytes)?),
            Self::TaskConclude => Intent::TaskConclude(crate::decode(bytes)?),
            Self::TaskNote => Intent::TaskNote(crate::decode(bytes)?),
            _ => unreachable!("board body partition is closed"),
        })
    }

    fn decode_board_transition_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::TaskDefer => Intent::TaskDefer(crate::decode(bytes)?),
            Self::TaskWake => Intent::TaskWake(crate::decode(bytes)?),
            Self::TaskFile => Intent::TaskFile(crate::decode(bytes)?),
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
                | Self::SteerInbox
                | Self::SteerHistory
                | Self::DirectiveRef
                | Self::SteerDispose
                | Self::QuoteRead
                | Self::ConsentDecide
                | Self::ConsentRead
                | Self::ConsentPolicy
                | Self::GlobalSettingsRead
                | Self::GlobalSettingsSet
                | Self::VendorCatalog
                | Self::TrustRef
        ) {
            self.decode_coordination_body(bytes)
        } else if matches!(
            self,
            Self::LaneRead
                | Self::LaneJobs
                | Self::LaneJob
                | Self::LaneSeat
                | Self::LaneArrange
                | Self::LaneState
                | Self::LaneRenewal
                | Self::LaneRetune
                | Self::LaneEffortRetune
                | Self::LaneFastRetune
                | Self::TeamConfigure
                | Self::TeamArrange
        ) {
            self.decode_lane_body(bytes)
        } else {
            self.decode_runtime_control_body(bytes)
        }
    }

    fn decode_coordination_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::SteerSend => Intent::SteerSend(crate::decode(bytes)?),
            Self::SteerInbox => Intent::SteerInbox(crate::decode(bytes)?),
            Self::SteerHistory => Intent::SteerHistory(crate::decode(bytes)?),
            Self::DirectiveRef => Intent::DirectiveRef(crate::decode(bytes)?),
            Self::SteerDispose => Intent::SteerDispose(crate::decode(bytes)?),
            Self::QuoteRead => Intent::QuoteRead(crate::decode(bytes)?),
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

    fn decode_lane_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::LaneRead => Intent::LaneRead(crate::decode(bytes)?),
            Self::LaneJobs => Intent::LaneJobs(crate::decode(bytes)?),
            Self::LaneJob => Intent::LaneJob(crate::decode(bytes)?),
            Self::LaneSeat => Intent::LaneSeat(crate::decode(bytes)?),
            Self::LaneArrange => Intent::LaneArrange(crate::decode(bytes)?),
            Self::LaneState => Intent::LaneState(crate::decode(bytes)?),
            Self::LaneRenewal => Intent::LaneRenewal(crate::decode(bytes)?),
            Self::LaneRetune => Intent::LaneRetune(crate::decode(bytes)?),
            Self::LaneEffortRetune => Intent::LaneEffortRetune(crate::decode(bytes)?),
            Self::LaneFastRetune => Intent::LaneFastRetune(crate::decode(bytes)?),
            Self::TeamConfigure => Intent::TeamConfigure(crate::decode(bytes)?),
            Self::TeamArrange => Intent::TeamArrange(crate::decode(bytes)?),
            _ => unreachable!("lane body partition is closed"),
        })
    }

    fn decode_runtime_control_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::LaneRecall => Intent::LaneRecall(crate::decode(bytes)?),
            Self::PrincipalDirectoryRead
            | Self::PrincipalRead
            | Self::PrincipalAdmit
            | Self::PrincipalEdit
            | Self::PrincipalRoleRef
            | Self::PrincipalDeauthorize => return self.decode_principal_body(bytes),
            Self::TasteRecord => Intent::TasteRecord(crate::decode(bytes)?),
            Self::Recognize => Intent::Recognize(crate::decode(bytes)?),
            Self::ReleasePublish => Intent::ReleasePublish(crate::decode(bytes)?),
            Self::UpChange => Intent::UpChange(crate::decode(bytes)?),
            _ => unreachable!("runtime body partition is closed"),
        })
    }
}
