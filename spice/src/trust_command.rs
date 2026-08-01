//! Operator trust reporting and exact-digest rulings.

use clap::ArgMatches;
use serde::Serialize;
use spice::config::LoadError;
use spice::repository::Repository;
use wire::intent::{CommandAnswer, TrustAnswer};
use wire::{
    ConfigSource, Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition,
    TrustState,
};

use crate::cli::Contract;

pub(crate) const SURFACE_NAME: &str = "configuration";

/// One securely acquired executable configuration surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Surface {
    digest: Digest,
    tracked: Document,
    local: Document,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Document {
    source: ConfigSource,
    location: String,
    text: Option<String>,
}

impl Surface {
    /// Acquire, parse, and digest the exact tracked and local policy documents.
    pub(crate) fn load(repository: &Repository) -> Result<Self, LoadError> {
        let loaded = spice::config::capture_repository(repository, &[])?;
        let tracked = Document {
            source: ConfigSource::Tracked,
            location: loaded.locations().tracked().display().to_string(),
            text: loaded.tracked().map(ToOwned::to_owned),
        };
        let local = Document {
            source: ConfigSource::Local,
            location: loaded.locations().local().display().to_string(),
            text: loaded.local().map(ToOwned::to_owned),
        };
        let digest = surface_digest(tracked.text.as_deref(), local.text.as_deref());
        Ok(Self {
            digest,
            tracked,
            local,
        })
    }

    /// Identity of the exact tracked plus local bytes.
    pub(crate) const fn digest(&self) -> &Digest {
        &self.digest
    }

    fn provenance(&self) -> Vec<PolicyProvenance> {
        [&self.tracked, &self.local]
            .into_iter()
            .map(|document| PolicyProvenance {
                source: document.source,
                location: document.location.clone(),
                present: document.text.is_some(),
                digest: document.text.as_ref().map(|text| {
                    let mut framed = Vec::new();
                    frame_document(&mut framed, document.source, Some(text));
                    Digest::compute(Domain::ConfigExecutableSurface, &framed)
                }),
            })
            .collect()
    }
}

fn surface_digest(tracked: Option<&str>, local: Option<&str>) -> Digest {
    let mut framed = Vec::new();
    frame_document(&mut framed, ConfigSource::Tracked, tracked);
    frame_document(&mut framed, ConfigSource::Local, local);
    Digest::compute(Domain::ConfigExecutableSurface, &framed)
}

fn frame_document(framed: &mut Vec<u8>, source: ConfigSource, text: Option<&str>) {
    framed.extend_from_slice(source.as_str().as_bytes());
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
}

/// Bare `spice trust` answer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TrustReport {
    surface: String,
    digest: Digest,
    standing: TrustState,
    provenance: Vec<PolicyProvenance>,
    rulings: Vec<TrustRuling>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct PolicyProvenance {
    source: ConfigSource,
    location: String,
    present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct TrustRuling {
    digest: Digest,
    standing: TrustState,
    current: bool,
}

/// Authenticate through the current resident and render its durable trust heads.
pub(crate) fn report(contract: &Contract) -> Outcome<TrustReport> {
    let repository = match current_repository() {
        Ok(repository) => repository,
        Err(outcome) => return outcome,
    };
    let epoch = match crate::command_client::authenticate(&repository, contract) {
        Ok(epoch) => epoch,
        Err(outcome) => return retag(outcome),
    };
    let surface = match Surface::load(&repository) {
        Ok(surface) => surface,
        Err(error) => return load_failure(&error),
    };
    let reader = match journal::JournalReader::open(repository.journal_path()) {
        Ok(reader) => reader,
        Err(error) => {
            return fault(
                FaultClass::Store,
                "trust::journal-unreadable",
                error.to_string(),
            );
        }
    };
    let (cursor, residency, heads) = match reader
        .read(|read| Ok((read.cursor()?, read.residency_head()?, read.trust_heads()?)))
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return fault(
                FaultClass::Store,
                "trust::journal-unreadable",
                error.to_string(),
            );
        }
    };
    let Some(residency) = residency else {
        return fault(
            FaultClass::Store,
            "trust::residency-missing",
            "the authenticated resident has no durable residency head".to_owned(),
        );
    };
    if residency.body().epoch != epoch {
        return refusal(
            "trust::epoch-moved",
            Some(epoch.to_string()),
            Some(residency.body().epoch.to_string()),
            "reconnect through the current resident discovery and retry",
        );
    }
    let Some(cursor) = cursor else {
        return fault(
            FaultClass::Store,
            "trust::cursor-missing",
            "the authenticated resident has no durable journal position".to_owned(),
        );
    };
    let standing = heads
        .iter()
        .find(|head| head.body().digest == *surface.digest())
        .map(|head| head.body().state);
    let Some(standing) = standing else {
        return refusal(
            "trust::digest-unknown",
            Some(surface.digest().to_string()),
            None,
            "restart the resident to pend the exact current policy surface before ruling",
        );
    };
    let current = match Surface::load(&repository) {
        Ok(current) => current,
        Err(error) => return load_failure(&error),
    };
    if current != surface {
        return refusal(
            "trust::surface-moved",
            Some(surface.digest().to_string()),
            Some(current.digest().to_string()),
            "retry against the current tracked and local policy documents",
        );
    }
    let rulings = heads
        .into_iter()
        .map(|head| TrustRuling {
            current: head.body().digest == *surface.digest(),
            digest: head.body().digest.clone(),
            standing: head.body().state,
        })
        .collect();
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(cursor),
        body: TrustReport {
            surface: SURFACE_NAME.to_owned(),
            digest: surface.digest().clone(),
            standing,
            provenance: surface.provenance(),
            rulings,
        },
    })
}

/// Apply one exact grant or revoke through the resident command path.
pub(crate) fn rule(matches: &ArgMatches, contract: &Contract, grant: bool) -> Outcome<TrustAnswer> {
    let digest = match matches
        .get_one::<String>("DIGEST")
        .expect("the generated trust syntax requires DIGEST")
        .parse::<Digest>()
    {
        Ok(digest) => digest,
        Err(error) => {
            return refusal(
                "trust::digest-invalid",
                Some("one canonical lowercase 64-character digest".to_owned()),
                Some(error.to_string()),
                "copy the exact digest from spice trust output",
            );
        }
    };
    let repository = match current_repository() {
        Ok(repository) => repository,
        Err(outcome) => return outcome,
    };
    let selected = if grant {
        crate::command_client::SelectedCommand::TrustGrant { digest: &digest }
    } else {
        crate::command_client::SelectedCommand::TrustRevoke { digest: &digest }
    };
    match crate::command_client::execute(&repository, contract, selected) {
        Outcome::Reply(reply) => match reply.body {
            CommandAnswer::Trust(answer) => Outcome::Reply(Reply {
                position: reply.position,
                body: answer,
            }),
            answer => fault(
                FaultClass::Protocol,
                "trust::answer-shape",
                format!("expected a trust answer, received {answer:?}"),
            ),
        },
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
    }
}

fn current_repository<B>() -> Result<Repository, Outcome<B>> {
    let current = std::env::current_dir().map_err(|error| {
        fault(
            FaultClass::Environment,
            "trust::working-directory-unavailable",
            error.to_string(),
        )
    })?;
    Repository::discover(&current).map_err(|error| {
        fault(
            FaultClass::Environment,
            "trust::repository-unresolved",
            error.to_string(),
        )
    })
}

pub(crate) fn load_failure<B>(error: &LoadError) -> Outcome<B> {
    if let Some(repository) = error.repository() {
        return fault(
            FaultClass::Environment,
            "trust::repository-unresolved",
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
        fault(FaultClass::Environment, "trust::policy-unreadable", detail)
    } else {
        refusal(
            "trust::policy-refused",
            Some("tracked and local values admitted by the typed registry".to_owned()),
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
        witness: Some("executable-policy".to_owned()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_identity_distinguishes_source_presence_and_bytes() {
        let absent = surface_digest(None, None);
        let tracked_empty = surface_digest(Some(""), None);
        let local_empty = surface_digest(None, Some(""));
        let tracked = surface_digest(Some("policy = 1"), None);
        let local = surface_digest(None, Some("policy = 1"));

        assert_ne!(absent, tracked_empty);
        assert_ne!(tracked_empty, local_empty);
        assert_ne!(tracked, local);
    }
}
