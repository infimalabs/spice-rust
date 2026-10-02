//! Bounded chronological commit projection over transcript turn streams.

use std::collections::BTreeSet;

use wire::Refusal;

use super::super::super::model::{CommitView, Residue, SourceView, TurnRecord};
use super::super::super::page::clip;
use super::super::super::source::ResolvedSource;
use super::super::{
    FrontierDirection, ResidueLedger, ResultFrontier, SessionWork, SessionWorkBudget,
    admit_sources, refusal, source_view_text_bytes,
};
use super::PhaseSource;

const MAX_COMMIT_IDENTITIES: usize = 4 * 1_024;

pub(crate) struct CommitScan {
    pub(crate) sources: Vec<SourceView>,
    /// Newest first-mentioned commits, oldest first.
    pub(crate) rows: Vec<CommitView>,
    /// Every first mention at or before the scan end, retained or not.
    pub(crate) population: usize,
    pub(crate) residues: Vec<Residue>,
}

impl CommitScan {
    /// First mentions of each commit in turns starting at or before `end`.
    pub(crate) fn scan(
        sources: &[ResolvedSource],
        end: Option<&str>,
        limit: usize,
        max_text: usize,
    ) -> Result<Self, Refusal> {
        admit_sources(sources)?;
        let mut work = SessionWork::new(SessionWorkBudget::report());
        let mut residues = ResidueLedger::new();
        let mut streams = sources
            .iter()
            .map(|source| PhaseSource::new(source, &mut work))
            .collect::<Result<Vec<_>, _>>()?;
        for stream in &mut streams {
            stream.advance(&mut residues, &mut work)?;
        }
        let mut rows = ResultFrontier::new(limit, FrontierDirection::Newest)?;
        let mut seen = BTreeSet::new();
        while let Some(index) = streams
            .iter()
            .enumerate()
            .filter(|(_, stream)| !stream.ready.is_empty())
            .min_by_key(|(_, stream)| {
                let turn = stream
                    .ready
                    .front()
                    .expect("filtered commit frontier is nonempty");
                (&turn.start_ts, &turn.source)
            })
            .map(|(index, _)| index)
        {
            let turn = streams[index]
                .ready
                .pop_front()
                .expect("selected commit frontier is nonempty");
            if end.is_some_and(|end| turn.start_ts.as_str() > end) {
                break;
            }
            visit_commit_views(&turn, max_text, |row| {
                if seen.contains(&row.sha) {
                    return Ok(());
                }
                if seen.len() == MAX_COMMIT_IDENTITIES {
                    return Err(refusal(
                        "session::commit-identity-limit",
                        Some(format!("maximum={MAX_COMMIT_IDENTITIES}")),
                        "narrow the immutable transcript inputs before requesting commits",
                    ));
                }
                seen.insert(row.sha.clone());
                rows.push(row);
                Ok(())
            })?;
            streams[index].advance(&mut residues, &mut work)?;
        }
        let mut source_views = Vec::with_capacity(streams.len());
        for stream in streams {
            let source = stream.source_view();
            work.admit_retained_text(source_view_text_bytes(&source), &source.path)?;
            source_views.push(source);
        }
        Ok(Self {
            sources: source_views,
            population: rows.offered(),
            rows: rows.into_rows(),
            residues: residues.finish(&mut work, "commit residue summary")?,
        })
    }
}

fn visit_commit_views(
    turn: &TurnRecord,
    max_text: usize,
    mut visit: impl FnMut(CommitView) -> Result<(), Refusal>,
) -> Result<(), Refusal> {
    let user = clip(
        turn.messages
            .iter()
            .find(|message| message.side == "user" && message.authored.is_some())
            .and_then(|message| message.authored.as_deref())
            .unwrap_or_default(),
        max_text,
    );
    let final_message = turn
        .messages
        .iter()
        .rfind(|message| message.side == "assistant" && message.phase == "final_answer")
        .into_iter();
    let nonfinal_messages = turn
        .messages
        .iter()
        .rev()
        .filter(|message| message.side == "assistant" && message.phase != "final_answer");
    for message in final_message.chain(nonfinal_messages) {
        let Some(authored) = message.authored.as_deref() else {
            continue;
        };
        for line in authored.lines() {
            if !commit_line(line) {
                continue;
            }
            for sha in commit_tokens(line) {
                if sha.bytes().all(|byte| byte.is_ascii_digit()) {
                    continue;
                }
                visit(CommitView {
                    source: turn.source.clone(),
                    ts: turn.start_ts.clone(),
                    turn_id: turn.turn_id.clone(),
                    sha,
                    line: clip(line, max_text),
                    user: user.clone(),
                })?;
            }
        }
    }
    Ok(())
}

fn commit_tokens(line: &str) -> impl Iterator<Item = String> + '_ {
    line.split_whitespace()
        .map(|word| word.trim_matches(|character: char| !character.is_ascii_hexdigit()))
        .filter(|word| {
            (7..=40).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .map(str::to_ascii_lowercase)
}

fn commit_line(line: &str) -> bool {
    line.split(|character: char| !character.is_alphanumeric())
        .any(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "commit" | "committed" | "sha"
            )
        })
}
