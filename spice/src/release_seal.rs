//! Post-proof composition and resident recognition of one release manifest.

use std::env;
use std::path::PathBuf;

use wire::manifest::ReleaseTarget;
use wire::{Fault, FaultClass, Outcome, Refusal, Reply, SealedManifest};

use spice::repository::Repository;

struct StoredProof {
    payload: Vec<u8>,
    receipt: Vec<u8>,
}

struct Prepared {
    published: proof::PublishedManifest,
    canvas: PathBuf,
    head: wire::Seq,
}

pub(crate) fn run(contract: &crate::cli::Contract) -> Outcome<SealedManifest> {
    let prepared = match prepare() {
        Ok(prepared) => prepared,
        Err(outcome) => return outcome,
    };
    let body = SealedManifest {
        digest: prepared.published.digest.clone(),
        manifest: prepared.published.manifest.clone(),
    };
    let outcome = crate::command_client::execute(
        &prepared.canvas,
        contract,
        crate::command_client::SelectedCommand::ProofRecognize {
            manifest: &prepared.published,
            head: prepared.head,
        },
    );
    recognition_outcome(outcome, body)
}

fn prepare() -> Result<Prepared, Outcome<SealedManifest>> {
    let current = env::current_dir().map_err(|error| {
        environment(
            "release::seal-working-directory-unavailable",
            &error.to_string(),
        )
    })?;
    let repository = Repository::discover(&current)
        .map_err(|error| environment("release::seal-repository-unresolved", &error.to_string()))?;
    let state = repository
        .admit_state()
        .map_err(|error| crate::state_failure(&error).outcome())?;
    let mut stored = Vec::with_capacity(2);
    for target in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ] {
        let layout = proof::Layout::new(repository.worktree_root(), state.proof(), target);
        let payload = layout
            .read_payload()
            .map_err(|error| refused("release::seal-input-refused", &error))?;
        let admitted = proof::admit(&payload)
            .map_err(|error| refused("release::seal-payload-refused", &error))?;
        let receipt = layout
            .receipt_for(admitted.value())
            .map_err(|error| refused("release::seal-layout-refused", &error))?
            .read()
            .map_err(|error| refused("release::seal-input-refused", &error))?;
        stored.push(StoredProof { payload, receipt });
    }
    let inputs = stored
        .iter()
        .map(|proof| proof::ManifestInput::new(&proof.payload, &proof.receipt));
    let stage = repository.worktree_root().join("dist/stage");
    let mut publisher = proof::FileManifestPublisher::new(&stage);
    let published = proof::publish_manifest(inputs, &mut publisher)
        .map_err(|error| refused("release::seal-manifest-refused", &error))?;
    let app = crate::release_command::app_digest(repository.worktree_root())
        .map_err(|error| refused("release::seal-app-refused", &error))?;
    let admitted = proof::admit_proof_manifest(&stage, &app)
        .map_err(|error| refused("release::seal-admission-refused", &error))?;
    if admitted != published {
        return Err(refused(
            "release::seal-publication-moved",
            "published manifest did not read back byte-identically",
        ));
    }
    let head = journal::JournalReader::open(state.journal())
        .and_then(|reader| reader.cursor())
        .map_err(|error| environment("release::seal-journal-unavailable", &error.to_string()))?
        .ok_or_else(|| {
            environment(
                "release::seal-journal-empty",
                "proof recognition requires one current journal head",
            )
        })?;
    Ok(Prepared {
        published,
        canvas: state.canvas().to_path_buf(),
        head,
    })
}

fn recognition_outcome(
    outcome: Outcome<wire::intent::CommandAnswer>,
    body: SealedManifest,
) -> Outcome<SealedManifest> {
    match outcome {
        Outcome::Reply(reply) => match reply.body {
            wire::intent::CommandAnswer::Recognition(_) => Outcome::Reply(Reply {
                position: reply.position,
                body,
            }),
            _ => Outcome::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "release::seal-answer-shape".to_owned(),
                detail: "resident returned a non-recognition manifest-seal answer".to_owned(),
            }),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn refused<B>(reason: &str, error: &(impl std::fmt::Display + ?Sized)) -> Outcome<B> {
    Outcome::Refusal(Refusal::bare(wire::RefusalDiagnostic::new(
        reason.to_owned(),
        Some("sealed-release".to_owned()),
        Some("both canonical payloads and clean detached receipts".to_owned()),
        Some(error.to_string()),
    )))
}

fn environment<B>(reason: &str, detail: &str) -> Outcome<B> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
