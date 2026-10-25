//! What a task sheet and a Design pass proposal show after their title and
//! pitch lead, in order: the sheet's three closed blocks with the count each
//! summary carries, and a proposal's uncollapsed output and input lines
//! followed by its reference count and collapsed justification.

use wire::TaskStamp;
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
use wire::intent::BoardStatement;
use wire::rooms::DesignPassProposal;

/// One closed `details` block beneath the sheet's title and pitch.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SheetBlock {
    /// The summary's label.
    pub label: &'static str,
    /// What the summary counts: description lines, acceptance items, or history facts.
    pub count: usize,
    /// What the open block reads when it counts nothing.
    pub empty: &'static str,
}

/// The sheet's blocks in reading order: Description, then Acceptance (or a
/// Design's Questions), then History over `history_facts` scalar facts.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub fn sheet_blocks(statement: &BoardStatement, history_facts: usize) -> [SheetBlock; 3] {
    let design = statement.intent == Some(wire::TaskIntent::Design);
    [
        SheetBlock {
            label: "Description",
            count: description_lines(statement.description.as_deref()),
            empty: "No description was recorded.",
        },
        if design {
            SheetBlock {
                label: "Questions",
                count: statement.acceptance.len(),
                empty: "No open questions were recorded.",
            }
        } else {
            SheetBlock {
                label: "Acceptance",
                count: statement.acceptance.len(),
                empty: "No acceptance criteria were recorded.",
            }
        },
        SheetBlock {
            label: "History",
            count: history_facts,
            empty: "No history was recorded.",
        },
    ]
}

/// How many prose lines a description carries; blank lines separate, they
/// do not count.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub fn description_lines(description: Option<&str>) -> usize {
    description.map_or(0, |text| {
        text.lines().filter(|line| !line.trim().is_empty()).count()
    })
}

/// One line of a proposal beneath its pitch-and-recommendation head.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalLine<'a> {
    /// A proposed output: its handle, pitch, and verdict control stay uncollapsed.
    Output(&'a TaskStamp),
    /// A governed input, presented as an output line without a verdict.
    Input(&'a TaskStamp),
    /// Retained references, shown only as their count.
    References(usize),
    /// The authored justification, collapsed beneath everything decided.
    Justification(&'a str),
}

/// A proposal's lines in reading order: every output, every governed input,
/// the reference count, then the justification.
#[cfg(any(target_arch = "wasm32", rust_analyzer))]
pub fn proposal_lines(candidate: &DesignPassProposal) -> Vec<ProposalLine<'_>> {
    candidate
        .outputs
        .iter()
        .map(ProposalLine::Output)
        .chain(candidate.governed_inputs.iter().map(ProposalLine::Input))
        .chain([
            ProposalLine::References(candidate.references.len()),
            ProposalLine::Justification(&candidate.justification),
        ])
        .collect()
}

/// Preparation-menu JSON is display text, not the proposal's wire encoding.
/// Convert only typed journal positions; authored numbers and source offsets
/// retain their exact meaning.
pub(crate) fn proposal_record(proposal: &DesignPassProposal) -> String {
    #[derive(serde::Serialize)]
    struct Display<'a> {
        candidate: &'a wire::rooms::CandidateRef,
        statement: wire::SequenceCode,
        #[serde(skip_serializing_if = "Option::is_none")]
        claim: Option<wire::SequenceCode>,
        recommendation: &'a wire::rooms::DesignPassRecommendation,
        governed_inputs: &'a [TaskStamp],
        outputs: &'a [TaskStamp],
        justification: &'a str,
        references: &'a [wire::rooms::CandidateReference],
    }

    let DesignPassProposal {
        candidate,
        statement,
        claim,
        recommendation,
        governed_inputs,
        outputs,
        justification,
        references,
    } = proposal;
    serde_json::to_string(&Display {
        candidate,
        statement: wire::SequenceCode::new(statement.get()),
        claim: claim.map(|claim| wire::SequenceCode::new(claim.get())),
        recommendation,
        governed_inputs,
        outputs,
        justification,
        references,
    })
    .expect("proposal display encodes")
}

#[cfg(test)]
mod tests;
