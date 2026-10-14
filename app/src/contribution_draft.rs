//! A lane's typed contribution to one open Design, as drafted in its read
//! view: choose a kind, write the text, cite sources, preview the effect at
//! the Design heads the preview was drawn from, then submit once.
//!
//! The draft never renders a contribution: only the committed room fact
//! does. A refusal, whether a stale Design, a source the resident cannot
//! resolve, or the lane-only rule, stays beside the retained draft and
//! moves nothing else.

use wire::intent::{BoardTask, RoomContribute, RoomContributeAnswer};
use wire::rooms::{
    ROOM_CONTRIBUTION_SOURCE_LIMIT, ROOM_CONTRIBUTION_TEXT_LIMIT, RoomAuthorityStamp,
    RoomContributionKind, RoomSource,
};
use wire::{Digest, SequenceCode, TaskStamp, Witness};

/// One contribution request together with the heads it was drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submission {
    /// The typed request.
    pub request: RoomContribute,
    /// The Design's statement and workflow heads, then each cited source.
    pub expected: Vec<Witness>,
}

/// Where a draft stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftPhase {
    /// Being written; nothing is ready to submit.
    Editing,
    /// Its effect is previewed; Submit sends exactly this.
    Previewing(Submission),
    /// Submitted; no second submission leaves until it settles.
    Pending(Submission),
}

/// The outcome of the last preview or submission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftNotice {
    /// Refused, locally or by the resident, with its stable reason.
    Refused(String),
    /// The resident recorded the contribution under this identity.
    Contributed(RoomAuthorityStamp),
}

/// One Design's contribution in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContributionDraft {
    design: TaskStamp,
    kind: Option<RoomContributionKind>,
    text: String,
    sources: Vec<RoomSource>,
    phase: DraftPhase,
    notice: Option<DraftNotice>,
}

impl ContributionDraft {
    /// An empty draft for `design`.
    #[must_use]
    pub const fn new(design: TaskStamp) -> Self {
        Self {
            design,
            kind: None,
            text: String::new(),
            sources: Vec::new(),
            phase: DraftPhase::Editing,
            notice: None,
        }
    }

    /// The Design this draft contributes to.
    #[must_use]
    #[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
    pub const fn design(&self) -> &TaskStamp {
        &self.design
    }

    /// The chosen kind, if any.
    #[must_use]
    pub const fn kind(&self) -> Option<RoomContributionKind> {
        self.kind
    }

    /// The text as written.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Cited sources, in citation order.
    #[must_use]
    pub fn sources(&self) -> &[RoomSource] {
        &self.sources
    }

    /// Where the draft stands.
    #[must_use]
    pub const fn phase(&self) -> &DraftPhase {
        &self.phase
    }

    /// The last outcome.
    #[must_use]
    pub const fn notice(&self) -> Option<&DraftNotice> {
        self.notice.as_ref()
    }

    /// Whether a submission is in flight.
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self.phase, DraftPhase::Pending(_))
    }

    /// Choose what the contribution offers.
    pub fn set_kind(&mut self, kind: RoomContributionKind) {
        self.edit(|draft| draft.kind = Some(kind));
    }

    /// Replace the text.
    pub fn set_text(&mut self, text: String) {
        self.edit(|draft| draft.text = text);
    }

    /// Cite `source`, or stop citing it when already cited.
    pub fn toggle_source(&mut self, source: RoomSource) {
        self.edit(|draft| {
            if let Some(index) = draft.sources.iter().position(|cited| *cited == source) {
                draft.sources.remove(index);
            } else if draft.sources.len() < ROOM_CONTRIBUTION_SOURCE_LIMIT {
                draft.sources.push(source);
            } else {
                draft.notice = Some(DraftNotice::Refused(format!(
                    "room::contribution-sources-limit: cite at most {ROOM_CONTRIBUTION_SOURCE_LIMIT} sources"
                )));
            }
        });
    }

    /// Cite the artifact digest or journal fact code `spelling` names; an
    /// unreadable spelling is refused beside the draft.
    pub fn cite(&mut self, spelling: &str) {
        let spelling = spelling.trim();
        let source = spelling
            .parse::<Digest>()
            .map(|digest| RoomSource::Artifact { digest })
            .or_else(|_| {
                spelling
                    .parse::<SequenceCode>()
                    .map(|fact| RoomSource::Journal { fact })
            });
        match source {
            Ok(source) if !self.sources.contains(&source) => self.toggle_source(source),
            Ok(_) => {}
            Err(_) => {
                self.notice = Some(DraftNotice::Refused(
                    "room::contribution-source-unreadable: name an artifact digest or a journal fact code"
                        .to_owned(),
                ));
            }
        }
    }

    /// Retract the preview and keep editing.
    pub fn revise(&mut self) {
        self.edit(|_| {});
    }

    /// Preview the contribution against `design` as the board shows it now,
    /// or refuse beside the draft when it cannot be submitted.
    pub fn preview(&mut self, design: &BoardTask) {
        if self.is_pending() {
            return;
        }
        match self.submission(design) {
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

    /// Take the previewed submission exactly once; `None` unless previewing.
    pub fn begin_submit(&mut self) -> Option<Submission> {
        if !matches!(self.phase, DraftPhase::Previewing(_)) {
            return None;
        }
        let DraftPhase::Previewing(submission) =
            std::mem::replace(&mut self.phase, DraftPhase::Editing)
        else {
            unreachable!("checked previewing above");
        };
        self.phase = DraftPhase::Pending(submission.clone());
        self.notice = None;
        Some(submission)
    }

    /// Settle the pending submission: a recorded contribution clears the
    /// text and sources; a refusal keeps them, with its reason beside them.
    pub fn settle(&mut self, outcome: Result<RoomContributeAnswer, String>) {
        if !self.is_pending() {
            return;
        }
        self.phase = DraftPhase::Editing;
        match outcome {
            Ok(answer) => {
                self.text.clear();
                self.sources.clear();
                self.notice = Some(DraftNotice::Contributed(answer.publication));
            }
            Err(reason) => self.notice = Some(DraftNotice::Refused(reason)),
        }
    }

    /// Apply one edit; a pending draft is frozen, and an edit retires any
    /// preview drawn from the earlier draft.
    fn edit(&mut self, change: impl FnOnce(&mut Self)) {
        if self.is_pending() {
            return;
        }
        self.phase = DraftPhase::Editing;
        self.notice = None;
        change(self);
    }

    fn submission(&self, design: &BoardTask) -> Result<Submission, String> {
        if design.statement.body.born != self.design {
            return Err(
                "room::contribution-design-mismatch: preview the Design this draft names"
                    .to_owned(),
            );
        }
        if !design
            .design_standing()
            .is_some_and(|standing| standing.is_open())
        {
            return Err(
                "room::design-not-open: a concluded Design takes no contribution".to_owned(),
            );
        }
        let Some(kind) = self.kind else {
            return Err(
                "room::contribution-kind-missing: choose what the contribution offers".to_owned(),
            );
        };
        let text = self.text.trim();
        if text.is_empty() {
            return Err("room::contribution-empty: write what the contribution offers".to_owned());
        }
        if text.len() > ROOM_CONTRIBUTION_TEXT_LIMIT {
            return Err(format!(
                "room::contribution-too-long: keep it within {ROOM_CONTRIBUTION_TEXT_LIMIT} bytes"
            ));
        }
        let born = &design.statement.body.born;
        let mut expected = vec![
            Witness::Task {
                task: born.clone(),
                head: design.statement.position,
            },
            Witness::Workflow {
                task: born.clone(),
                head: design.workflow.position,
                phase: design.workflow.body.phase.clone(),
            },
        ];
        expected.extend(self.sources.iter().map(|source| Witness::Source {
            source: source.authority_digest(),
        }));
        Ok(Submission {
            request: RoomContribute {
                room: design.statement.body.handle.clone(),
                kind,
                text: text.to_owned(),
                sources: self.sources.clone(),
            },
            expected,
        })
    }
}

#[cfg(test)]
mod tests;
