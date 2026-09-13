use clap::ArgMatches;
use wire::Refusal;

use super::super::dataset::{aggregate::timeline::TimelineScan, frontier_limit};
use super::super::model::{CompactionRecord, Report, TimelineView, TurnRecord};
use super::{DEFAULT_TIMELINE_TEXT, clip, number, turn_timeline_text};

const DEFAULT_LIMIT: usize = 50;

pub(crate) fn options(matches: &ArgMatches) -> Result<(usize, usize), Refusal> {
    let limit = number(matches, "limit", DEFAULT_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_TIMELINE_TEXT)?;
    frontier_limit(limit)?;
    Ok((limit, max_text))
}

pub(crate) fn report(dataset: TimelineScan) -> Report {
    Report::Timeline {
        sources: dataset.sources,
        rows: dataset.rows,
        residues: dataset.residues,
    }
}

pub(crate) fn turn_view(turn: &TurnRecord, max_text: usize) -> TimelineView {
    TimelineView {
        source: turn.source.clone(),
        ts: turn.start_ts.clone(),
        kind: "turn".to_owned(),
        turn_id: turn.turn_id.clone(),
        text: clip(&turn_timeline_text(turn), max_text),
    }
}

pub(crate) fn compaction_view(row: CompactionRecord, max_text: usize) -> TimelineView {
    TimelineView {
        source: row.source,
        ts: row.ts,
        kind: "compaction".to_owned(),
        turn_id: None,
        text: clip(
            &format!(
                "assistant_before={} summary_after={} intent={} user_after={}",
                row.assistant_before, row.summary_after, row.intent, row.user_after
            ),
            max_text,
        ),
    }
}
