//! One-pass bounded timeline projection over turn and compaction frontiers.

use std::collections::VecDeque;

use wire::Refusal;

use super::super::super::model::{Residue, SourceView, TimelineView};
use super::super::super::project::{Filters, timeline_compaction_view, timeline_turn_view};
use super::super::super::source::ResolvedSource;
use super::super::{
    FrontierDirection, ResidueLedger, ResultFrontier, SessionWork, SessionWorkBudget,
    admit_sources, source_view_text_bytes,
};
use super::{PhaseSource, merge_compactions};

pub(crate) struct TimelineScan {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) rows: Vec<TimelineView>,
    pub(crate) residues: Vec<Residue>,
}

impl TimelineScan {
    pub(crate) fn scan(
        sources: &[ResolvedSource],
        filters: &Filters,
        limit: usize,
        max_text: usize,
    ) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut residues = ResidueLedger::new();
        let mut streams = sources
            .iter()
            .map(|source| timeline_source(source, filters, limit, &mut work))
            .collect::<Result<Vec<_>, _>>()?;
        for stream in &mut streams {
            stream.advance(&mut residues, &mut work)?;
        }
        let mut turns = ResultFrontier::new(limit, FrontierDirection::Newest)?;
        let mut any_turn = false;
        let mut any_turn_identity = false;
        while let Some(index) = next_turn_source(&streams) {
            let turn = streams[index]
                .ready
                .pop_front()
                .expect("selected timeline frontier is nonempty");
            any_turn = true;
            any_turn_identity |= turn.turn_id.is_some();
            if filters.turn_matches(&turn) {
                turns.push(timeline_turn_view(&turn, max_text));
            }
            streams[index].advance(&mut residues, &mut work)?;
        }
        if any_turn {
            filters.validate_turn_identity_available(any_turn_identity)?;
        }
        let mut source_views = Vec::with_capacity(streams.len());
        let mut compaction_sources = Vec::with_capacity(streams.len());
        for stream in streams {
            let (source, compactions) = stream.into_window(&mut work)?;
            work.admit_retained_text(source_view_text_bytes(&source), &source.path)?;
            source_views.push(source);
            compaction_sources.push(VecDeque::from(compactions));
        }
        let compactions = merge_compactions(compaction_sources, limit, FrontierDirection::Newest)?;
        let mut turns = VecDeque::from(turns.into_rows());
        let mut compactions = compactions
            .into_iter()
            .map(|row| timeline_compaction_view(row, max_text))
            .collect::<VecDeque<_>>();
        let mut rows = ResultFrontier::new(limit, FrontierDirection::Newest)?;
        while let Some(row) = next_row(&mut turns, &mut compactions) {
            rows.push(row);
        }
        Ok(Self {
            sources: source_views,
            rows: rows.into_rows(),
            residues: residues.finish(&mut work, "timeline residue summary")?,
        })
    }
}

fn timeline_source<'a>(
    source: &'a ResolvedSource,
    filters: &Filters,
    limit: usize,
    work: &mut SessionWork,
) -> Result<PhaseSource<'a>, Refusal> {
    let mut stream = PhaseSource::window(
        source,
        filters.start_bound(),
        filters.start_bound(),
        filters.end_bound(),
        limit,
        FrontierDirection::Newest,
        work,
    )?;
    stream
        .compactions
        .as_mut()
        .expect("timeline source retains compaction state")
        .filter_with(filters.clone());
    Ok(stream)
}

fn next_turn_source(streams: &[PhaseSource<'_>]) -> Option<usize> {
    streams
        .iter()
        .enumerate()
        .filter(|(_, stream)| !stream.ready.is_empty())
        .min_by_key(|(_, stream)| {
            let turn = stream
                .ready
                .front()
                .expect("filtered timeline frontier is nonempty");
            (&turn.start_ts, &turn.source)
        })
        .map(|(index, _)| index)
}

fn next_row(
    turns: &mut VecDeque<TimelineView>,
    compactions: &mut VecDeque<TimelineView>,
) -> Option<TimelineView> {
    let take_turn = match (turns.front(), compactions.front()) {
        (Some(turn), Some(compaction)) => {
            (&turn.ts, &turn.kind) <= (&compaction.ts, &compaction.kind)
        }
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => return None,
    };
    if take_turn {
        turns.pop_front()
    } else {
        compactions.pop_front()
    }
}
