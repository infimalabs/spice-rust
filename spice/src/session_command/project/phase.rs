//! Where a working session turns over.
//!
//! A phase is a run of turns doing the same kind of work. Only two things end
//! one: a long idle gap, or a change of activity family carrying turns on both
//! sides of it. The rest of this module exists so that a single turn stepping
//! out of the pattern and straight back into it does not read as a change of
//! direction.

use clap::ArgMatches;
use wire::Refusal;

use super::{
    DEFAULT_PHASE_EXAMPLES, DEFAULT_PHASE_TEXT, PhaseExample, TurnRecord, clip, number,
    number_allow_zero, one,
};
const MAX_PHASE_ROWS: usize = 256;
const MAX_PHASE_EXAMPLES: usize = 16;
const MAX_PHASE_TEXT: usize = 16 * 1_024;

#[derive(Clone, Copy)]
pub(crate) struct PhaseOptions {
    pub(crate) rows: usize,
    pub(crate) examples: usize,
    pub(crate) max_text: usize,
}

pub(crate) fn options(matches: &ArgMatches) -> Result<PhaseOptions, Refusal> {
    let options = PhaseOptions {
        rows: one(matches, "limit")
            .map(|_| number(matches, "limit", 1))
            .transpose()?
            .unwrap_or(MAX_PHASE_ROWS),
        examples: number_allow_zero(matches, "examples", DEFAULT_PHASE_EXAMPLES)?,
        max_text: number(matches, "max-text", DEFAULT_PHASE_TEXT)?,
    };
    for (name, value, maximum) in [
        ("limit", options.rows, MAX_PHASE_ROWS),
        ("examples", options.examples, MAX_PHASE_EXAMPLES),
        ("max-text", options.max_text, MAX_PHASE_TEXT),
    ] {
        if value > maximum {
            return Err(super::refusal(
                "session::bound-exceeded",
                Some(format!("--{name}={value} maximum={maximum}")),
                "pass a phase report bound within the repository ceiling",
            ));
        }
    }
    Ok(options)
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
            .map(|message| clip(message, max_text)),
        final_answer: turn
            .messages
            .iter()
            .rev()
            .find(|message| message.phase == "final_answer" && message.authored.is_some())
            .and_then(|message| message.authored.as_deref())
            .map(|message| clip(message, max_text)),
    }
}
