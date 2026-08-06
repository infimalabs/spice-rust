//! Binary-rim translation for release publication and daemonless notes.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use serde::de::DeserializeOwned;
use wire::body::{ProofSealed, PublishedTarget, ReleasePublished};
use wire::{
    Digest, Domain, FactKind, Fault, FaultClass, Outcome, Refusal, ReleaseNotes, ReleasePlan,
    Reply, ReplyPosition, Seq,
};

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

pub(crate) fn run(matches: &ArgMatches, contract: &crate::cli::Contract) -> RunResult {
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(outcome) => return RunResult::Plan(*outcome),
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
    let outcome = crate::command_client::execute(
        &prepared.canvas,
        contract,
        crate::command_client::SelectedCommand::ReleasePublish {
            release: &prepared.release,
            state: prepared.state,
        },
    );
    RunResult::Published(recognition_outcome(outcome))
}

struct Prepared {
    release: AdmittedRelease,
    state: PublicationState,
    canvas: PathBuf,
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
    let app = app_digest(repository.worktree_root())
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::app-stage-refused", &error)))?;
    let release = admit_stage(&repository.worktree_root().join("dist/stage"), &app)
        .map_err(|error| Box::new(refusal::<ReleasePlan>("release::manifest-refused", &error)))?;
    let state_paths = repository
        .admit_state()
        .map_err(|error| Box::new(crate::state_failure(&error).outcome()))?;
    let reader = journal::JournalReader::open(state_paths.journal()).map_err(|error| {
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
    Ok(Prepared {
        release,
        state,
        canvas: state_paths.canvas().to_path_buf(),
    })
}

fn recognition_outcome(
    outcome: Outcome<wire::intent::CommandAnswer>,
) -> Outcome<wire::intent::RecognitionAnswer> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            wire::intent::CommandAnswer::Recognition(body) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            _ => Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "release::answer-shape".to_owned(),
                detail: "resident returned a non-recognition release answer".to_owned(),
            }),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

#[cfg(test)]
mod tests {
    use wire::intent::{CommandAnswer, StateAnswer};
    use wire::{ResidentState, Seq, SeqRange};

    use super::*;

    #[test]
    fn publication_wrapper_refuses_an_answer_outside_its_command_binding() {
        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: Seq::new(1).expect("fixture position"),
                last: Seq::new(1).expect("fixture position"),
            }),
            body: CommandAnswer::State(StateAnswer {
                from: ResidentState::Seated,
                to: ResidentState::Dark,
            }),
        });

        let Outcome::Fault(fault) = recognition_outcome(outcome) else {
            panic!("a release reply with another command's answer must fault");
        };
        assert_eq!(fault.class, FaultClass::Protocol);
        assert_eq!(fault.reason, "release::answer-shape");
    }

    #[test]
    fn notes_reads_the_one_explicit_input_operand_it_was_given() {
        let name = format!("spice-release-notes-input-{}", std::process::id());
        let root = std::env::temp_dir().join(name);
        fs::create_dir_all(&root).expect("scratch root");
        let input = root.join("release-input.json");
        let path = input.to_str().expect("scratch path is UTF-8");
        let dispatch = |path: &str| {
            let matches = spice::cli::command()
                .try_get_matches_from(["spice", "release", "notes", path])
                .expect("one explicit release input parses");
            notes(spice::cli::selected_matches(&matches))
        };
        let refused = |bytes: &[u8]| {
            fs::write(&input, bytes).expect("scratch input");
            let outcome = dispatch(path);
            let Outcome::Refusal(refusal) = outcome else {
                panic!("release notes must refuse an input that is not release evidence");
            };
            assert_eq!(refusal.reason, "release::notes-refused");
            refusal.observed.expect("a refusal names what it read")
        };

        // Two inputs at one path refuse differently, so the bytes behind the
        // operand decide the answer and no other source can stand in for them.
        assert_ne!(refused(b"{}"), refused(b"a release range"));

        fs::remove_file(&input).expect("scratch input is removable");
        let Outcome::Fault(fault) = dispatch(path) else {
            panic!("an unreadable release input must fault");
        };
        assert_eq!(fault.class, FaultClass::Environment);
        assert_eq!(fault.reason, "release::input-unreadable");
        assert!(
            fault.detail.contains(path),
            "the release-notes fault named an input it was never given: {:?}",
            fault.detail,
        );
        fs::remove_dir_all(&root).expect("scratch root is removable");
    }
}

pub(crate) fn app_digest(worktree: &Path) -> Result<Digest, String> {
    canvas::AppSource::resolve(worktree)
        .and_then(|source| source.digest())
        .map_err(|error| error.to_string())
}

pub(crate) fn admit_stage(root: &Path, app: &Digest) -> Result<AdmittedRelease, ReleaseError> {
    let executable = std::env::current_exe().map_err(ReleaseError::CurrentExecutable)?;
    let bytes = fs::read(&executable).map_err(|source| ReleaseError::ReadExecutable {
        path: executable,
        source,
    })?;
    let native = Digest::compute(Domain::DistArtifact, &bytes);
    let target = proof::compiled_target().map_err(ReleaseError::UnsupportedHost)?;
    let published =
        proof::admit_manifest(root, app, target, &native).map_err(ReleaseError::Manifest)?;
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

pub(crate) fn notes(matches: &clap::ArgMatches) -> Outcome<ReleaseNotes> {
    let input = matches
        .get_one::<String>("INPUT")
        .expect("release notes requires its generated INPUT operand");
    let bytes = match fs::read(Path::new(input)) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "release::input-unreadable".to_owned(),
                detail: format!("cannot read release input {input:?}: {error}"),
            });
        }
    };
    match dist::release_notes(&bytes) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn failure(error: dist::NotesError) -> Outcome<ReleaseNotes> {
    Outcome::Refusal(Refusal {
        reason: "release::notes-refused".to_owned(),
        witness: Some("release-input".to_owned()),
        expected: Some("one canonical current ordered release evidence document".to_owned()),
        observed: Some(error.to_string()),
        remedy: "supply both target-matched proof and gate receipts for one source".to_owned(),
    })
}
