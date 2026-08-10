//! Where a working session turns over.
//!
//! A phase is a run of turns doing the same kind of work. Only two things end
//! one: a long idle gap, or a change of activity family carrying turns on both
//! sides of it. The rest of this module exists so that a single turn stepping
//! out of the pattern and straight back into it does not read as a change of
//! direction.

use std::collections::BTreeMap;

use clap::ArgMatches;
use wire::Refusal;

use super::{
    DEFAULT_PHASE_EXAMPLES, DEFAULT_PHASE_TEXT, Dataset, Filters, PhaseExample, PhaseView, Report,
    TurnRecord, clip, number, number_allow_zero, one, turn_end,
};

/// Idle time that ends a phase on its own, without any change of activity.
const PHASE_GAP_SECONDS: i64 = 45 * 60;
/// Turns of the same family each side of a change must carry to make it a phase.
const PHASE_SUPPORT_TURNS: usize = 2;

pub(crate) fn phases(
    dataset: &Dataset,
    matches: &ArgMatches,
    filters: &Filters,
) -> Result<Report, Refusal> {
    filters.validate_turn_ids(dataset)?;
    let examples = number_allow_zero(matches, "examples", DEFAULT_PHASE_EXAMPLES)?;
    let max_text = number(matches, "max-text", DEFAULT_PHASE_TEXT)?;
    let turns = dataset
        .turns
        .iter()
        .filter(|turn| filters.turn_matches(turn))
        .collect::<Vec<_>>();
    let mut rows = build_phases(&turns, examples, max_text);
    if one(matches, "limit").is_some() {
        rows.truncate(number(matches, "limit", rows.len().max(1))?);
    }
    Ok(Report::Phases {
        sources: dataset.sources.clone(),
        rows,
        residues: dataset.residues.clone(),
    })
}
fn build_phases(turns: &[&TurnRecord], examples: usize, max_text: usize) -> Vec<PhaseView> {
    segment_phases(turns)
        .into_iter()
        .enumerate()
        .map(|(index, rows)| phase_view(index + 1, &rows, examples, max_text))
        .collect()
}

/// Cut the turn sequence where a phase actually turns over.
///
/// Two things end a phase and nothing else does: a long idle gap, or a change
/// of activity family carrying at least two turns on each side of it. A lone
/// turn that breaks the pattern and hands it straight back can satisfy neither
/// side, so it stays inside the phase it interrupted instead of splitting it.
fn segment_phases<'turn>(turns: &[&'turn TurnRecord]) -> Vec<Vec<&'turn TurnRecord>> {
    let families = turns
        .iter()
        .map(|turn| turn_family(turn))
        .collect::<Vec<_>>();
    let mut groups: Vec<Vec<&TurnRecord>> = Vec::new();
    for (index, turn) in turns.iter().enumerate() {
        if index == 0 || starts_phase(turns, &families, index) {
            groups.push(vec![*turn]);
        } else {
            groups
                .last_mut()
                .expect("the first turn opened a group")
                .push(*turn);
        }
    }
    groups
}

fn starts_phase(turns: &[&TurnRecord], families: &[String], index: usize) -> bool {
    if idle_seconds(turns[index - 1], turns[index]) >= PHASE_GAP_SECONDS {
        return true;
    }
    families[index] != families[index - 1] && change_is_supported(families, index)
}

/// Whether the family change arriving at `index` is supported on both sides.
///
/// Support is the unbroken run of the outgoing family behind the change and of
/// the incoming family ahead of it, and both must reach [`PHASE_SUPPORT_TURNS`].
/// Requiring the same evidence on each side is what makes a lone deviating turn
/// unable to open a phase or to close one: it is its own whole run, so the
/// boundary into it fails ahead and the boundary out of it fails behind.
fn change_is_supported(families: &[String], index: usize) -> bool {
    let behind = families[..index]
        .iter()
        .rev()
        .take_while(|family| **family == families[index - 1])
        .count();
    let ahead = families[index..]
        .iter()
        .take_while(|family| **family == families[index])
        .count();
    behind >= PHASE_SUPPORT_TURNS && ahead >= PHASE_SUPPORT_TURNS
}

/// The family a phase is named for: most turns wins, earliest appearance breaks
/// a tie, so the label does not depend on how the map is iterated.
fn dominant_family(turns: &[&TurnRecord]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for turn in turns {
        let family = turn_family(turn);
        if let Some(entry) = counts.iter_mut().find(|entry| entry.0 == family) {
            entry.1 += 1;
        } else {
            counts.push((family, 1));
        }
    }
    counts
        .into_iter()
        .reduce(|best, next| if next.1 > best.1 { next } else { best })
        .map_or_else(String::new, |(family, _)| family)
}

/// Seconds between the end of `previous` and the start of `current`.
///
/// A gap only exists when both ends are declared: a timestamp that does not
/// parse yields no gap rather than an assumed one, so an undated turn cannot
/// manufacture a boundary.
fn idle_seconds(previous: &TurnRecord, current: &TurnRecord) -> i64 {
    let (Ok(start), Ok(end)) = (
        turn_end(previous).parse::<jiff::Timestamp>(),
        current.start_ts.parse::<jiff::Timestamp>(),
    ) else {
        return 0;
    };
    end.as_second() - start.as_second()
}

fn phase_view(index: usize, rows: &[&TurnRecord], examples: usize, max_text: usize) -> PhaseView {
    let family = dominant_family(rows);
    PhaseView {
        index,
        primary: family.clone(),
        family,
        turns: rows.len(),
        commands: rows.iter().map(|turn| turn.command_count).sum(),
        patches: rows.iter().map(|turn| turn.patch_count).sum(),
        compactions: rows.iter().map(|turn| turn.compaction_count).sum(),
        errors: rows.iter().map(|turn| turn.error_count).sum(),
        duration_millis: None,
        start: rows
            .first()
            .map_or_else(String::new, |turn| turn.start_ts.clone()),
        end: rows.last().map_or_else(String::new, |turn| turn_end(turn)),
        top_paths: path_counts(rows),
        examples: rows
            .iter()
            .take(examples)
            .map(|turn| phase_example(turn, max_text))
            .collect(),
    }
}

fn turn_family(turn: &TurnRecord) -> String {
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

fn path_counts(turns: &[&TurnRecord]) -> Vec<String> {
    let mut counts = BTreeMap::new();
    for path in turns.iter().flat_map(|turn| turn.touched_files.iter()) {
        *counts.entry(path.clone()).or_insert(0_usize) += 1;
    }
    let mut rows = counts.into_iter().collect::<Vec<_>>();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.into_iter().take(5).map(|(path, _)| path).collect()
}

fn phase_example(turn: &TurnRecord, max_text: usize) -> PhaseExample {
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
#[cfg(test)]
mod phase_tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::session_command::model::Vendor;

    use super::{
        PhaseView, TurnRecord, build_phases, dominant_family, segment_phases, turn_family,
    };

    const ORIGIN: &str = "2026-01-01T00:00:00Z";
    const DIALOGUE: &str = "dialogue";
    const EXECUTION: &str = "execution";
    const IMPLEMENTATION: &str = "implementation";

    fn at(minute: i64) -> String {
        ORIGIN
            .parse::<jiff::Timestamp>()
            .expect("fixture origin parses")
            .checked_add(jiff::SignedDuration::from_mins(minute))
            .expect("fixture offset stays in range")
            .to_string()
    }

    /// One turn of `family`, open from `minute` until `last_minute`.
    ///
    /// The requested family is asserted against the classifier rather than
    /// trusted, so a fixture that miscounts its own activity fails here instead
    /// of quietly testing a phase made of some other family.
    fn open_turn(source: &str, minute: i64, last_minute: i64, family: &str) -> TurnRecord {
        let record = TurnRecord {
            source: source.to_owned(),
            vendor: Vendor::Claude,
            turn_id: Some(format!("{source}-{minute}")),
            start_ts: at(minute),
            end_ts: None,
            last_activity_ts: at(last_minute),
            completed: false,
            messages: Vec::new(),
            tool_calls: BTreeMap::new(),
            touched_files: BTreeSet::new(),
            command_count: u64::from(family == EXECUTION),
            patch_count: u64::from(family == IMPLEMENTATION),
            error_count: 0,
            web_search_count: 0,
            compaction_count: 0,
        };
        assert_eq!(
            turn_family(&record),
            family,
            "fixture turn carries the family it is named for"
        );
        record
    }

    fn turn(minute: i64, family: &str) -> TurnRecord {
        TurnRecord {
            end_ts: Some(at(minute + 1)),
            completed: true,
            ..open_turn("one", minute, minute + 1, family)
        }
    }

    fn run(turns: &[TurnRecord], family: &str, count: usize) -> Vec<TurnRecord> {
        let start = turns.last().map_or(-5, minute_of) + 5;
        (0..count)
            .map(|offset| {
                turn(
                    start + 5 * i64::try_from(offset).expect("fixture run is small"),
                    family,
                )
            })
            .collect()
    }

    fn minute_of(turn: &TurnRecord) -> i64 {
        turn.start_ts
            .parse::<jiff::Timestamp>()
            .expect("fixture turn is dated")
            .as_second()
            / 60
            - ORIGIN
                .parse::<jiff::Timestamp>()
                .expect("fixture origin parses")
                .as_second()
                / 60
    }

    fn chain(spans: &[(&str, usize)]) -> Vec<TurnRecord> {
        let mut turns = Vec::new();
        for (family, count) in spans {
            turns.extend(run(&turns, family, *count));
        }
        turns
    }

    /// Each phase as the family it is named for and how many turns it holds.
    fn shape(turns: &[TurnRecord]) -> Vec<(String, usize)> {
        let borrowed = turns.iter().collect::<Vec<_>>();
        segment_phases(&borrowed)
            .iter()
            .map(|group| (dominant_family(group), group.len()))
            .collect()
    }

    fn phases(turns: &[TurnRecord], examples: usize) -> Vec<PhaseView> {
        let borrowed = turns.iter().collect::<Vec<_>>();
        build_phases(&borrowed, examples, 80)
    }

    fn named(family: &str, count: usize) -> (String, usize) {
        (family.to_owned(), count)
    }

    #[test]
    fn a_change_carrying_turns_on_both_sides_opens_a_phase() {
        let turns = chain(&[(IMPLEMENTATION, 2), (EXECUTION, 2)]);
        assert_eq!(
            shape(&turns),
            vec![named(IMPLEMENTATION, 2), named(EXECUTION, 2)],
            "a change with two turns each side is a phase boundary"
        );
    }

    #[test]
    fn a_single_deviating_turn_does_not_open_a_phase() {
        let deviation = chain(&[(IMPLEMENTATION, 4), (EXECUTION, 1), (IMPLEMENTATION, 4)]);
        assert_eq!(
            shape(&deviation),
            vec![named(IMPLEMENTATION, 9)],
            "one deviating turn neither opens a phase nor closes the one it interrupts"
        );
        let sustained = chain(&[(IMPLEMENTATION, 4), (EXECUTION, 2), (IMPLEMENTATION, 4)]);
        assert_eq!(
            shape(&sustained),
            vec![
                named(IMPLEMENTATION, 4),
                named(EXECUTION, 2),
                named(IMPLEMENTATION, 4)
            ],
            "the same shape with a second deviating turn does split, so the case above can fail"
        );
    }

    #[test]
    fn a_deviating_turn_keeps_its_own_archetype_in_the_phase_it_joins() {
        let turns = chain(&[(IMPLEMENTATION, 4), (EXECUTION, 1), (IMPLEMENTATION, 4)]);
        let rows = phases(&turns, turns.len());
        let [phase] = rows.as_slice() else {
            panic!("one deviating turn leaves one phase, saw {}", rows.len());
        };
        assert_eq!(phase.family, IMPLEMENTATION);
        assert_eq!(
            phase.examples[4].archetype, EXECUTION,
            "an example reports the turn's own activity, not the phase label"
        );
    }

    #[test]
    fn a_long_idle_gap_opens_a_phase_without_a_family_change() {
        let settled = chain(&[(IMPLEMENTATION, 2)]);
        let resumed = minute_of(&settled[1]) + 1;
        let mut idle = settled.clone();
        idle.extend([
            turn(resumed + 45, IMPLEMENTATION),
            turn(resumed + 50, IMPLEMENTATION),
        ]);
        assert_eq!(
            shape(&idle),
            vec![named(IMPLEMENTATION, 2), named(IMPLEMENTATION, 2)],
            "idle time alone ends a phase once it reaches the gap"
        );
        let mut brief = settled;
        brief.extend([
            turn(resumed + 44, IMPLEMENTATION),
            turn(resumed + 49, IMPLEMENTATION),
        ]);
        assert_eq!(
            shape(&brief),
            vec![named(IMPLEMENTATION, 4)],
            "one minute short of the gap is not a boundary"
        );
    }

    #[test]
    fn an_open_turn_is_measured_from_its_last_activity() {
        let turns = vec![
            open_turn("one", 0, 40, IMPLEMENTATION),
            turn(50, IMPLEMENTATION),
        ];
        assert_eq!(
            shape(&turns),
            vec![named(IMPLEMENTATION, 2)],
            "an unfinished turn idles from its last activity, not from its start"
        );
        assert_eq!(
            phases(&turns, 1)[0].start,
            at(0),
            "the phase starts where its first turn started"
        );
        let ended = vec![open_turn("one", 0, 40, IMPLEMENTATION)];
        assert_eq!(
            phases(&ended, 1)[0].end,
            at(40),
            "a phase ending on an open turn ends at that turn's last activity"
        );
    }

    #[test]
    fn interleaved_sources_segment_as_one_ordered_transcript() {
        let single = chain(&[(IMPLEMENTATION, 2), (EXECUTION, 2)]);
        let interleaved = single
            .iter()
            .enumerate()
            .map(|(index, turn)| TurnRecord {
                source: if index % 2 == 0 { "one" } else { "two" }.to_owned(),
                ..turn.clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shape(&interleaved),
            shape(&single),
            "which file a turn was read from does not move a boundary"
        );
    }

    #[test]
    fn a_tied_family_mix_is_labelled_by_first_appearance() {
        let opening = vec![turn(0, DIALOGUE), turn(5, EXECUTION)];
        assert_eq!(
            shape(&opening),
            vec![named(DIALOGUE, 2)],
            "a tie is settled by which family appeared first"
        );
        let reversed = vec![turn(0, EXECUTION), turn(5, DIALOGUE)];
        assert_eq!(
            shape(&reversed),
            vec![named(EXECUTION, 2)],
            "reversing the same two turns moves the label, so the rule is order and not chance"
        );
        assert_eq!(
            phases(&opening, 2),
            phases(&opening, 2),
            "the same turns project the same phases"
        );
    }
}
