use clap::ArgMatches;
use wire::Refusal;

use super::super::dataset::aggregate::timeline::TimelineScan;
use super::super::model::{CompactionRecord, Report, TimelineView, TurnRecord};
use super::{Filters, turn_timeline_text};
use crate::read_page::{self as page, Page};

/// One page of turns and compactions, newest first.
pub(crate) fn report(
    sources: &[super::ResolvedSource],
    matches: &ArgMatches,
    page: Page,
) -> Result<Report, Refusal> {
    let filters = Filters::from_matches(matches)?;
    let end = page.end(filters.end_bound())?;
    let data = TimelineScan::scan(
        sources,
        &filters.ending(end),
        page.frontier()?,
        page.limits().text,
    )?;
    let rows = data.rows.into_iter().rev().collect();
    let paged = page.newest(rows, data.population, |row| &row.ts)?;
    Ok(super::report(paged, |page| Report::Timeline {
        sources: data.sources,
        page,
        residues: data.residues,
    }))
}

pub(crate) fn turn_view(turn: &TurnRecord, max_text: usize) -> TimelineView {
    TimelineView {
        source: turn.source.clone(),
        ts: turn.start_ts.clone(),
        kind: "turn".to_owned(),
        turn_id: turn.turn_id.clone(),
        text: page::clip(&turn_timeline_text(turn), max_text),
    }
}

pub(crate) fn compaction_view(row: CompactionRecord, max_text: usize) -> TimelineView {
    TimelineView {
        source: row.source,
        ts: row.ts,
        kind: "compaction".to_owned(),
        turn_id: None,
        text: page::clip(
            &format!(
                "assistant_before={} summary_after={} intent={} user_after={}",
                row.assistant_before, row.summary_after, row.intent, row.user_after
            ),
            max_text,
        ),
    }
}
