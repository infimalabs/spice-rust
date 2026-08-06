//! Tracked taste reporting and reviewed curation proposals.

use std::collections::BTreeMap;

use clap::ArgMatches;
use serde::Serialize;
use spice::config::LoadError;
use spice::repository::Repository;
use wire::body::{TasteCleared, TasteConfirmed};
use wire::intent::{CommandAnswer, TaskAdd, TaskAnswer};
use wire::{
    ConfigSource, Digest, Domain, Fact, FactKind, Fault, FaultClass, Instant, Name, Outcome,
    Refusal, Reply, ReplyPosition, Seq,
};

use crate::cli::Contract;

const PROJECT: &str = "taste.policy";

#[derive(Clone, Debug, Eq, PartialEq)]
struct Surface {
    text: Option<String>,
    location: String,
    digest: Digest,
    tastes: Vec<TasteDefinition>,
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
        Ok(Self {
            text,
            location,
            digest,
            tastes,
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
    judgments: Vec<TasteJudgment>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TasteJudgment {
    position: Seq,
    at: Instant,
    verdict: Verdict,
    locus: String,
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
            origin: None,
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
    let clearances = row
        .judgments
        .iter()
        .filter(|judgment| judgment.verdict == Verdict::Cleared)
        .map(|judgment| {
            format!(
                "- {} at {} (journal {})",
                judgment.locus, judgment.at, judgment.position
            )
        })
        .collect::<Vec<_>>();
    let clearances = if clearances.is_empty() {
        "- none".to_owned()
    } else {
        clearances.join("\n")
    };
    let words = serde_json::to_string(&row.words).expect("taste words encode as JSON");
    let message = serde_json::to_string(&row.message).expect("taste message encodes as JSON");
    let description = format!(
        concat!(
            "Current tracked row:\n",
            "[taste.{name}]\nwords = {words}\nmessage = {message}\n\n",
            "Tracked source: {location}\nTracked generation: {digest}\n\n",
            "Current cleared loci:\n{clearances}"
        ),
        name = row.name,
        words = words,
        message = message,
        location = reply.body.provenance.location,
        digest = reply.body.digest,
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
            origin: None,
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
    let judgments = collect_judgments(&facts)
        .map_err(|detail| fault(FaultClass::Store, "taste::judgment-invalid", detail))?;
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
        .map(|taste| TasteRow {
            name: taste.name.clone(),
            words: taste.words.clone(),
            message: taste.message.clone(),
            judgments: judgments
                .get(taste.name.as_str())
                .cloned()
                .unwrap_or_default(),
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

fn collect_judgments(
    snapshot: &journal::FactSnapshot,
) -> Result<BTreeMap<String, Vec<TasteJudgment>>, String> {
    let mut judgments = BTreeMap::<String, Vec<TasteJudgment>>::new();
    for envelope in snapshot.facts() {
        let (taste, verdict, locus) = match envelope.fact.kind() {
            FactKind::TasteConfirmed => {
                let fact = decode_fact::<TasteConfirmed>(&envelope.fact)?;
                (fact.body.taste, Verdict::Confirmed, fact.body.locus)
            }
            FactKind::TasteCleared => {
                let fact = decode_fact::<TasteCleared>(&envelope.fact)?;
                (fact.body.taste, Verdict::Cleared, fact.body.locus)
            }
            _ => continue,
        };
        judgments.entry(taste).or_default().push(TasteJudgment {
            position: envelope.seq,
            at: envelope.at,
            verdict,
            locus,
        });
    }
    Ok(judgments)
}

fn decode_fact<T>(fact: &wire::AnyFact) -> Result<Fact<T>, String>
where
    T: wire::Body,
{
    let bytes = wire::encode(fact).map_err(|error| error.to_string())?;
    wire::decode(&bytes).map_err(|error| error.to_string())
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
    remedy: &str,
) -> Outcome<B> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("tracked-taste-set".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn fault<B>(class: FaultClass, reason: &str, detail: String) -> Outcome<B> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail,
    })
}
