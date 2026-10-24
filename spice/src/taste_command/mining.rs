//! Bounded recurrence mining from durable operator steering evidence.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use wire::body::{SteerAcked, SteerDeclined, SteerSent, TaskAdded, TranscriptLocus};
use wire::intent::{CommandAnswer, TasteProposalAdd};
use wire::{
    CanonicalUuid, Digest, Domain, FaultClass, Origin, Outcome, Reply, ReplyPosition, Seq,
    TaskHandle,
};

use super::{
    Contract, Surface, current_repository, fault, load_failure, locus_text, refusal, retag,
};

const MAX_AUTHORED_CORRECTIONS: usize = 1_024;
const MAX_EXCERPT_CHARS: usize = 1_000;
const MAX_TRIGGERS: usize = 4;

/// Result of one deterministic mining pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TasteMiningReport {
    threshold: usize,
    scanned: usize,
    excluded: usize,
    qualifying: usize,
    findings: Vec<MinedFinding>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct MinedFinding {
    finding: Digest,
    recurrence: usize,
    triggers: Vec<String>,
    message: String,
    task: TaskHandle,
    standing: ProposalStanding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ProposalStanding {
    Filed,
    ReusedActive,
    ReusedCompleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Correction {
    seq: Seq,
    directive: CanonicalUuid,
    fanout: Option<CanonicalUuid>,
    key: String,
    excerpt: String,
    tokens: BTreeSet<String>,
    lane: CanonicalUuid,
    locus: TranscriptLocus,
    disposition: &'static str,
    decline_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct Candidate {
    finding: Digest,
    occurrence: Digest,
    corrections: Vec<Correction>,
    triggers: Vec<String>,
    message: String,
}

#[derive(Clone, Debug)]
struct PriorProposal {
    handle: TaskHandle,
    occurrence: Digest,
    recurrence: usize,
    completed: bool,
}

enum EvidenceError {
    AnswerWithoutSent(CanonicalUuid),
    AnswerMismatch(CanonicalUuid),
    DuplicateAnswer(CanonicalUuid),
    TooMany(usize),
    ProposalFinding(TaskHandle),
    ProposalStanding(TaskHandle),
    ProposalEvidence(TaskHandle),
}

struct MiningSnapshot {
    repository: spice::repository::Repository,
    threshold: usize,
    facts: journal::FactSnapshot,
    cursor: Seq,
}

/// Mine recurring corrections and file only hidden, operator-reviewed work.
pub(crate) fn mine(
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Outcome<TasteMiningReport> {
    let MiningSnapshot {
        repository,
        threshold,
        facts,
        cursor,
    } = match mining_snapshot(contract, configuration) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let (corrections, scanned, excluded) = match corrections(&facts) {
        Ok(result) => result,
        Err(error) => return evidence_fault(error),
    };
    let candidates = cluster(&corrections, threshold);
    let tasks = facts
        .facts()
        .iter()
        .filter_map(|envelope| {
            let added = envelope.fact.read::<TaskAdded>()?;
            added
                .project
                .starts_with(".maxim_proposal.taste.")
                .then_some(added.task)
        })
        .collect();
    let board = match crate::board_command::current_board(
        &repository,
        wire::intent::BoardRead::Tasks {
            tasks,
            dependencies: false,
            claims: false,
            history: false,
        },
    ) {
        Ok(board) => board,
        Err(outcome) => return retag(outcome),
    };
    let priors = match prior_proposals(&facts, &board) {
        Ok(priors) => priors,
        Err(error) => return evidence_fault(error),
    };
    let filed = file_candidates(
        &repository,
        contract,
        threshold,
        cursor,
        &candidates,
        &priors,
    );
    let Reply {
        position,
        body: findings,
    } = match filed {
        Outcome::Reply(reply) => reply,
        Outcome::Refusal(refusal) => return Outcome::Refusal(refusal),
        Outcome::Fault(fault) => return Outcome::Fault(fault),
    };
    Outcome::Reply(Reply {
        position,
        body: TasteMiningReport {
            threshold,
            scanned,
            excluded,
            qualifying: candidates.len(),
            findings,
        },
    })
}

fn mining_snapshot(
    contract: &Contract,
    configuration: Option<&spice::config::RepositoryCapture>,
) -> Result<MiningSnapshot, Outcome<TasteMiningReport>> {
    let repository = current_repository()?;
    let surface = match Surface::scoped(&repository, configuration) {
        Ok(surface) => surface,
        Err(error) => return Err(load_failure(&error)),
    };
    let Some(threshold) = surface.mining_threshold else {
        return Err(refusal(
            "taste::mining-disabled",
            Some("policy.taste_mining_threshold >= 2".to_owned()),
            None,
            "declare a tracked recurrence threshold before mining operator corrections",
        ));
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return Err(crate::state_failure(&error).outcome()),
    };
    if let Err(outcome) = crate::command_client::authenticate(state.serve(), contract) {
        return Err(retag(outcome));
    }
    let reader = match journal::JournalReader::open(state.journal()) {
        Ok(reader) => reader,
        Err(error) => {
            return Err(fault(
                FaultClass::Store,
                "taste::journal-unreadable",
                error.to_string(),
            ));
        }
    };
    let facts = match reader.fact_snapshot() {
        Ok(facts) => facts,
        Err(error) => {
            return Err(fault(
                FaultClass::Store,
                "taste::journal-unreadable",
                error.to_string(),
            ));
        }
    };
    let Some(cursor) = facts.horizon() else {
        return Err(fault(
            FaultClass::Store,
            "taste::cursor-missing",
            "the authenticated resident has no durable journal position".to_owned(),
        ));
    };
    Ok(MiningSnapshot {
        repository,
        threshold,
        facts,
        cursor,
    })
}

/// File each qualifying candidate not already proposed, answering every
/// candidate's finding at the position of the last filing.
fn file_candidates(
    repository: &spice::repository::Repository,
    contract: &Contract,
    threshold: usize,
    cursor: Seq,
    candidates: &[Candidate],
    priors: &BTreeMap<Digest, Vec<PriorProposal>>,
) -> Outcome<Vec<MinedFinding>> {
    let mut findings = Vec::new();
    let mut position = ReplyPosition::Snapshot(cursor);
    for candidate in candidates {
        let (standing, handle, predecessor) = plan(candidate, priors);
        if standing != ProposalStanding::Filed {
            findings.push(report_finding(
                candidate,
                handle.expect("a reused proposal carries its projected handle"),
                standing,
            ));
            continue;
        }
        let oldest = candidate
            .corrections
            .first()
            .expect("qualifying candidate carries evidence");
        let origin = format!("ack:{}", oldest.key)
            .parse::<Origin>()
            .expect("an ACK key is a nonempty origin value");
        let body = TasteProposalAdd {
            finding: candidate.finding.clone(),
            occurrence: candidate.occurrence.clone(),
            recurrence: candidate.corrections.len(),
            title: format!("Review mined taste: {}", headline(&candidate.message)),
            description: description(candidate, threshold, predecessor.as_ref()),
            acceptance: acceptance(candidate),
            origin,
            depends: predecessor.into_iter().collect(),
        };
        match crate::board_command::submit_taste_proposal(repository, contract, body) {
            Outcome::Reply(reply) => {
                position = reply.position;
                let CommandAnswer::Task(answer) = reply.body else {
                    return fault(
                        FaultClass::Protocol,
                        "taste::proposal-answer",
                        "taste proposal command returned a non-task answer".to_owned(),
                    );
                };
                findings.push(report_finding(
                    candidate,
                    answer.task,
                    match answer.status.as_str() {
                        "reused-active" => ProposalStanding::ReusedActive,
                        "reused-completed" => ProposalStanding::ReusedCompleted,
                        _ => ProposalStanding::Filed,
                    },
                ));
            }
            Outcome::Refusal(refusal) => return Outcome::Refusal(refusal),
            Outcome::Fault(fault) => return Outcome::Fault(fault),
        }
    }
    Outcome::Reply(Reply {
        position,
        body: findings,
    })
}

fn corrections(
    snapshot: &journal::FactSnapshot,
) -> Result<(Vec<Correction>, usize, usize), EvidenceError> {
    let mut sent = BTreeMap::<CanonicalUuid, (Seq, bool, SteerSent)>::new();
    for envelope in snapshot.facts() {
        let opened = envelope
            .fact
            .read::<SteerSent>()
            .or_else(|| {
                envelope
                    .fact
                    .read::<wire::body::SteerMessaged>()
                    .map(|body| body.as_sent())
            })
            .or_else(|| {
                envelope
                    .fact
                    .read::<wire::body::SteerQueued>()
                    .map(|body| body.as_sent())
            });
        if let Some(body) = opened {
            sent.entry(body.directive).or_insert((
                envelope.seq,
                wire::principal::ActingPrincipal::of(&envelope.actor).is_some(),
                body,
            ));
        }
    }
    let scanned = sent.values().filter(|(_, authored, _)| *authored).count();
    if scanned > MAX_AUTHORED_CORRECTIONS {
        return Err(EvidenceError::TooMany(scanned));
    }
    let mut answered = BTreeSet::new();
    let mut rows = Vec::new();
    for envelope in snapshot.facts() {
        let answer = if let Some(body) = envelope.fact.read::<SteerAcked>() {
            Some((
                body.directive,
                body.key,
                body.lane,
                body.locus,
                "acked",
                None,
            ))
        } else if let Some(body) = envelope.fact.read::<SteerDeclined>() {
            Some((
                body.directive,
                body.key,
                body.lane,
                body.locus,
                "declined",
                Some(body.reason),
            ))
        } else {
            None
        };
        let Some((directive, key, lane, locus, disposition, decline_reason)) = answer else {
            continue;
        };
        let Some((seq, authored, published)) = sent.get(&directive) else {
            return Err(EvidenceError::AnswerWithoutSent(directive));
        };
        if published.key != key || published.lane != lane {
            return Err(EvidenceError::AnswerMismatch(directive));
        }
        if !answered.insert(directive) {
            return Err(EvidenceError::DuplicateAnswer(directive));
        }
        if !authored {
            continue;
        }
        if decline_reason.as_ref().is_some_and(|reason| {
            reason.chars().count() > MAX_EXCERPT_CHARS || super::contains_sensitive_text(reason)
        }) {
            continue;
        }
        let Some((excerpt, tokens)) = authored_correction(published) else {
            continue;
        };
        rows.push(Correction {
            seq: *seq,
            directive,
            fanout: published.fanout,
            key,
            excerpt,
            tokens,
            lane,
            locus,
            disposition,
            decline_reason,
        });
    }
    rows.sort_by_key(|row| row.seq);
    let excluded = scanned.saturating_sub(rows.len());
    Ok((rows, scanned, excluded))
}

fn authored_correction(sent: &SteerSent) -> Option<(String, BTreeSet<String>)> {
    if sent.control.is_some() || !sent.attachments.is_empty() {
        return None;
    }
    let lower = sent.text.to_ascii_lowercase();
    if lower.len() > MAX_EXCERPT_CHARS * 4
        || excluded_text(&lower)
        || lower.trim_start().starts_with('🌶')
    {
        return None;
    }
    let excerpt = sent
        .text
        .lines()
        .find(|line| correction_cue(&line.trim().to_ascii_lowercase()))?;
    if excerpt.trim().is_empty() || excerpt.chars().count() > MAX_EXCERPT_CHARS {
        return None;
    }
    let tokens = tokens(excerpt);
    (tokens.len() >= 2).then(|| (excerpt.to_owned(), tokens))
}

fn excluded_text(lower: &str) -> bool {
    const EXCLUDED: [&str; 21] = [
        "diff --git",
        "```",
        "tool output",
        "tool result",
        "begin patch",
        "maybe",
        "perhaps",
        "might want",
        "i wonder",
        "could you consider",
        "status update",
        "what is the status",
        "what's the status",
        "how is it going",
        "how's it going",
        "continue working",
        "keep going",
        "proceed",
        "maxim proposal",
        "last maxim",
        "working state:",
    ];
    super::contains_sensitive_text(lower) || EXCLUDED.iter().any(|needle| lower.contains(needle))
}

fn correction_cue(lower: &str) -> bool {
    const CUES: [&str; 14] = [
        "do not ",
        "don't ",
        "never ",
        "must ",
        "stop ",
        " instead",
        "rather than",
        "prefer ",
        "avoid ",
        "make sure ",
        "always ",
        "should not ",
        "shouldn't ",
        "use ",
    ];
    CUES.iter().any(|cue| lower.contains(cue))
}

fn tokens(text: &str) -> BTreeSet<String> {
    const STOP: [&str; 33] = [
        "a", "always", "an", "and", "are", "as", "avoid", "be", "do", "don't", "for", "from",
        "here", "in", "instead", "is", "it", "make", "must", "never", "not", "of", "on", "or",
        "please", "prefer", "rather", "should", "stop", "than", "the", "to", "use",
    ];
    text.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .filter_map(|word| {
            let word = word.to_ascii_lowercase();
            (word.len() >= 2 && !STOP.contains(&word.as_str())).then_some(word)
        })
        .collect()
}

fn cluster(corrections: &[Correction], threshold: usize) -> Vec<Candidate> {
    let mut independent = corrections.to_vec();
    independent.sort_by_key(|row| row.seq);
    let mut seen = BTreeSet::new();
    independent.retain(|row| seen.insert(occurrence_identity(row)));

    let mut by_theme = BTreeMap::<(String, String), Vec<Correction>>::new();
    for correction in independent {
        let words = correction.tokens.iter().collect::<Vec<_>>();
        for left in 0..words.len() {
            for right in left + 1..words.len() {
                by_theme
                    .entry((words[left].clone(), words[right].clone()))
                    .or_default()
                    .push(correction.clone());
            }
        }
    }

    // Several token pairs can describe the same near-duplicate evidence set.
    // Collapse that set once and retain the lexically first shared pair as its
    // stable finding identity. Unlike transitive union, every retained row
    // directly contains both theme tokens.
    let mut groups = BTreeMap::<Vec<String>, ((String, String), Vec<Correction>)>::new();
    for (theme, mut rows) in by_theme {
        if rows.len() < threshold {
            continue;
        }
        rows.sort_by_key(|row| row.seq);
        let loci = rows.iter().map(occurrence_identity).collect::<Vec<_>>();
        groups.entry(loci).or_insert((theme, rows));
    }
    let mut candidates = groups
        .into_values()
        .filter_map(|(theme, rows)| {
            let candidate = candidate(rows, theme);
            (!candidate.triggers.is_empty()).then_some(candidate)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.finding.cmp(&right.finding));
    candidates
}

fn candidate(corrections: Vec<Correction>, theme: (String, String)) -> Candidate {
    let oldest = corrections.first().expect("candidate has corrections");
    let fingerprint = format!("{}\0{}", theme.0, theme.1);
    let finding = Digest::compute(Domain::TasteProse, fingerprint.as_bytes());
    let mut loci = corrections
        .iter()
        .map(occurrence_identity)
        .collect::<Vec<_>>();
    loci.sort();
    let occurrence_seed = format!("{}\0{}", finding, loci.join("\0"));
    let occurrence = Digest::compute(Domain::BoardTaskIdentity, occurrence_seed.as_bytes());
    let triggers = frequent_tokens(&corrections);
    let message = oldest.excerpt.clone();
    Candidate {
        finding,
        occurrence,
        corrections,
        triggers,
        message,
    }
}

fn frequent_tokens(corrections: &[Correction]) -> Vec<String> {
    let mut frequencies = BTreeMap::<String, usize>::new();
    for correction in corrections {
        for token in &correction.tokens {
            *frequencies.entry(token.clone()).or_default() += 1;
        }
    }
    let mut ranked = frequencies
        .into_iter()
        .filter(|(token, count)| token.len() >= 3 && *count > 1)
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_token, left_count), (right_token, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_token.cmp(right_token))
    });
    ranked
        .into_iter()
        .take(MAX_TRIGGERS)
        .map(|(token, _)| token)
        .collect()
}

fn occurrence_identity(correction: &Correction) -> String {
    if let Some(fanout) = correction.fanout {
        return format!("fanout:{fanout}");
    }
    let source = correction
        .locus
        .source
        .map_or_else(|| "absent".to_owned(), |source| source.to_string());
    format!(
        "pre-fanout:lane:{}:source:{source}:{}:{}:{}:{}:{}:{}",
        correction.lane,
        correction.locus.member,
        correction.locus.offset,
        correction.locus.length,
        correction.locus.event,
        correction.locus.line,
        correction.locus.column,
    )
}

fn prior_proposals(
    facts: &journal::FactSnapshot,
    board: &wire::intent::BoardAnswer,
) -> Result<BTreeMap<Digest, Vec<PriorProposal>>, EvidenceError> {
    let completion = board
        .tasks
        .iter()
        .map(|task| {
            (
                task.statement.body.born.clone(),
                (
                    task.statement.body.handle.clone(),
                    !task.workflow.body.status.is_pending(),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut proposals = BTreeMap::<Digest, Vec<PriorProposal>>::new();
    for envelope in facts.facts() {
        let Some(added) = envelope.fact.read::<TaskAdded>() else {
            continue;
        };
        let Some(finding_text) = added.project.strip_prefix(".maxim_proposal.taste.") else {
            continue;
        };
        let fallback = TaskHandle::for_project(&added.project, &added.task);
        let finding = finding_text
            .parse::<Digest>()
            .map_err(|_| EvidenceError::ProposalFinding(fallback.clone()))?;
        let (handle, completed) = completion
            .get(&added.task)
            .cloned()
            .ok_or_else(|| EvidenceError::ProposalStanding(fallback.clone()))?;
        let proposal = added
            .taste_proposal
            .ok_or_else(|| EvidenceError::ProposalEvidence(handle.clone()))?;
        proposals.entry(finding).or_default().push(PriorProposal {
            completed,
            handle,
            occurrence: proposal.occurrence,
            recurrence: proposal.recurrence,
        });
    }
    Ok(proposals)
}

fn plan(
    candidate: &Candidate,
    priors: &BTreeMap<Digest, Vec<PriorProposal>>,
) -> (ProposalStanding, Option<TaskHandle>, Option<TaskHandle>) {
    let Some(priors) = priors.get(&candidate.finding) else {
        return (ProposalStanding::Filed, None, None);
    };
    if let Some(prior) = priors
        .iter()
        .find(|prior| prior.occurrence == candidate.occurrence)
    {
        return (
            if prior.completed {
                ProposalStanding::ReusedCompleted
            } else {
                ProposalStanding::ReusedActive
            },
            Some(prior.handle.clone()),
            None,
        );
    }
    if let Some(prior) = priors.iter().find(|prior| !prior.completed) {
        return (
            ProposalStanding::ReusedActive,
            Some(prior.handle.clone()),
            None,
        );
    }
    let predecessor = priors.iter().max_by(|left, right| {
        left.recurrence
            .cmp(&right.recurrence)
            .then_with(|| left.handle.cmp(&right.handle))
    });
    if let Some(prior) = predecessor
        && prior.recurrence >= candidate.corrections.len()
    {
        return (
            ProposalStanding::ReusedCompleted,
            Some(prior.handle.clone()),
            None,
        );
    }
    (
        ProposalStanding::Filed,
        None,
        predecessor.map(|p| p.handle.clone()),
    )
}

fn description(
    candidate: &Candidate,
    threshold: usize,
    predecessor: Option<&TaskHandle>,
) -> String {
    let triggers = serde_json::to_string(&candidate.triggers).expect("triggers encode");
    let message = serde_json::to_string(&candidate.message).expect("message encodes");
    let mut description = format!(
        concat!(
            "Finding identity: {}\n",
            "Occurrence identity: {}\n",
            "Recurrence count: {}\n",
            "Configured threshold: {}\n",
            "Proposed triggers: {}\n",
            "Proposed message: {}\n",
            "Predecessor: {}\n\n",
            "Exact authored correction evidence:\n"
        ),
        candidate.finding,
        candidate.occurrence,
        candidate.corrections.len(),
        threshold,
        triggers,
        message,
        predecessor.map_or("none", TaskHandle::as_str),
    );
    for correction in &candidate.corrections {
        let fanout = correction
            .fanout
            .map_or_else(|| "pre-fanout".to_owned(), |fanout| fanout.to_string());
        description.push_str(&format!(
            "- directive {} fanout {} key {} ({}) at {}: {:?}",
            correction.directive,
            fanout,
            correction.key,
            correction.disposition,
            locus_text(&correction.locus),
            correction.excerpt,
        ));
        if let Some(reason) = &correction.decline_reason {
            description.push_str(&format!("; NAK reason: {reason:?}"));
        }
        description.push('\n');
    }
    description
}

fn acceptance(candidate: &Candidate) -> Vec<String> {
    let triggers = serde_json::to_string(&candidate.triggers).expect("triggers encode");
    let message = serde_json::to_string(&candidate.message).expect("message encodes");
    vec![
        format!(
            "Operator review admits, revises, or rejects one tracked taste row grounded in the cited excerpts; proposed words = {triggers}, proposed message = {message}."
        ),
        "Positive behavior and literal-mention fixtures prove the reviewed triggers and message without treating mention as behavior.".to_owned(),
        "Non-hit fixtures cover generated diffs and patches, tool output, maxim self-echo, secrets, speculation, ordinary status, and a below-threshold correction.".to_owned(),
    ]
}

fn report_finding(
    candidate: &Candidate,
    task: TaskHandle,
    standing: ProposalStanding,
) -> MinedFinding {
    MinedFinding {
        finding: candidate.finding.clone(),
        recurrence: candidate.corrections.len(),
        triggers: candidate.triggers.clone(),
        message: candidate.message.clone(),
        task,
        standing,
    }
}

fn headline(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let prefix = chars.by_ref().take(72).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn evidence_fault(error: EvidenceError) -> Outcome<TasteMiningReport> {
    let detail = match error {
        EvidenceError::AnswerWithoutSent(directive) => {
            format!("ACK/NAK references unavailable operator steering directive {directive}")
        }
        EvidenceError::AnswerMismatch(directive) => {
            format!("ACK/NAK key or lane contradicts steering directive {directive}")
        }
        EvidenceError::DuplicateAnswer(directive) => {
            format!("steering directive {directive} has more than one ACK/NAK disposition")
        }
        EvidenceError::TooMany(count) => format!(
            "{count} operator steering directives exceed the bounded mining limit {MAX_AUTHORED_CORRECTIONS}"
        ),
        EvidenceError::ProposalFinding(task) => {
            format!("hidden taste proposal {task} carries an invalid finding route")
        }
        EvidenceError::ProposalStanding(task) => {
            format!("hidden taste proposal {task} has no current workflow standing")
        }
        EvidenceError::ProposalEvidence(task) => {
            format!("hidden taste proposal {task} carries no proposal evidence")
        }
    };
    fault(FaultClass::Store, "taste::mining-evidence-invalid", detail)
}

#[cfg(test)]
mod tests;
