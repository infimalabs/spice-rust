//! The Design pass proposal assembled from what the pass built and what the
//! Design's room holds; the holder authors only the justification and, when
//! the pass built nothing, whether the Design concludes or collapses.

use std::collections::BTreeSet;

use wire::Name;
use wire::intent::BoardTask;
use wire::rooms::{
    CandidateReference, DecisionOutcome, DesignPassBody, DesignPassRecommendation,
    PreparationDecided, RoomAuthorityStamp, RoomContributed, RoomContributionKind, RoomSource,
};
use wire::{Seq, SequenceCode, TaskStamp};

use super::*;

/// The Design's room and decision record at the assembled head.
#[derive(Debug, Default)]
pub(in crate::board_command) struct PassRecord {
    /// Every contribution in the room, in journal order.
    pub contributions: Vec<(RoomAuthorityStamp, RoomContributionKind)>,
    /// Every decision under the Design, in journal order, with the outputs
    /// it resolved; a returned decision resolves none.
    pub decisions: Vec<(Seq, Vec<TaskStamp>)>,
}

/// The body one pass of `design` proposes from the open board `tasks` and
/// `record`. Every pending task whose preparation names `design` as parent
/// and itself as construction is an output unless a decision already
/// resolved it; `cites` add references or relabel assembled ones.
pub(in crate::board_command) fn assemble(
    design: &BoardTask,
    tasks: &[BoardTask],
    record: &PassRecord,
    recommend: Option<DesignPassRecommendation>,
    cites: Vec<CandidateReference>,
    justification: String,
) -> Result<DesignPassBody, Outcome<CommandAnswer>> {
    let handle = &design.statement.body.handle;
    let built = built_outputs(design, tasks, record)?;
    let recommendation = recommendation(design, &built, recommend)?;
    let governed_inputs = governed_inputs(design, tasks, &built);
    let references = references(design, record, cites)?;
    DesignPassBody::new(
        recommendation,
        governed_inputs,
        built
            .iter()
            .map(|output| output.statement.body.born.clone())
            .collect(),
        justification,
        references,
    )
    .map_err(|rule| {
        refusal(
            "preparation::proposal-invalid",
            Some(handle.to_string()),
            rule,
        )
    })
}

/// The pending tasks this pass built under `design` that no decision
/// resolved, in birth order, each still gated by the Design and accepted.
fn built_outputs<'a>(
    design: &BoardTask,
    tasks: &'a [BoardTask],
    record: &PassRecord,
) -> Result<Vec<&'a BoardTask>, Outcome<CommandAnswer>> {
    let born = &design.statement.body.born;
    let handle = &design.statement.body.handle;
    let resolved = record
        .decisions
        .iter()
        .flat_map(|(_, outputs)| outputs)
        .collect::<BTreeSet<_>>();
    let mut built = tasks
        .iter()
        .filter(|task| {
            let statement = &task.statement.body;
            task.workflow.body.status.is_pending()
                && statement.preparation.as_ref().is_some_and(|preparation| {
                    preparation.parent == *born && preparation.construction == statement.born
                })
                && !resolved.contains(&statement.born)
        })
        .collect::<Vec<_>>();
    built.sort_by(|left, right| {
        left.statement
            .body
            .born
            .birth_cmp(&right.statement.body.born)
    });
    for output in &built {
        let statement = &output.statement.body;
        if !statement.dependencies.contains(born) {
            return Err(refusal(
                "preparation::output-ungated",
                Some(format!("{} after {handle}", statement.handle)),
                &format!(
                    "restore the edge with `spice task depends {} {handle}`; only a decision \
                     releases work the pass built",
                    statement.handle
                ),
            ));
        }
        if statement.acceptance.is_empty() {
            return Err(refusal(
                "preparation::output-unaccepted",
                Some(statement.handle.to_string()),
                "give the output acceptance with `spice task modify`, or delete it, before \
                 proposing",
            ));
        }
    }
    Ok(built)
}

/// Work when the pass built outputs; otherwise the holder's conclude or
/// collapse, and neither or both refuses.
fn recommendation(
    design: &BoardTask,
    built: &[&BoardTask],
    recommend: Option<DesignPassRecommendation>,
) -> Result<DesignPassRecommendation, Outcome<CommandAnswer>> {
    match (recommend, built.is_empty()) {
        (None, false) => Ok(DesignPassRecommendation::Work),
        (Some(recommendation), true) => Ok(recommendation),
        (None, true) => Err(refusal(
            "preparation::proposal-empty",
            Some(design.statement.body.handle.to_string()),
            "build the work this pass proposes with `spice task add` under the claim, or end \
             it with --recommend conclude or --recommend collapse",
        )),
        (Some(_), false) => Err(refusal(
            "preparation::recommend-with-outputs",
            Some(
                built
                    .iter()
                    .map(|output| output.statement.body.handle.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "a pass that built outputs recommends that work; drop --recommend, or delete the \
             outputs first",
        )),
    }
}

/// The outputs' pending dependencies other than the Design and each other.
fn governed_inputs(
    design: &BoardTask,
    tasks: &[BoardTask],
    built: &[&BoardTask],
) -> Vec<TaskStamp> {
    let born = &design.statement.body.born;
    let mut inputs = Vec::new();
    for output in built {
        for dependency in &output.statement.body.dependencies {
            let pending = tasks.iter().any(|task| {
                task.statement.body.born == *dependency && task.workflow.body.status.is_pending()
            });
            let built_too = built
                .iter()
                .any(|other| other.statement.body.born == *dependency);
            if pending && dependency != born && !built_too && !inputs.contains(dependency) {
                inputs.push(dependency.clone());
            }
        }
    }
    inputs
}

/// Every room contribution, related by its charge or kind, and every
/// decision, with `cites` relabeling an equal source or adding their own.
fn references(
    design: &BoardTask,
    record: &PassRecord,
    cites: Vec<CandidateReference>,
) -> Result<Vec<CandidateReference>, Outcome<CommandAnswer>> {
    let relation = |word: &str| {
        word.parse::<Name>()
            .expect("a fixed relation is a wire name")
    };
    let mut references = record
        .contributions
        .iter()
        .map(|(publication, kind)| CandidateReference {
            relation: relation(match kind {
                RoomContributionKind::Charge(charge) if charge.get() > 0 => "for",
                RoomContributionKind::Charge(charge) if charge.get() < 0 => "against",
                RoomContributionKind::Charge(_) => "evidence",
                RoomContributionKind::Question => "question",
            }),
            source: RoomSource::Contribution {
                room: design.statement.body.born.clone(),
                publication: publication.clone(),
            },
        })
        .chain(record.decisions.iter().map(|(seq, _)| CandidateReference {
            relation: relation("decision"),
            source: RoomSource::Journal {
                fact: SequenceCode::new(seq.get()),
            },
        }))
        .collect::<Vec<_>>();
    for cite in cites {
        match references
            .iter_mut()
            .find(|reference| reference.source == cite.source)
        {
            Some(assembled) => assembled.relation = cite.relation,
            None => references.push(cite),
        }
    }
    if references.is_empty() {
        return Err(refusal(
            "preparation::proposal-unreferenced",
            Some(design.statement.body.handle.to_string()),
            "the room holds no contribution or decision to assemble; cite the authority this \
             pass rests on with --cite RELATION=KEY",
        ));
    }
    Ok(references)
}

/// `design`'s room contributions, its decisions, and the room's newest
/// publication, read at one journal position. A cited publication of the
/// room that is not a contribution, such as an operator post, becomes the
/// journal fact it is.
pub(in crate::board_command) fn pass_record(
    read: &journal::JournalRead<'_>,
    design: &TaskStamp,
    cites: &mut [CandidateReference],
) -> Result<(PassRecord, Option<Seq>), journal::GuardError> {
    let budget = journal::FactReadBudget::RETAINED;
    let mut record = PassRecord::default();
    let mut after = None;
    loop {
        let page = read.room_contributions(design, after, budget)?;
        record
            .contributions
            .extend(page.rows.iter().filter_map(|envelope| {
                envelope
                    .fact
                    .read::<RoomContributed>()
                    .map(|contributed| (contributed.publication, contributed.contribution))
            }));
        let Some(next) = page.continuation else { break };
        after = Some(next);
    }
    let mut after = None;
    loop {
        let page = read.preparation_decisions(design, after, budget)?;
        record
            .decisions
            .extend(page.rows.iter().filter_map(|envelope| {
                let decided = envelope.fact.read::<PreparationDecided>()?;
                let outputs = match decided.decision.outcome {
                    DecisionOutcome::Resolved { outputs, .. } => {
                        outputs.into_iter().map(|output| output.output).collect()
                    }
                    DecisionOutcome::Returned { .. } | DecisionOutcome::Accepted { .. } => {
                        Vec::new()
                    }
                };
                Some((envelope.seq, outputs))
            }));
        let Some(next) = page.continuation else { break };
        after = Some(next);
    }
    for cite in cites.iter_mut() {
        let RoomSource::Contribution { room, publication } = &cite.source else {
            continue;
        };
        let Some(seq) = read.room_publication_seq(room, publication)? else {
            continue;
        };
        let contributed = read
            .room_fact(seq)?
            .is_some_and(|envelope| envelope.fact.read::<RoomContributed>().is_some());
        if !contributed {
            cite.source = RoomSource::Journal {
                fact: SequenceCode::new(seq.get()),
            };
        }
    }
    Ok((record, read.room_publication_head(design)?))
}
