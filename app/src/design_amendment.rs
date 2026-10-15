//! One invited occupant's title and description amendment for an open Design.
//!
//! Editing and previewing are browser-local. A draft remembers the statement
//! it was drawn from: preview sends only the fields the occupant changed from
//! that base and witnesses its head, so a statement edit landing meanwhile
//! refuses through the ordinary `TaskEdit` witness instead of being reverted.
//! A refusal returns to editing without changing either entered field. Each
//! entered field keeps at most the bytes one durable fact can carry.

use wire::intent::{BoardTask, TaskEdit};
use wire::rooms::DesignNoticed;
use wire::{CanonicalUuid, TaskStamp, Witness};

/// Most bytes one entered field retains: an amendment lands as one fact.
const FIELD_BYTES: usize = wire::MAX_DURABLE_FACT_ENVELOPE_BYTES;

/// The durable invitation through which this exact current lane occupant
/// may amend the still-open Design, if any.
#[must_use]
pub fn invitation<'a>(
    design: &BoardTask,
    identity: Option<(CanonicalUuid, CanonicalUuid)>,
    notices: impl IntoIterator<Item = &'a DesignNoticed>,
) -> Option<&'a DesignNoticed> {
    let (lane, occupant) = identity?;
    if !design
        .design_standing()
        .is_some_and(|standing| standing.is_open())
    {
        return None;
    }
    notices.into_iter().find(|notice| {
        notice.design == design.statement.body.born
            && notice.lane == lane
            && notice.occupant == occupant
    })
}

/// One statement amendment together with the exact head it was drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// Ordinary task edit; unchanged fields stay absent.
    pub request: TaskEdit,
    /// Exactly the statement head shown by the preview.
    pub expected: Vec<Witness>,
}

/// Where the amendment draft stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftPhase {
    /// Fields can be changed and previewed.
    Editing,
    /// The exact request is visible and ready to submit.
    Previewing(Submission),
    /// The exact request is in flight and cannot be sent again.
    Pending(Submission),
}

/// Outcome of the latest preview or submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftNotice {
    /// Local validation or resident refusal.
    Refused(String),
    /// The resident accepted the ordinary task edit.
    Saved,
}
/// The statement fields an amendment compares, without cloning the rest of a
/// board row into each browser-local transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatementSnapshot {
    design: TaskStamp,
    handle: wire::TaskHandle,
    title: String,
    description: String,
    head: wire::Seq,
    open: bool,
}

impl StatementSnapshot {
    #[must_use]
    pub(crate) fn new(design: &BoardTask) -> Self {
        Self {
            design: design.statement.body.born.clone(),
            handle: design.statement.body.handle.clone(),
            title: design.presentation_title.trim().to_owned(),
            description: design
                .statement
                .body
                .description
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_owned(),
            head: design.statement.position,
            open: design
                .design_standing()
                .is_some_and(|standing| standing.is_open()),
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[must_use]
    pub(crate) const fn design(&self) -> &TaskStamp {
        &self.design
    }
}

/// Browser-local amendment fields for one Design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesignAmendment {
    /// The statement the entered fields were drawn from.
    base: StatementSnapshot,
    /// Set once a submission settles: the next preview first rebases onto the
    /// statement current then.
    rebase: bool,
    title: String,
    description: String,
    phase: DraftPhase,
    notice: Option<DraftNotice>,
}

impl DesignAmendment {
    /// Start from the statement currently shown for `design`.
    #[must_use]
    pub fn new(design: &BoardTask) -> Self {
        Self::from_snapshot(&StatementSnapshot::new(design))
    }

    #[must_use]
    pub(crate) fn from_snapshot(statement: &StatementSnapshot) -> Self {
        Self {
            base: statement.clone(),
            rebase: false,
            title: statement.title.clone(),
            description: statement.description.clone(),
            phase: DraftPhase::Editing,
            notice: None,
        }
    }

    /// Entered title, retained verbatim across refusal.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Entered description, retained verbatim across refusal.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Current phase.
    #[must_use]
    pub const fn phase(&self) -> &DraftPhase {
        &self.phase
    }

    /// Latest outcome.
    #[must_use]
    pub const fn notice(&self) -> Option<&DraftNotice> {
        self.notice.as_ref()
    }

    /// Whether a submission is in flight.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self.phase, DraftPhase::Pending(_))
    }

    /// Replace the title and retire any earlier preview or outcome.
    pub fn set_title(&mut self, title: String) {
        self.edit(|draft| draft.title = draft.bounded(title));
    }

    /// Replace the description and retire any earlier preview or outcome.
    pub fn set_description(&mut self, description: String) {
        self.edit(|draft| draft.description = draft.bounded(description));
    }

    /// `text` clipped at a character boundary within [`FIELD_BYTES`], noting
    /// the clip as a refusal of the excess.
    fn bounded(&mut self, mut text: String) -> String {
        if text.len() > FIELD_BYTES {
            let mut end = FIELD_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            self.notice = Some(DraftNotice::Refused(format!(
                "task::text-too-long: kept the first {FIELD_BYTES} bytes one statement can carry"
            )));
        }
        text
    }

    #[cfg(test)]
    pub fn preview(&mut self, design: &BoardTask) {
        self.preview_snapshot(&StatementSnapshot::new(design));
    }

    pub(crate) fn preview_snapshot(&mut self, statement: &StatementSnapshot) {
        if self.is_pending() {
            return;
        }
        if self.rebase && statement.design == self.base.design {
            self.rebase_onto(statement);
        }
        match self.submission(statement) {
            Ok(submission) => {
                self.phase = DraftPhase::Previewing(submission);
                self.notice = None;
            }
            Err(reason) => {
                self.phase = DraftPhase::Editing;
                self.notice = Some(DraftNotice::Refused(reason));
            }
        }
    }

    /// Retract the preview while retaining both fields.
    pub fn revise(&mut self) {
        self.edit(|_| {});
    }

    /// Take the preview once and freeze it as pending.
    pub fn begin_submit(&mut self) -> Option<Submission> {
        let DraftPhase::Previewing(submission) = &self.phase else {
            return None;
        };
        let submission = submission.clone();
        self.phase = DraftPhase::Pending(submission.clone());
        self.notice = None;
        Some(submission)
    }

    /// Settle the pending edit. Success records the exact trimmed values
    /// submitted; refusal retains the entered values verbatim. Either way the
    /// next preview rebases onto the statement current then.
    pub fn settle(&mut self, outcome: Result<(), String>) {
        let DraftPhase::Pending(submission) = &self.phase else {
            return;
        };
        let submission = submission.clone();
        self.phase = DraftPhase::Editing;
        self.rebase = true;
        match outcome {
            Ok(()) => {
                if let Some(title) = submission.request.title {
                    self.base.title.clone_from(&title);
                    self.title = title;
                }
                if let Some(description) = submission.request.description {
                    self.base.description.clone_from(&description);
                    self.description = description;
                }
                self.notice = Some(DraftNotice::Saved);
            }
            Err(reason) => self.notice = Some(DraftNotice::Refused(reason)),
        }
    }

    /// Adopt `statement` as the base: a field still equal to the old base
    /// follows the statement, while a changed field keeps its entered text.
    fn rebase_onto(&mut self, statement: &StatementSnapshot) {
        if self.title.trim() == self.base.title {
            self.title.clone_from(&statement.title);
        }
        if self.description.trim() == self.base.description {
            self.description.clone_from(&statement.description);
        }
        self.base = statement.clone();
        self.rebase = false;
    }

    fn edit(&mut self, change: impl FnOnce(&mut Self)) {
        if self.is_pending() {
            return;
        }
        self.phase = DraftPhase::Editing;
        self.notice = None;
        change(self);
    }

    /// Changed fields relative to the base, witnessed at the base head. Only
    /// the current statement's design and standing are consulted.
    fn submission(&self, statement: &StatementSnapshot) -> Result<Submission, String> {
        let base = &self.base;
        if statement.design != base.design {
            return Err(
                "task::edit-design-mismatch: preview the Design this draft names".to_owned(),
            );
        }
        if !statement.open {
            return Err("task::settled: a settled Design statement cannot be amended".to_owned());
        }
        let title = self.title.trim();
        if title.is_empty() {
            return Err("task::title-empty: write a title".to_owned());
        }
        let description = self.description.trim();
        let title = (title != base.title).then(|| title.to_owned());
        let description = (description != base.description).then(|| description.to_owned());
        if title.is_none() && description.is_none() {
            return Err("task::edit-empty: change the title or description".to_owned());
        }
        Ok(Submission {
            request: TaskEdit {
                task: base.handle.clone(),
                title,
                description,
                acceptance: Vec::new(),
                requirements: None,
                intent: None,
                evidence: None,
                priority: None,
                project: None,
                flow: None,
            },
            expected: vec![Witness::Task {
                task: base.design.clone(),
                head: base.head,
            }],
        })
    }
}

#[cfg(test)]
mod tests;
