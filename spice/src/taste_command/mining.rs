//! Bounded recurrence mining from durable operator steering evidence.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use wire::body::{SteerAcked, SteerDeclined, SteerSent, TaskAdded, TranscriptLocus};
use wire::intent::{CommandAnswer, TasteProposalAdd};
use wire::{
    Actor, CanonicalUuid, Digest, Domain, FaultClass, Origin, Outcome, Reply, ReplyPosition, Seq,
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
    key: String,
    lane: CanonicalUuid,
    excerpt: String,
    tokens: BTreeSet<String>,
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
    ProposalDescription(TaskHandle),
}

struct MiningSnapshot {
    repository: spice::repository::Repository,
    threshold: usize,
    facts: journal::FactSnapshot,
    cursor: Seq,
}

/// Mine recurring corrections and file only hidden, operator-reviewed work.
pub(crate) fn mine(contract: &Contract) -> Outcome<TasteMiningReport> {
    let MiningSnapshot {
        repository,
        threshold,
        facts,
        cursor,
    } = match mining_snapshot(contract) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let (corrections, scanned, excluded) = match corrections(&facts) {
        Ok(result) => result,
        Err(error) => return evidence_fault(error),
    };
    let candidates = cluster(&corrections, threshold);
    let board = match crate::board_command::current_board(&repository) {
        Ok(board) => board,
        Err(outcome) => return retag(outcome),
    };
    let priors = match prior_proposals(&facts, &board) {
        Ok(priors) => priors,
        Err(error) => return evidence_fault(error),
    };
    file_candidates(
        &repository,
        contract,
        threshold,
        scanned,
        excluded,
        cursor,
        &candidates,
        &board,
        &priors,
    )
}

fn mining_snapshot(contract: &Contract) -> Result<MiningSnapshot, Outcome<TasteMiningReport>> {
    let repository = current_repository()?;
    let surface = match Surface::load(&repository) {
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
    if let Err(outcome) = crate::command_client::authenticate(state.canvas(), contract) {
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

#[allow(clippy::too_many_arguments)]
fn file_candidates(
    repository: &spice::repository::Repository,
    contract: &Contract,
    threshold: usize,
    scanned: usize,
    excluded: usize,
    cursor: Seq,
    candidates: &[Candidate],
    board: &wire::intent::BoardAnswer,
    priors: &BTreeMap<Digest, Vec<PriorProposal>>,
) -> Outcome<TasteMiningReport> {
    let mut findings = Vec::new();
    let mut position = ReplyPosition::Snapshot(cursor);
    for candidate in candidates {
        let expected = crate::board_command::taste_proposal_handle(&candidate.occurrence);
        if let Some(existing) = board
            .tasks
            .iter()
            .find(|task| task.statement.body.handle == expected)
        {
            let standing = if existing.workflow.body.completion.is_some() {
                ProposalStanding::ReusedCompleted
            } else {
                ProposalStanding::ReusedActive
            };
            findings.push(report_finding(candidate, expected, standing));
            continue;
        }
        let (handle, standing, predecessor) = plan(candidate, priors);
        if standing != ProposalStanding::Filed {
            findings.push(report_finding(candidate, handle, standing));
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
                    ProposalStanding::Filed,
                ));
            }
            Outcome::Refusal(refusal) => return Outcome::Refusal(refusal),
            Outcome::Fault(fault) => return Outcome::Fault(fault),
        }
    }
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

fn corrections(
    snapshot: &journal::FactSnapshot,
) -> Result<(Vec<Correction>, usize, usize), EvidenceError> {
    let mut sent = BTreeMap::<CanonicalUuid, (Seq, bool, SteerSent)>::new();
    for envelope in snapshot.facts() {
        if let Some(body) = envelope.fact.read::<SteerSent>() {
            sent.entry(body.directive).or_insert((
                envelope.seq,
                envelope.actor == Actor::Operator,
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
            reason.chars().count() > MAX_EXCERPT_CHARS
                || contains_sensitive(&reason.to_ascii_lowercase())
        }) {
            continue;
        }
        let Some((excerpt, tokens)) = authored_correction(published) else {
            continue;
        };
        rows.push(Correction {
            seq: *seq,
            directive,
            key,
            lane,
            excerpt,
            tokens,
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

fn contains_sensitive(lower: &str) -> bool {
    [
        "api_key", "api key", "password", "secret", "token=", "bearer ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn excluded_text(lower: &str) -> bool {
    const EXCLUDED: [&str; 27] = [
        "diff --git",
        "```",
        "tool output",
        "tool result",
        "begin patch",
        "api_key",
        "api key",
        "password",
        "secret",
        "token=",
        "bearer ",
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
    EXCLUDED.iter().any(|needle| lower.contains(needle))
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
    let mut parent = (0..corrections.len()).collect::<Vec<_>>();
    for left in 0..corrections.len() {
        for right in left + 1..corrections.len() {
            if related(&corrections[left].tokens, &corrections[right].tokens) {
                unite(&mut parent, left, right);
            }
        }
    }
    let mut groups = BTreeMap::<usize, Vec<Correction>>::new();
    for (index, correction) in corrections.iter().enumerate() {
        let root = root(&mut parent, index);
        groups.entry(root).or_default().push(correction.clone());
    }
    let mut candidates = groups
        .into_values()
        .filter_map(|mut rows| {
            rows.sort_by_key(|row| row.seq);
            let mut seen = BTreeSet::new();
            rows.retain(|row| seen.insert(locus_identity(row)));
            if rows.len() < threshold {
                return None;
            }
            let candidate = candidate(rows);
            (!candidate.triggers.is_empty()).then_some(candidate)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.finding.cmp(&right.finding));
    candidates
}

fn related(left: &BTreeSet<String>, right: &BTreeSet<String>) -> bool {
    let shared = left.intersection(right).count();
    let smaller = left.len().min(right.len());
    shared >= 2 && shared * 3 >= smaller * 2
}

fn root(parent: &mut [usize], index: usize) -> usize {
    if parent[index] != index {
        parent[index] = root(parent, parent[index]);
    }
    parent[index]
}

fn unite(parent: &mut [usize], left: usize, right: usize) {
    let left = root(parent, left);
    let right = root(parent, right);
    let chosen = left.min(right);
    parent[left] = chosen;
    parent[right] = chosen;
}

fn candidate(corrections: Vec<Correction>) -> Candidate {
    let oldest = corrections.first().expect("candidate has corrections");
    let fingerprint = oldest.tokens.iter().cloned().collect::<Vec<_>>().join("\0");
    let finding = Digest::compute(Domain::TasteProse, fingerprint.as_bytes());
    let mut loci = corrections.iter().map(locus_identity).collect::<Vec<_>>();
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

fn locus_identity(correction: &Correction) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}",
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
                task.statement.body.handle.clone(),
                task.workflow.body.completion.is_some(),
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
        let finding = finding_text
            .parse::<Digest>()
            .map_err(|_| EvidenceError::ProposalFinding(added.task.clone()))?;
        let recurrence = added
            .description
            .as_deref()
            .and_then(parse_recurrence)
            .ok_or_else(|| EvidenceError::ProposalDescription(added.task.clone()))?;
        proposals.entry(finding).or_default().push(PriorProposal {
            completed: completion
                .get(&added.task)
                .copied()
                .ok_or_else(|| EvidenceError::ProposalStanding(added.task.clone()))?,
            handle: added.task,
            recurrence,
        });
    }
    Ok(proposals)
}

fn parse_recurrence(description: &str) -> Option<usize> {
    description.lines().find_map(|line| {
        line.strip_prefix("Recurrence count: ")
            .and_then(|count| count.parse().ok())
    })
}

fn plan(
    candidate: &Candidate,
    priors: &BTreeMap<Digest, Vec<PriorProposal>>,
) -> (TaskHandle, ProposalStanding, Option<TaskHandle>) {
    let expected = crate::board_command::taste_proposal_handle(&candidate.occurrence);
    let Some(priors) = priors.get(&candidate.finding) else {
        return (expected, ProposalStanding::Filed, None);
    };
    if let Some(prior) = priors.iter().find(|prior| prior.handle == expected) {
        return (
            prior.handle.clone(),
            if prior.completed {
                ProposalStanding::ReusedCompleted
            } else {
                ProposalStanding::ReusedActive
            },
            None,
        );
    }
    if let Some(prior) = priors.iter().find(|prior| !prior.completed) {
        return (prior.handle.clone(), ProposalStanding::ReusedActive, None);
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
            prior.handle.clone(),
            ProposalStanding::ReusedCompleted,
            None,
        );
    }
    (
        expected,
        ProposalStanding::Filed,
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
        description.push_str(&format!(
            "- directive {} key {} ({}) at {}: {:?}",
            correction.directive,
            correction.key,
            correction.disposition,
            locus_text(&correction.locus),
            correction.excerpt,
        ));
        if let Some(reason) = &correction.decline_reason {
            description.push_str(&format!("; NACK reason: {reason:?}"));
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
            format!("ACK/NACK references unavailable operator steering directive {directive}")
        }
        EvidenceError::AnswerMismatch(directive) => {
            format!("ACK/NACK key or lane contradicts steering directive {directive}")
        }
        EvidenceError::DuplicateAnswer(directive) => {
            format!("steering directive {directive} has more than one ACK/NACK disposition")
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
        EvidenceError::ProposalDescription(task) => {
            format!("hidden taste proposal {task} has no admitted recurrence count")
        }
    };
    fault(FaultClass::Store, "taste::mining-evidence-invalid", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(text: &str) -> SteerSent {
        SteerSent {
            directive: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            key: "taste-1".to_owned(),
            lane: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            text: text.to_owned(),
            control: None,
            attachments: Vec::new(),
            expires: "2026-08-09T20:00:00.000Z".parse().unwrap(),
        }
    }

    #[test]
    fn correction_filter_preserves_exact_authored_excerpt_and_excludes_noise() {
        let text = "Context.\n  Do not use grep here; use rg instead.\nThanks";
        let (excerpt, words) = authored_correction(&sent(text)).expect("correction qualifies");
        assert_eq!(excerpt, "  Do not use grep here; use rg instead.");
        assert!(words.contains("grep"));
        assert!(words.contains("rg"));
        for excluded in [
            "Maybe use rg instead.",
            "Status update: use rg.",
            "```diff\n-use grep\n+use rg\n```",
            "Tool output: do not use grep",
            "API key secret: do not use grep",
            "🌶️ last maxim: do not use grep",
        ] {
            assert!(authored_correction(&sent(excluded)).is_none(), "{excluded}");
        }
        assert!(contains_sensitive(
            "declined because bearer abc was exposed"
        ));
    }

    #[test]
    fn near_duplicate_theme_qualifies_once_and_nonrecurrence_does_not() {
        let corrections = [
            "Do not use grep here; use rg instead.",
            "Prefer rg rather than grep for searches.",
            "Never reach for grep; always use rg.",
            "Always validate config with cargo metadata.",
        ]
        .into_iter()
        .enumerate()
        .map(|(index, text)| correction(index as u64 + 1, text))
        .collect::<Vec<_>>();
        let candidates = cluster(&corrections, 3);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].corrections.len(), 3);
        assert_eq!(candidates[0].message, corrections[0].excerpt);
    }

    #[test]
    fn completed_predecessor_refiles_only_after_genuine_new_recurrence() {
        let base = vec![
            correction(1, "Do not use grep; use rg instead."),
            correction(2, "Prefer rg rather than grep."),
            correction(3, "Never use grep; always use rg."),
        ];
        let first = candidate(base.clone());
        let prior = PriorProposal {
            handle: "TASTE-0123456789abcdef".parse().unwrap(),
            recurrence: 3,
            completed: true,
        };
        let priors = BTreeMap::from([(first.finding.clone(), vec![prior.clone()])]);
        let (_, standing, dependency) = plan(&first, &priors);
        assert_eq!(standing, ProposalStanding::ReusedCompleted);
        assert!(dependency.is_none());

        let mut expanded = base;
        expanded.push(correction(4, "Avoid grep and use rg for searches."));
        let later = candidate(expanded);
        let (_, standing, dependency) = plan(&later, &priors);
        assert_eq!(standing, ProposalStanding::Filed);
        assert_eq!(dependency, Some(prior.handle));
    }

    #[test]
    fn unavailable_ack_context_is_a_loud_store_fault() {
        let directive = "00000000-0000-4000-8000-000000000099".parse().unwrap();
        let fact = wire::Fact::new(SteerAcked {
            directive,
            key: "missing-ack".to_owned(),
            lane: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            epoch: "00000000-0000-4000-8000-000000000003".parse().unwrap(),
            locus: TranscriptLocus {
                member: 1,
                offset: 10,
                length: 50,
                event: 0,
                line: 1,
                column: 1,
            },
        });
        let opaque = wire::decode::<wire::AnyFact>(&wire::encode(&fact).unwrap()).unwrap();
        let snapshot = journal::FactSnapshot::from_facts(vec![wire::Envelope {
            v: wire::WireVersion::CURRENT,
            seq: Seq::new(1).unwrap(),
            at: "2026-08-09T20:00:00.000Z".parse().unwrap(),
            actor: Actor::Spice,
            origin: "taste:test".parse().unwrap(),
            cause: None,
            fact: opaque,
        }])
        .unwrap();
        assert!(matches!(
            corrections(&snapshot),
            Err(EvidenceError::AnswerWithoutSent(missing)) if missing == directive
        ));
        let Outcome::Fault(fault) = evidence_fault(EvidenceError::AnswerWithoutSent(directive))
        else {
            panic!("unavailable evidence must fault")
        };
        assert_eq!(fault.reason, "taste::mining-evidence-invalid");
        assert!(
            fault
                .detail
                .contains("unavailable operator steering directive")
        );
    }

    fn correction(position: u64, text: &str) -> Correction {
        Correction {
            seq: Seq::new(position).unwrap(),
            directive: format!("00000000-0000-4000-8000-{position:012}")
                .parse()
                .unwrap(),
            key: format!("taste-{position}"),
            lane: "00000000-0000-4000-8000-000000000002".parse().unwrap(),
            excerpt: text.to_owned(),
            tokens: tokens(text),
            locus: TranscriptLocus {
                member: position,
                offset: position * 10,
                length: 50,
                event: 0,
                line: 1,
                column: 1,
            },
            disposition: "acked",
            decline_reason: None,
        }
    }
}
