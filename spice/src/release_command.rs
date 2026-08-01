use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use serde::de::DeserializeOwned;
use wire::body::{ProofSealed, PublishedTarget, ReleasePublished};
use wire::{Digest, Domain, FactKind, Outcome, Refusal, ReleasePlan, Reply, ReplyPosition, Seq};

use crate::resident;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdmittedRelease {
    published: proof::PublishedManifest,
    key: Digest,
    targets: Vec<PublishedTarget>,
}

impl AdmittedRelease {
    pub(crate) const fn key(&self) -> &Digest {
        &self.key
    }

    pub(crate) const fn manifest(&self) -> &Digest {
        &self.published.digest
    }

    pub(crate) fn receipts(&self) -> impl Iterator<Item = &Digest> {
        self.targets.iter().map(|target| &target.receipt)
    }

    pub(crate) fn fact(&self) -> ReleasePublished {
        ReleasePublished {
            manifest: self.published.digest.clone(),
            targets: self.targets.clone(),
        }
    }

    fn plan(&self) -> ReleasePlan {
        ReleasePlan {
            key: self.key.clone(),
            manifest: self.published.digest.clone(),
            source: self.published.manifest.source.clone(),
            targets: self.targets.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationState {
    Fresh { proof: Seq },
    Published { fact: Seq },
}

impl PublicationState {
    pub(crate) const fn observation(self) -> Seq {
        match self {
            Self::Fresh { proof } => proof,
            Self::Published { fact } => fact,
        }
    }
}

pub(crate) enum RunResult {
    Plan(Outcome<ReleasePlan>),
    Published(Outcome<wire::intent::RecognitionAnswer>),
}

pub(crate) fn run(matches: &ArgMatches) -> RunResult {
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(outcome) => return RunResult::Plan(*outcome),
    };
    let connection = match resident::connect_operator(&prepared.discovery) {
        Ok(connection) => connection,
        Err(outcome) => return RunResult::Plan(map_outcome(outcome, |_| unreachable!())),
    };
    let Some(apply) = matches.get_one::<String>("apply") else {
        return RunResult::Plan(Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: prepared.release.plan(),
        }));
    };
    let applied = match apply.parse::<Digest>() {
        Ok(applied) => applied,
        Err(error) => {
            return RunResult::Published(Outcome::Refusal(Refusal {
                reason: "release::apply-digest-refused".to_owned(),
                witness: Some("publication-plan".to_owned()),
                expected: Some(prepared.release.key().to_string()),
                observed: Some(error.to_string()),
                remedy: "preview the current release and apply its exact key".to_owned(),
            }));
        }
    };
    if applied != *prepared.release.key() {
        return RunResult::Published(Outcome::Refusal(Refusal {
            reason: "release::plan-moved".to_owned(),
            witness: Some("publication-plan".to_owned()),
            expected: Some(prepared.release.key().to_string()),
            observed: Some(applied.to_string()),
            remedy: "preview the current sealed manifest and apply its new key".to_owned(),
        }));
    }
    RunResult::Published(resident::request_release(
        connection,
        &prepared.release,
        prepared.state,
    ))
}

struct Prepared {
    release: AdmittedRelease,
    state: PublicationState,
    discovery: canvas::Discovery,
}

fn prepare() -> Result<Prepared, Box<Outcome<ReleasePlan>>> {
    let current = std::env::current_dir().map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::working-directory-unavailable",
            &error.to_string(),
        ))
    })?;
    let repository = spice::repository::Repository::discover(&current).map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::repository-unresolved",
            &error.to_string(),
        ))
    })?;
    let release = admit_stage(&repository.worktree_root().join("dist/stage"))
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::manifest-refused", &error)))?;
    let reader = journal::JournalReader::open(repository.journal_path()).map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::journal-unavailable",
            &error.to_string(),
        ))
    })?;
    let snapshot = reader.fact_snapshot().map_err(|error| {
        Box::new(environment::<ReleasePlan>(
            "release::journal-unreadable",
            &error.to_string(),
        ))
    })?;
    let state = observe(&snapshot, &release)
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::evidence-refused", &error)))?;
    let canvas = repository.common_dir().join(".spice/canvas");
    let discovery = resident::read_operator_discovery(&canvas)
        .map_err(|outcome| Box::new(map_outcome(outcome, |_| unreachable!())))?;
    Ok(Prepared {
        release,
        state,
        discovery,
    })
}

pub(crate) fn admit_stage(root: &Path) -> Result<AdmittedRelease, ReleaseError> {
    let executable = std::env::current_exe().map_err(ReleaseError::CurrentExecutable)?;
    let bytes = fs::read(&executable).map_err(|source| ReleaseError::ReadExecutable {
        path: executable,
        source,
    })?;
    let native = Digest::compute(Domain::DistArtifact, &bytes);
    let target = proof::compiled_target().map_err(ReleaseError::UnsupportedHost)?;
    let published = proof::admit_manifest(root, &canvas::EMBEDDED_APP.digest(), target, &native)
        .map_err(ReleaseError::Manifest)?;
    let key = Digest::compute(Domain::DistReleasePlan, &published.bytes);
    let targets = published
        .manifest
        .targets
        .iter()
        .map(|target| PublishedTarget {
            target: target.target.to_string(),
            artifact: target.payload.clone(),
            receipt: target.receipt.clone(),
        })
        .collect();
    Ok(AdmittedRelease {
        published,
        key,
        targets,
    })
}

pub(crate) fn observe(
    snapshot: &journal::FactSnapshot,
    release: &AdmittedRelease,
) -> Result<PublicationState, ObservationError> {
    let mut proof = None;
    let mut published = None;
    let expected = release.fact();
    for envelope in snapshot.facts() {
        match envelope.fact.kind() {
            FactKind::ProofSealed => {
                let body: ProofSealed = fact_body(&envelope.fact)?;
                if body.manifest == *release.manifest() {
                    proof = Some(envelope.seq);
                }
            }
            FactKind::ReleasePublished => {
                let body: ReleasePublished = fact_body(&envelope.fact)?;
                if body.manifest == *release.manifest() {
                    if body != expected {
                        return Err(ObservationError::PublishedMismatch(envelope.seq));
                    }
                    if published.replace(envelope.seq).is_some() {
                        return Err(ObservationError::DuplicatePublication);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(fact) = published {
        Ok(PublicationState::Published { fact })
    } else {
        proof
            .map(|proof| PublicationState::Fresh { proof })
            .ok_or(ObservationError::MissingProof)
    }
}

fn fact_body<T: DeserializeOwned>(fact: &wire::AnyFact) -> Result<T, ObservationError> {
    serde_json::from_value(serde_json::Value::Object(fact.body().clone()))
        .map_err(|source| ObservationError::InvalidFact(source.to_string()))
}

#[derive(Debug)]
pub(crate) enum ReleaseError {
    CurrentExecutable(std::io::Error),
    ReadExecutable {
        path: PathBuf,
        source: std::io::Error,
    },
    UnsupportedHost(proof::UnsupportedHost),
    Manifest(proof::ManifestAdmissionError),
}

impl fmt::Display for ReleaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(source) => {
                write!(formatter, "current executable is unresolved: {source}")
            }
            Self::ReadExecutable { path, source } => {
                write!(
                    formatter,
                    "current executable {} is unreadable: {source}",
                    path.display()
                )
            }
            Self::UnsupportedHost(source) => source.fmt(formatter),
            Self::Manifest(source) => source.fmt(formatter),
        }
    }
}

impl Error for ReleaseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable(source) => Some(source),
            Self::ReadExecutable { source, .. } => Some(source),
            Self::UnsupportedHost(source) => Some(source),
            Self::Manifest(source) => Some(source),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ObservationError {
    MissingProof,
    PublishedMismatch(Seq),
    DuplicatePublication,
    InvalidFact(String),
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingProof => {
                formatter.write_str("the current manifest has no durable proof::sealed recognition")
            }
            Self::PublishedMismatch(seq) => write!(
                formatter,
                "release publication at {seq} disagrees with the sealed target identities"
            ),
            Self::DuplicatePublication => {
                formatter.write_str("the sealed manifest was published more than once")
            }
            Self::InvalidFact(source) => {
                write!(
                    formatter,
                    "release authority contains an invalid fact: {source}"
                )
            }
        }
    }
}

impl Error for ObservationError {}

fn refusal<B>(reason: &str, error: &impl fmt::Display) -> Outcome<B> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("sealed-release".to_owned()),
        expected: Some("one current complete two-target proof and manifest chain".to_owned()),
        observed: Some(error.to_string()),
        remedy: "restore and recognize the exact current sealed release before publishing"
            .to_owned(),
    })
}

fn environment<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(wire::Fault {
        class: wire::FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn map_outcome<A, B>(outcome: Outcome<A>, map: impl FnOnce(A) -> B) -> Outcome<B> {
    match outcome {
        Outcome::Reply(reply) => Outcome::Reply(Reply {
            position: reply.position,
            body: map(reply.body),
        }),
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}
