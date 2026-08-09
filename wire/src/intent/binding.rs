//! Closed command body and answer bindings.

use serde::Serialize;

use crate::command::CommandKind;

use super::{
    BoardAnswer, BoardGraph, BoardHandout, BoardRead, ClaimAnswer, ClaimCarry, ClaimRenew,
    ClaimWin, ConsentAnswer, ConsentDecide, ConsentPolicy, ConsentPolicyAnswer, ConsentQueueAnswer,
    ConsentRead, DirectiveAnswer, DirectiveRef, DocAnswer, DocApply, DocLedger, EffortAnswer,
    EffortRead, GateFindings, InboxAnswer, LaneAnswer, LaneArrange, LaneChangeAnswer, LaneRead,
    LaneRecall, LaneSeat, LaneState, LineageArrange, LineageConfigure, RecallAnswer,
    RecognitionAnswer, Recognize, ReleasePublish, StateAnswer, SteerDispose, SteerInbox, SteerSend,
    TaskAdd, TaskAnswer, TaskAssign, TaskDefer, TaskDelete, TaskDepends, TaskDone, TaskEdit,
    TaskNote, TaskOopsAdd, TaskPhase, TaskRef, TaskReview, TasteAnswer, TasteRecord, TextAnswer,
    TrustAnswer, TrustRef, UpChange,
};

/// Which intent DTO a command kind's body decodes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyShape {
    /// [`BoardRead`]
    BoardRead,
    /// [`BoardGraph`]
    BoardGraph,
    /// [`BoardHandout`]
    BoardHandout,
    /// [`DocLedger`]
    DocLedger,
    /// [`EffortRead`]
    EffortRead,
    /// [`TaskAdd`]
    TaskAdd,
    /// [`TaskOopsAdd`], the strict body for the server-owned `.oops` route.
    TaskOopsAdd,
    /// [`GateFindings`]
    GateFindings,
    /// [`TaskEdit`]
    TaskEdit,
    /// [`TaskDelete`]
    TaskDelete,
    /// [`TaskNote`]
    TaskNote,
    /// [`TaskDefer`]
    TaskDefer,
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
    /// [`LineageConfigure`]
    LineageConfigure,
    /// [`LineageArrange`]
    LineageArrange,
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
    /// [`ConsentAnswer`]
    Consent,
    /// [`ConsentQueueAnswer`]
    ConsentQueue,
    /// [`ConsentPolicyAnswer`]
    ConsentPolicy,
    /// [`EffortAnswer`]
    Effort,
    /// [`TrustAnswer`]
    Trust,
    /// [`LaneAnswer`]
    Lane,
    /// [`LaneChangeAnswer`]
    LaneChange,
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
    /// [`AnswerShape::Text`]
    Text(TextAnswer),
    /// [`AnswerShape::Task`]
    Task(TaskAnswer),
    /// [`AnswerShape::Claim`]
    Claim(ClaimAnswer),
    /// [`AnswerShape::Doc`]
    Doc(DocAnswer),
    /// [`AnswerShape::Directive`]
    Directive(DirectiveAnswer),
    /// [`AnswerShape::Inbox`]
    Inbox(InboxAnswer),
    /// [`AnswerShape::Consent`]
    Consent(ConsentAnswer),
    /// [`AnswerShape::ConsentQueue`]
    ConsentQueue(ConsentQueueAnswer),
    /// [`AnswerShape::ConsentPolicy`]
    ConsentPolicy(ConsentPolicyAnswer),
    /// [`AnswerShape::Effort`]
    Effort(Box<EffortAnswer>),
    /// [`AnswerShape::Trust`]
    Trust(TrustAnswer),
    /// [`AnswerShape::Lane`]
    Lane(Box<LaneAnswer>),
    /// [`AnswerShape::LaneChange`]
    LaneChange(LaneChangeAnswer),
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
            Self::Text(_) => AnswerShape::Text,
            Self::Task(_) => AnswerShape::Task,
            Self::Claim(_) => AnswerShape::Claim,
            Self::Doc(_) => AnswerShape::Doc,
            Self::Directive(_) => AnswerShape::Directive,
            Self::Inbox(_) => AnswerShape::Inbox,
            Self::Consent(_) => AnswerShape::Consent,
            Self::ConsentQueue(_) => AnswerShape::ConsentQueue,
            Self::ConsentPolicy(_) => AnswerShape::ConsentPolicy,
            Self::Effort(_) => AnswerShape::Effort,
            Self::Trust(_) => AnswerShape::Trust,
            Self::Lane(_) => AnswerShape::Lane,
            Self::LaneChange(_) => AnswerShape::LaneChange,
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
            | Self::Text
            | Self::Task
            | Self::Claim
            | Self::Doc
            | Self::Directive
            | Self::Inbox
            | Self::Consent
            | Self::ConsentQueue => self.decode_primary(bytes),
            Self::ConsentPolicy
            | Self::Effort
            | Self::Trust
            | Self::Lane
            | Self::LaneChange
            | Self::State
            | Self::Recall
            | Self::Taste
            | Self::Recognition => self.decode_secondary(bytes),
        }
    }

    fn decode_primary(self, bytes: &[u8]) -> Result<CommandAnswer, serde_json::Error> {
        Ok(match self {
            Self::Board => CommandAnswer::Board(Box::new(crate::decode(bytes)?)),
            Self::Text => CommandAnswer::Text(crate::decode(bytes)?),
            Self::Task => CommandAnswer::Task(crate::decode(bytes)?),
            Self::Claim => CommandAnswer::Claim(crate::decode(bytes)?),
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
            Self::Effort => CommandAnswer::Effort(Box::new(crate::decode(bytes)?)),
            Self::Trust => CommandAnswer::Trust(crate::decode(bytes)?),
            Self::Lane => CommandAnswer::Lane(Box::new(crate::decode(bytes)?)),
            Self::LaneChange => CommandAnswer::LaneChange(crate::decode(bytes)?),
            Self::State => CommandAnswer::State(crate::decode(bytes)?),
            Self::Recall => CommandAnswer::Recall(Box::new(crate::decode(bytes)?)),
            Self::Taste => CommandAnswer::Taste(crate::decode(bytes)?),
            Self::Recognition => CommandAnswer::Recognition(crate::decode(bytes)?),
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
            | Self::BoardGraph
            | Self::BoardHandout
            | Self::DocLedger
            | Self::EffortRead
            | Self::TaskAdd
            | Self::TaskOopsAdd
            | Self::GateFindings
            | Self::TaskEdit
            | Self::TaskDelete
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
            | Self::SteerInbox
            | Self::SteerResend
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
            | Self::LineageConfigure
            | Self::LineageArrange
            | Self::LaneRecall
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
            Self::BoardGraph => Binding::of(B::BoardGraph, A::Text),
            Self::BoardHandout => Binding::of(B::BoardHandout, A::Text),
            Self::DocLedger => Binding::of(B::DocLedger, A::Text),
            Self::EffortRead => Binding::of(B::EffortRead, A::Effort),
            Self::TaskAdd => Binding::of(B::TaskAdd, A::Task),
            Self::TaskOopsAdd => Binding::of(B::TaskOopsAdd, A::Task),
            Self::GateFindings => Binding::of(B::GateFindings, A::Text),
            Self::TaskEdit => Binding::of(B::TaskEdit, A::Task),
            Self::TaskDelete => Binding::of(B::TaskDelete, A::Task),
            Self::TaskNote => Binding::of(B::TaskNote, A::Task),
            Self::TaskDefer => Binding::of(B::TaskDefer, A::Task),
            Self::TaskWake => Binding::of(B::TaskRef, A::Task),
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
            Self::SteerInbox => Binding::of(B::SteerInbox, A::Inbox),
            Self::SteerResend | Self::SteerCancel => Binding::of(B::DirectiveRef, A::Directive),
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
            Self::LineageConfigure => Binding::of(B::LineageConfigure, A::Lane),
            Self::LineageArrange => Binding::of(B::LineageArrange, A::Lane),
            Self::LaneRecall => Binding::of(B::LaneRecall, A::Recall),
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
    /// [`BodyShape::BoardGraph`]
    BoardGraph(BoardGraph),
    /// [`BodyShape::BoardHandout`]
    BoardHandout(BoardHandout),
    /// [`BodyShape::DocLedger`]
    DocLedger(DocLedger),
    /// [`BodyShape::EffortRead`]
    EffortRead(EffortRead),
    /// [`BodyShape::TaskAdd`]
    TaskAdd(TaskAdd),
    /// [`BodyShape::TaskOopsAdd`]
    TaskOopsAdd(TaskOopsAdd),
    /// [`BodyShape::GateFindings`]
    GateFindings(GateFindings),
    /// [`BodyShape::TaskEdit`]
    TaskEdit(TaskEdit),
    /// [`BodyShape::TaskDelete`]
    TaskDelete(TaskDelete),
    /// [`BodyShape::TaskNote`]
    TaskNote(TaskNote),
    /// [`BodyShape::TaskDefer`]
    TaskDefer(TaskDefer),
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
    /// [`BodyShape::LineageConfigure`]
    LineageConfigure(LineageConfigure),
    /// [`BodyShape::LineageArrange`]
    LineageArrange(LineageArrange),
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
            | Self::BoardGraph(_)
            | Self::BoardHandout(_)
            | Self::DocLedger(_)
            | Self::EffortRead(_)
            | Self::TaskAdd(_)
            | Self::TaskOopsAdd(_)
            | Self::GateFindings(_)
            | Self::TaskEdit(_)
            | Self::TaskDelete(_)
            | Self::TaskNote(_)
            | Self::TaskDefer(_)
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
            | Self::LineageConfigure(_)
            | Self::LineageArrange(_)
            | Self::LaneRecall(_)
            | Self::TasteRecord(_)
            | Self::Recognize(_)
            | Self::ReleasePublish(_)
            | Self::UpChange(_) => self.runtime_shape(),
        }
    }

    const fn board_shape(&self) -> BodyShape {
        match self {
            Self::BoardRead(_) => BodyShape::BoardRead,
            Self::BoardGraph(_) => BodyShape::BoardGraph,
            Self::BoardHandout(_) => BodyShape::BoardHandout,
            Self::DocLedger(_) => BodyShape::DocLedger,
            Self::EffortRead(_) => BodyShape::EffortRead,
            Self::TaskAdd(_) => BodyShape::TaskAdd,
            Self::TaskOopsAdd(_) => BodyShape::TaskOopsAdd,
            Self::GateFindings(_) => BodyShape::GateFindings,
            Self::TaskEdit(_) => BodyShape::TaskEdit,
            Self::TaskDelete(_) => BodyShape::TaskDelete,
            Self::TaskNote(_) => BodyShape::TaskNote,
            Self::TaskDefer(_) => BodyShape::TaskDefer,
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
            Self::SteerInbox(_) => BodyShape::SteerInbox,
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
            Self::LineageConfigure(_) => BodyShape::LineageConfigure,
            Self::LineageArrange(_) => BodyShape::LineageArrange,
            Self::LaneRecall(_) => BodyShape::LaneRecall,
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
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::EffortRead
                | Self::TaskAdd
                | Self::TaskOopsAdd
                | Self::GateFindings
                | Self::TaskEdit
                | Self::TaskDelete
                | Self::TaskNote
                | Self::TaskDefer
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
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::EffortRead
                | Self::TaskAdd
                | Self::TaskOopsAdd
                | Self::GateFindings
                | Self::TaskEdit
                | Self::TaskDelete
                | Self::TaskNote
                | Self::TaskDefer
                | Self::TaskRef
        ) {
            self.decode_board_query_body(bytes)
        } else {
            self.decode_board_transition_body(bytes)
        }
    }

    fn decode_board_query_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
            Self::BoardRead => Intent::BoardRead(crate::decode(bytes)?),
            Self::BoardGraph => Intent::BoardGraph(crate::decode(bytes)?),
            Self::BoardHandout => Intent::BoardHandout(crate::decode(bytes)?),
            Self::DocLedger => Intent::DocLedger(crate::decode(bytes)?),
            Self::EffortRead => Intent::EffortRead(crate::decode(bytes)?),
            Self::TaskAdd => Intent::TaskAdd(crate::decode(bytes)?),
            Self::TaskOopsAdd => Intent::TaskOopsAdd(crate::decode(bytes)?),
            Self::GateFindings => Intent::GateFindings(crate::decode(bytes)?),
            Self::TaskEdit => Intent::TaskEdit(crate::decode(bytes)?),
            Self::TaskDelete => Intent::TaskDelete(crate::decode(bytes)?),
            Self::TaskNote => Intent::TaskNote(crate::decode(bytes)?),
            Self::TaskDefer => Intent::TaskDefer(crate::decode(bytes)?),
            Self::TaskRef => Intent::TaskRef(crate::decode(bytes)?),
            _ => unreachable!("board body partition is closed"),
        })
    }

    fn decode_board_transition_body(self, bytes: &[u8]) -> Result<Intent, serde_json::Error> {
        Ok(match self {
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
                | Self::DirectiveRef
                | Self::SteerDispose
                | Self::ConsentDecide
                | Self::ConsentRead
                | Self::ConsentPolicy
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
            Self::SteerInbox => Intent::SteerInbox(crate::decode(bytes)?),
            Self::DirectiveRef => Intent::DirectiveRef(crate::decode(bytes)?),
            Self::SteerDispose => Intent::SteerDispose(crate::decode(bytes)?),
            Self::ConsentDecide => Intent::ConsentDecide(crate::decode(bytes)?),
            Self::ConsentRead => Intent::ConsentRead(crate::decode(bytes)?),
            Self::ConsentPolicy => Intent::ConsentPolicy(crate::decode(bytes)?),
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
            Self::LineageConfigure => Intent::LineageConfigure(crate::decode(bytes)?),
            Self::LineageArrange => Intent::LineageArrange(crate::decode(bytes)?),
            Self::LaneRecall => Intent::LaneRecall(crate::decode(bytes)?),
            Self::TasteRecord => Intent::TasteRecord(crate::decode(bytes)?),
            Self::Recognize => Intent::Recognize(crate::decode(bytes)?),
            Self::ReleasePublish => Intent::ReleasePublish(crate::decode(bytes)?),
            Self::UpChange => Intent::UpChange(crate::decode(bytes)?),
            _ => unreachable!("runtime body partition is closed"),
        })
    }
}
