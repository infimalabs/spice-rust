//! Tracked taste reporting and reviewed curation proposals.

use std::collections::{BTreeMap, BTreeSet};

use clap::ArgMatches;
use serde::Serialize;
use spice::config::LoadError;
use spice::repository::Repository;
use wire::body::{
    LaneSeated, TasteCleared, TasteConfirmed, TasteDecision, TasteJudgeAttempt, TasteJudged,
    TasteReminded,
};
use wire::intent::{CommandAnswer, TaskAdd, TaskAnswer};
use wire::{
    CanonicalUuid, ConfigSource, Digest, Disposition, Domain, Fault, FaultClass, Instant, Name,
    Outcome, Refusal, Reply, ReplyPosition, Seq,
};

use crate::cli::Contract;

mod mining;
pub(crate) use mining::mine;

const PROJECT: &str = "taste.policy";

/// Recent fires carried per taste.
///
/// The curation view shows fires and their ages rather than an all-time count,
/// so each row is bounded and newest first. Nothing is persisted: the whole
/// view is derived from one journal snapshot on every call.
const RECENT_FIRES: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Surface {
    text: Option<String>,
    location: String,
    digest: Digest,
    tastes: Vec<TasteDefinition>,
    mining_threshold: Option<usize>,
}

impl Surface {
    fn load(repository: &Repository) -> Result<Self, LoadError> {
        let loaded = spice::config::capture_repository(repository, &[])?;
        let text = loaded.tracked().map(ToOwned::to_owned);
        let location = loaded.locations().tracked().display().to_string();
        let digest = tracked_digest(text.as_deref());
        let tastes = loaded
            .resolved()
            .root()
            .taste()
            .bag()
            .tastes()
            .iter()
            .map(|taste| TasteDefinition {
                name: taste.name().clone(),
                words: taste
                    .triggers()
                    .iter()
                    .map(|trigger| trigger.as_str().to_owned())
                    .collect(),
                message: taste.message().to_owned(),
            })
            .collect();
        let mining_threshold = loaded.resolved().root().policy().taste_mining_threshold();
        Ok(Self {
            text,
            location,
            digest,
            tastes,
            mining_threshold,
        })
    }

    fn provenance(&self) -> TasteProvenance {
        TasteProvenance {
            source: ConfigSource::Tracked,
            location: self.location.clone(),
            present: self.text.is_some(),
            digest: self.text.as_ref().map(|_| self.digest.clone()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TasteDefinition {
    name: Name,
    words: Vec<String>,
    message: String,
}

/// Bare `spice taste` answer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TasteReport {
    digest: Digest,
    provenance: TasteProvenance,
    tastes: Vec<TasteRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TasteProvenance {
    source: ConfigSource,
    location: String,
    present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TasteRow {
    name: Name,
    words: Vec<String>,
    message: String,
    fires: Vec<TasteFire>,
    vendors: Vec<VendorTally>,
    recurrence: Vec<Recurrence>,
    judgments: Vec<TasteJudgment>,
}

/// One reminder this taste opened, with the standing of its stable directive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TasteFire {
    position: Seq,
    at: Instant,
    lane: CanonicalUuid,
    vendor: Name,
    occupant: CanonicalUuid,
    compaction: u64,
    content: Digest,
    key: String,
    standing: Disposition,
    locus: String,
}

struct FireEvidence {
    taste: String,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    compaction: u64,
    content: Digest,
    directive: CanonicalUuid,
    key: String,
    locus: wire::body::TranscriptLocus,
}

/// Fires and answers for one vendor, over the fires this report carries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct VendorTally {
    vendor: Name,
    fires: usize,
    acked: usize,
    declined: usize,
}

/// One unchanged identity that fired again in a later compaction epoch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct Recurrence {
    content: Digest,
    epochs: usize,
    latest: Seq,
}

/// A join this report refuses to guess at.
enum EvidenceError {
    /// A reminder names a lane the durable stream never seated.
    ///
    /// Nothing in the stream requires a reminded lane to have been seated
    /// first, so this is the one join a real journal can actually be missing.
    LaneUnseated(CanonicalUuid),
    /// A reminder names a directive its own steering standing does not carry.
    ///
    /// The standing is replayed from the same snapshot the reminder came from,
    /// and the replay opens a directive for every reminder it absorbs, so this
    /// join can only fail if the store contradicts itself.
    DirectiveMissing(CanonicalUuid),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TasteJudgment {
    position: Seq,
    at: Instant,
    verdict: Verdict,
    locus: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bag: Option<Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attempts: Vec<TasteJudgeAttempt>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    Confirmed,
    Cleared,
}

/// Authenticate through the resident and report one exact tracked generation.
pub(crate) fn report(contract: &Contract) -> Outcome<TasteReport> {
    match snapshot::<TasteReport>(contract) {
        Ok((_, reply)) => Outcome::Reply(reply),
        Err(outcome) => outcome,
    }
}

/// Create reviewed work for one evidence-backed taste admission.
pub(crate) fn add(matches: &ArgMatches, contract: &Contract) -> Outcome<TaskAnswer> {
    let evidence = matches
        .get_one::<String>("TEXT")
        .expect("the generated taste add syntax requires TEXT")
        .clone();
    if evidence.trim().is_empty() {
        return refusal(
            "taste::evidence-empty",
            Some("non-empty recurring evidence".to_owned()),
            Some(evidence),
            "describe the recurring evidence that motivates the proposed taste",
        );
    }
    let origin = match crate::board_command::parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return retag(outcome),
    };
    let repository = match current_repository() {
        Ok(repository) => repository,
        Err(outcome) => return outcome,
    };
    let surface = match Surface::load(&repository) {
        Ok(surface) => surface,
        Err(error) => return load_failure(&error),
    };
    let title = format!("Propose tracked taste addition: {}", headline(&evidence));
    let description = format!(
        concat!(
            "Recurring evidence:\n\n{}\n\n",
            "Tracked source: {}\nTracked generation: {}"
        ),
        evidence, surface.location, surface.digest,
    );
    submit_plan(
        &repository,
        contract,
        TaskAdd {
            title,
            project: Some(PROJECT.to_owned()),
            description: Some(description),
            acceptance: vec![
                "Operator review adds one exact [taste.NAME] row to tracked spice.toml.".to_owned(),
                concat!(
                    "Paired behavior and mention fixtures confirm and clear the proposed bag, ",
                    "and the standing non-hit corpus remains clear."
                )
                .to_owned(),
            ],
            origin,
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
        },
    )
}

/// Create reviewed work for one current tracked taste retirement.
pub(crate) fn drop(matches: &ArgMatches, contract: &Contract) -> Outcome<TaskAnswer> {
    let written = matches
        .get_one::<String>("NAME")
        .expect("the generated taste drop syntax requires NAME");
    let name = match written.parse::<Name>() {
        Ok(name) => name,
        Err(error) => {
            return refusal(
                "taste::name-invalid",
                Some("one current lower-case taste name".to_owned()),
                Some(error.to_string()),
                "copy one exact name from spice taste output",
            );
        }
    };
    let origin = match crate::board_command::parse_origin(matches) {
        Ok(origin) => origin,
        Err(outcome) => return retag(outcome),
    };
    let (repository, reply) = match snapshot::<TaskAnswer>(contract) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(row) = reply.body.tastes.iter().find(|row| row.name == name) else {
        return refusal(
            "taste::unknown",
            Some(
                reply
                    .body
                    .tastes
                    .iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Some(name.to_string()),
            "read the current tracked taste set and name one active taste",
        );
    };
    let declines = cited(
        row.fires
            .iter()
            .filter(|fire| fire.standing == Disposition::Declined)
            .map(|fire| {
                format!(
                    "- {} on {} at {} (journal {})",
                    fire.locus, fire.vendor, fire.at, fire.position
                )
            }),
    );
    let clearances = cited(
        row.judgments
            .iter()
            .filter(|judgment| judgment.verdict == Verdict::Cleared)
            .map(|judgment| {
                format!(
                    "- {} at {} (journal {})",
                    judgment.locus, judgment.at, judgment.position
                )
            }),
    );
    let words = serde_json::to_string(&row.words).expect("taste words encode as JSON");
    let message = serde_json::to_string(&row.message).expect("taste message encodes as JSON");
    let description = format!(
        concat!(
            "Current tracked row:\n",
            "[taste.{name}]\nwords = {words}\nmessage = {message}\n\n",
            "Tracked source: {location}\nTracked generation: {digest}\n\n",
            "Recent declined loci:\n{declines}\n\n",
            "Historical cleared loci:\n{clearances}"
        ),
        name = row.name,
        words = words,
        message = message,
        location = reply.body.provenance.location,
        digest = reply.body.digest,
        declines = declines,
        clearances = clearances,
    );
    submit_plan(
        &repository,
        contract,
        TaskAdd {
            title: format!("Propose tracked taste retirement: {}", row.name),
            project: Some(PROJECT.to_owned()),
            description: Some(description),
            acceptance: vec![
                format!(
                    concat!(
                        "Operator review narrows, revises, or deletes [taste.{}] in tracked ",
                        "spice.toml."
                    ),
                    row.name
                ),
                "Earlier durable judgments remain history and cannot restore deleted membership."
                    .to_owned(),
            ],
            origin,
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
        },
    )
}

fn snapshot<B>(contract: &Contract) -> Result<(Repository, Reply<TasteReport>), Outcome<B>> {
    let repository = current_repository()?;
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let epoch = crate::command_client::authenticate(state.canvas(), contract).map_err(retag)?;
    let surface = Surface::load(&repository).map_err(|error| load_failure(&error))?;
    let reader = journal::JournalReader::open(state.journal()).map_err(|error| {
        fault(
            FaultClass::Store,
            "taste::journal-unreadable",
            error.to_string(),
        )
    })?;
    let (residency, facts) = reader
        .read(|read| {
            let residency = read.residency_head()?;
            let facts = read
                .fact_snapshot()
                .map_err(|error| journal::GuardError::Invalid(error.to_string()))?;
            Ok((residency, facts))
        })
        .map_err(|error| {
            fault(
                FaultClass::Store,
                "taste::journal-unreadable",
                error.to_string(),
            )
        })?;
    let Some(residency) = residency else {
        return Err(fault(
            FaultClass::Store,
            "taste::residency-missing",
            "the authenticated resident has no durable residency head".to_owned(),
        ));
    };
    if residency.body().epoch != epoch {
        return Err(refusal(
            "taste::epoch-moved",
            Some(epoch.to_string()),
            Some(residency.body().epoch.to_string()),
            "reconnect through the current resident discovery and retry",
        ));
    }
    let Some(cursor) = facts.horizon() else {
        return Err(fault(
            FaultClass::Store,
            "taste::cursor-missing",
            "the authenticated resident has no durable journal position".to_owned(),
        ));
    };
    let judgments = collect_judgments(&facts);
    // One snapshot answers the whole view: steering standing, lane seating, and
    // reminders are all read from these same facts, so no two parts of the row
    // can describe different moments.
    let ledger = steer::Ledger::from_snapshot(&facts).map_err(|error| {
        fault(
            FaultClass::Store,
            "taste::steering-invalid",
            error.to_string(),
        )
    })?;
    let seating = lane_seating(&facts);
    let fires = collect_fires(&facts, &ledger, &seating).map_err(evidence_failure)?;
    let current = Surface::load(&repository).map_err(|error| load_failure(&error))?;
    if current != surface {
        return Err(refusal(
            "taste::surface-moved",
            Some(surface.digest.to_string()),
            Some(current.digest.to_string()),
            "retry against the current tracked taste configuration",
        ));
    }
    let tastes = surface
        .tastes
        .iter()
        .map(|taste| {
            let fires = fires.get(taste.name.as_str()).cloned().unwrap_or_default();
            TasteRow {
                name: taste.name.clone(),
                words: taste.words.clone(),
                message: taste.message.clone(),
                vendors: tally(&fires),
                recurrence: recurrence(&fires),
                fires,
                judgments: judgments
                    .get(taste.name.as_str())
                    .cloned()
                    .unwrap_or_default(),
            }
        })
        .collect();
    let report = TasteReport {
        digest: surface.digest.clone(),
        provenance: surface.provenance(),
        tastes,
    };
    Ok((
        repository,
        Reply {
            position: ReplyPosition::Snapshot(cursor),
            body: report,
        },
    ))
}

fn collect_judgments(snapshot: &journal::FactSnapshot) -> BTreeMap<String, Vec<TasteJudgment>> {
    let mut judgments = BTreeMap::<String, Vec<TasteJudgment>>::new();
    for envelope in snapshot.facts() {
        let (taste, judgment) = if let Some(body) = envelope.fact.read::<TasteConfirmed>() {
            (
                body.taste,
                historical_judgment(envelope, Verdict::Confirmed, body.locus),
            )
        } else if let Some(body) = envelope.fact.read::<TasteCleared>() {
            (
                body.taste,
                historical_judgment(envelope, Verdict::Cleared, body.locus),
            )
        } else if let Some(body) = envelope.fact.read::<TasteJudged>() {
            (
                body.taste,
                TasteJudgment {
                    position: envelope.seq,
                    at: envelope.at,
                    verdict: match body.decision {
                        TasteDecision::Confirmed { .. } => Verdict::Confirmed,
                        TasteDecision::Cleared => Verdict::Cleared,
                    },
                    locus: locus_text(&body.locus),
                    content: Some(body.content),
                    bag: Some(body.bag),
                    trigger: Some(body.trigger),
                    driver: Some(body.driver),
                    source: Some(body.source),
                    compaction: Some(body.compaction),
                    attempts: body.attempts,
                },
            )
        } else {
            continue;
        };
        judgments.entry(taste).or_default().push(judgment);
    }
    judgments
}

fn historical_judgment(
    envelope: &wire::Envelope<wire::AnyFact>,
    verdict: Verdict,
    locus: String,
) -> TasteJudgment {
    TasteJudgment {
        position: envelope.seq,
        at: envelope.at,
        verdict,
        locus,
        content: None,
        bag: None,
        trigger: None,
        driver: None,
        source: None,
        compaction: None,
        attempts: Vec::new(),
    }
}

/// Every reminder in the snapshot, grouped by taste and newest first.
fn collect_fires(
    snapshot: &journal::FactSnapshot,
    ledger: &steer::Ledger,
    seating: &BTreeMap<CanonicalUuid, Name>,
) -> Result<BTreeMap<String, Vec<TasteFire>>, EvidenceError> {
    let mut fires = BTreeMap::<String, Vec<TasteFire>>::new();
    for envelope in snapshot.facts() {
        let evidence = if let Some(body) = envelope.fact.read::<TasteReminded>() {
            FireEvidence {
                taste: body.taste,
                lane: body.lane,
                occupant: body.occupant,
                compaction: body.compaction,
                content: body.content,
                directive: body.directive,
                key: body.key,
                locus: body.locus,
            }
        } else if let Some(body) = envelope.fact.read::<TasteJudged>() {
            let TasteDecision::Confirmed { directive, key, .. } = body.decision else {
                continue;
            };
            FireEvidence {
                taste: body.taste,
                lane: body.lane,
                occupant: body.occupant,
                compaction: body.compaction,
                content: body.content,
                directive,
                key,
                locus: body.locus,
            }
        } else {
            continue;
        };
        let vendor = seating
            .get(&evidence.lane)
            .ok_or(EvidenceError::LaneUnseated(evidence.lane))?
            .clone();
        // Escalation never replaces the reminder's identity, so this directive
        // carries its current disposition directly. Reading it here counts one
        // publication exactly once regardless of presentation stage.
        let standing = ledger
            .directive(evidence.directive)
            .ok_or(EvidenceError::DirectiveMissing(evidence.directive))?
            .disposition();
        fires.entry(evidence.taste).or_default().push(TasteFire {
            position: envelope.seq,
            at: envelope.at,
            lane: evidence.lane,
            vendor,
            occupant: evidence.occupant,
            compaction: evidence.compaction,
            content: evidence.content,
            key: evidence.key,
            standing,
            locus: locus_text(&evidence.locus),
        });
    }
    for rows in fires.values_mut() {
        rows.sort_by_key(|row| std::cmp::Reverse(row.position));
        rows.truncate(RECENT_FIRES);
    }
    Ok(fires)
}

/// The vendor each lane was seated with.
///
/// Attribution reads seating rather than the occupant a fire happened to name,
/// because the durable lane laws make the seat the stable answer: a lane is
/// seated exactly once, and a worktree transfer must retain the same vendor.
/// Every renewal therefore attributes to the vendor the lane has always had,
/// instead of splitting one vendor into a bucket per occupant generation. A
/// lane the stream never seated has no vendor at all, and the report refuses
/// rather than inventing a bucket that sums with a real one — a refusal the
/// fire join raises, since absence is only knowable once a reminder names it.
fn lane_seating(snapshot: &journal::FactSnapshot) -> BTreeMap<CanonicalUuid, Name> {
    let mut seating = BTreeMap::<CanonicalUuid, Name>::new();
    for envelope in snapshot.facts() {
        if let Some(body) = envelope.fact.read::<LaneSeated>() {
            seating.insert(body.lane, body.vendor);
        }
    }
    seating
}

/// Fires and answers per vendor, over the fires the row carries.
fn tally(fires: &[TasteFire]) -> Vec<VendorTally> {
    let mut tallies = BTreeMap::<Name, VendorTally>::new();
    for fire in fires {
        let tally = tallies
            .entry(fire.vendor.clone())
            .or_insert_with(|| VendorTally {
                vendor: fire.vendor.clone(),
                fires: 0,
                acked: 0,
                declined: 0,
            });
        tally.fires += 1;
        match fire.standing {
            Disposition::Acked => tally.acked += 1,
            Disposition::Declined => tally.declined += 1,
            _ => {}
        }
    }
    tallies.into_values().collect()
}

/// Identities that fired again after a compaction epoch moved.
fn recurrence(fires: &[TasteFire]) -> Vec<Recurrence> {
    let mut spans = BTreeMap::<Digest, (BTreeSet<u64>, Seq)>::new();
    for fire in fires {
        let span = spans
            .entry(fire.content.clone())
            .or_insert_with(|| (BTreeSet::new(), fire.position));
        span.0.insert(fire.compaction);
        span.1 = span.1.max(fire.position);
    }
    spans
        .into_iter()
        .filter(|(_, (epochs, _))| epochs.len() > 1)
        .map(|(content, (epochs, latest))| Recurrence {
            content,
            epochs: epochs.len(),
            latest,
        })
        .collect()
}

fn locus_text(locus: &wire::body::TranscriptLocus) -> String {
    format!(
        "tape {}+{} event {} line {} column {}",
        locus.member, locus.offset, locus.event, locus.line, locus.column
    )
}

fn submit_plan(repository: &Repository, contract: &Contract, plan: TaskAdd) -> Outcome<TaskAnswer> {
    match crate::board_command::submit_task_add(repository, contract, plan) {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::Task(answer) => Outcome::Reply(Reply {
                position: reply.position,
                body: answer,
            }),
            answer => fault(
                FaultClass::Protocol,
                "taste::task-answer",
                format!("expected a task answer, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

/// Render cited evidence, spelling an empty citation rather than dropping it.
fn cited(lines: impl Iterator<Item = String>) -> String {
    let lines = lines.collect::<Vec<_>>();
    if lines.is_empty() {
        "- none".to_owned()
    } else {
        lines.join("\n")
    }
}

fn headline(evidence: &str) -> String {
    let normalized = evidence.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let prefix = chars.by_ref().take(72).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn tracked_digest(text: Option<&str>) -> Digest {
    let mut framed = Vec::new();
    framed.extend_from_slice(ConfigSource::Tracked.as_str().as_bytes());
    framed.push(0);
    match text {
        None => framed.push(0),
        Some(text) => {
            framed.push(1);
            let length = u64::try_from(text.len()).expect("configuration text length fits u64");
            framed.extend_from_slice(&length.to_be_bytes());
            framed.extend_from_slice(text.as_bytes());
        }
    }
    Digest::compute(Domain::ConfigExecutableSurface, &framed)
}

fn current_repository<B>() -> Result<Repository, Outcome<B>> {
    let current = std::env::current_dir().map_err(|error| {
        fault(
            FaultClass::Environment,
            "taste::working-directory-unavailable",
            error.to_string(),
        )
    })?;
    Repository::discover(&current).map_err(|error| {
        fault(
            FaultClass::Environment,
            "taste::repository-unresolved",
            error.to_string(),
        )
    })
}

/// Report a join the durable stream cannot answer, by name.
///
/// A missing seating or directive is evidence the report would otherwise have
/// to guess at, and a guess here reads as curation truth. It refuses instead,
/// naming the exact identity that did not join.
fn evidence_failure<B>(error: EvidenceError) -> Outcome<B> {
    match error {
        EvidenceError::LaneUnseated(lane) => refusal(
            "taste::lane-unseated",
            Some("one durable lane seating at or before the reminder".to_owned()),
            Some(lane.to_string()),
            "replay or repair the lane history that seats the reminded lane",
        ),
        // A refusal would offer the operator a remedy for something no request
        // of theirs caused. Replayed standing that lost a reminder it just
        // absorbed is a broken invariant, so it faults by name instead.
        EvidenceError::DirectiveMissing(directive) => fault(
            FaultClass::Store,
            "taste::directive-missing",
            format!("replayed steering standing does not carry directive {directive}"),
        ),
    }
}

fn load_failure<B>(error: &LoadError) -> Outcome<B> {
    if let Some(repository) = error.repository() {
        return fault(
            FaultClass::Environment,
            "taste::repository-unresolved",
            repository.to_string(),
        );
    }
    let mut diagnostics = error
        .acquisition()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(resolution) = error.resolution() {
        diagnostics.extend(resolution.issues().iter().map(ToString::to_string));
    }
    let detail = if diagnostics.is_empty() {
        error.to_string()
    } else {
        diagnostics.join(" | ")
    };
    if error
        .acquisition()
        .iter()
        .any(|issue| issue.io_error().is_some())
    {
        fault(FaultClass::Environment, "taste::policy-unreadable", detail)
    } else {
        refusal(
            "taste::policy-refused",
            Some("one tracked taste set admitted by the typed registry".to_owned()),
            Some(detail),
            "correct the named configuration inputs and retry",
        )
    }
}

fn retag<B>(outcome: Outcome<CommandAnswer>) -> Outcome<B> {
    match outcome {
        Outcome::Reply(_) => unreachable!("authentication does not return a command reply"),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn refusal<B>(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    detail: &str,
) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("tracked-taste-set".to_owned()),
            expected,
            observed,
        )
        .with_detail(detail),
    ))
}

fn fault<B>(class: FaultClass, reason: &str, detail: String) -> Outcome<B> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail,
    })
}
