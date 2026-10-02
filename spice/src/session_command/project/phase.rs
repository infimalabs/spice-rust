//! Where a working session turns over.
//!
//! A phase is a run of turns doing the same kind of work. Only two things end
//! one: a long idle gap, or a change of activity family carrying turns on both
//! sides of it. The rest of this module exists so that a single turn stepping
//! out of the pattern and straight back into it does not read as a change of
//! direction.

use clap::ArgMatches;
use wire::Refusal;

use super::super::dataset::Aggregate;
use super::super::model::PhaseExample;
use super::super::page::{self, Page};
use super::{Filters, Report, ResolvedSource, TurnRecord, one, report};

const DEFAULT_PHASE_EXAMPLES: usize = 2;
const MAX_PHASE_EXAMPLES: usize = 16;
/// Cursor prefix naming a phase by its ordinal from the first.
const PHASE_KEY: &str = "phase-";

#[derive(Clone, Copy)]
pub(crate) struct PhaseOptions {
    pub(crate) rows: usize,
    pub(crate) examples: usize,
    pub(crate) max_text: usize,
    /// The newest phase ordinal a resumed page considers.
    pub(crate) through: Option<usize>,
}

/// One page of phases, newest first, keyed by ordinal.
pub(crate) fn phases(
    sources: &[ResolvedSource],
    matches: &ArgMatches,
    page: Page,
) -> Result<Report, Refusal> {
    let examples = one(matches, "examples").map_or(Ok(DEFAULT_PHASE_EXAMPLES), |value| {
        value.parse::<usize>().map_err(|_| {
            super::refusal(
                "session::bound-invalid",
                Some(format!("--examples={value}")),
                "pass a nonnegative decimal integer bound",
            )
        })
    })?;
    if examples > MAX_PHASE_EXAMPLES {
        return Err(super::refusal(
            "session::bound-exceeded",
            Some(format!(
                "--examples={examples} maximum={MAX_PHASE_EXAMPLES}"
            )),
            "pass a phase example bound within the repository ceiling",
        ));
    }
    let options = PhaseOptions {
        rows: page.retain(),
        examples,
        max_text: page.limits().text,
        through: page.after_ordinal(PHASE_KEY)?,
    };
    let data = Aggregate::scan_phases(sources, options, Filters::from_matches(matches)?)?;
    let rows = data
        .rows
        .into_iter()
        .map(|row| {
            let key = format!("{PHASE_KEY}{}", row.index);
            (row, key)
        })
        .collect();
    let paged = page.admit(rows, data.population)?;
    Ok(report(paged, |page| Report::Phases {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

pub(crate) fn turn_family(turn: &TurnRecord) -> String {
    if turn.patch_count > 0 {
        "implementation"
    } else if turn.command_count > 0 {
        "execution"
    } else if turn.web_search_count > 0 {
        "research"
    } else {
        "dialogue"
    }
    .to_owned()
}

pub(crate) fn phase_example(turn: &TurnRecord, max_text: usize) -> PhaseExample {
    PhaseExample {
        start: turn.start_ts.clone(),
        turn_id: turn.turn_id.clone(),
        archetype: turn_family(turn),
        path: turn.touched_files.iter().next().cloned(),
        user: turn
            .messages
            .iter()
            .find(|message| message.side == "user" && message.authored.is_some())
            .and_then(|message| message.authored.as_deref())
            .map(|message| page::clip(message, max_text)),
        final_answer: turn
            .messages
            .iter()
            .rev()
            .find(|message| message.phase == "final_answer" && message.authored.is_some())
            .and_then(|message| message.authored.as_deref())
            .map(|message| page::clip(message, max_text)),
    }
}
