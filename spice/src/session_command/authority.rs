use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::Entry;
use journal::{JournalRead, JournalReader};
use spice::repository::Repository;
use wire::{Actor, ResidencyHead};

use super::model::{
    AuthorityFoldView, AuthorityUnknown, DecisionPriorityView, HeldClaimView, HeldWorkView,
    OutstandingAskView, RecallAuthorities,
};
use super::{Environment, source};

pub(crate) fn observe(
    sources: &[source::ResolvedSource],
    environment: &Environment,
) -> RecallAuthorities {
    let mut recall = empty_recall_authorities();

    let thread = live_thread(environment, &mut recall);
    recall.held_work.thread.clone_from(&thread);
    observe_transcript_threads(sources, thread.as_deref(), &mut recall);

    if let Some(thread) = thread.as_deref() {
        observe_current_authorities(thread, &mut recall);
    } else {
        unavailable_probes(
            &mut recall,
            "no admitted live vendor thread identity is available",
        );
    }
    recall
}

fn empty_recall_authorities() -> RecallAuthorities {
    RecallAuthorities {
        fold: AuthorityFoldView {
            mode: "bounded-read-only".to_owned(),
            probes: vec![
                "live-vendor-thread".to_owned(),
                "selected-transcript-threads".to_owned(),
                "repository-state".to_owned(),
                "configuration".to_owned(),
                "journal-residency-head".to_owned(),
                "journal-steering-index".to_owned(),
                "journal-claim-index".to_owned(),
            ],
            writes: Vec::new(),
        },
        priority: DecisionPriorityView {
            order: vec![
                "outstanding-asks".to_owned(),
                "held-work".to_owned(),
                "transcript-deliveries".to_owned(),
                "transcript-working-set".to_owned(),
                "ambient-posture".to_owned(),
            ],
            end_pruning_preserves: vec!["outstanding-asks".to_owned(), "held-work".to_owned()],
        },
        asks: Vec::new(),
        held_work: HeldWorkView {
            thread: None,
            lane: None,
            own_claim: None,
            peer_claims: None,
        },
        unknowns: Vec::new(),
    }
}

struct ObservedAuthorities {
    lane: Option<wire::LaneResidency>,
    residency_error: Option<AuthorityReadError>,
    ledger: Option<Result<steer::Ledger, AuthorityReadError>>,
    own_claim: Option<Result<Option<Entry>, AuthorityReadError>>,
    peer_claims: Option<Result<usize, AuthorityReadError>>,
}

fn observe_current_authorities(thread: &str, recall: &mut RecallAuthorities) {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            unknown(recall, "repository", "current-directory", &error);
            unavailable_probes(recall, "current directory is unavailable");
            return;
        }
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            unknown(recall, "repository", "discover", &error);
            unavailable_probes(recall, "repository discovery failed");
            return;
        }
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => {
            unknown(recall, "repository", "admit-state", &error);
            unavailable_probes(recall, "repository state is unavailable");
            return;
        }
    };
    let projects = match spice::config::load_repository(&repository, &[]) {
        Ok(config) => Some(config.root().tasks().project_policy().clone()),
        Err(error) => {
            unknown(recall, "configuration", "load-repository", &error);
            None
        }
    };
    let reader = match JournalReader::open(state.journal()) {
        Ok(reader) => reader,
        Err(error) => {
            unknown(recall, "journal", "open-read-only", &error);
            unavailable_probes(recall, "read-only journal open failed");
            return;
        }
    };
    match reader.read_with(|read| read_authorities(read, thread, projects.as_ref())) {
        Ok(observed) => apply_observed(recall, thread, observed),
        Err(error) => {
            unknown(recall, "journal", "read-transaction", &error);
            unavailable_probes(recall, "journal read transaction failed");
        }
    }
}

#[derive(Debug)]
enum AuthorityReadError {
    ResidencyMissing,
    ProjectPolicyUnavailable,
    Journal(journal::GuardError),
    Steering(steer::directives::Refusal),
    Board(board::TaskError),
}

impl fmt::Display for AuthorityReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResidencyMissing => {
                formatter.write_str("journal snapshot has no current residency head")
            }
            Self::ProjectPolicyUnavailable => {
                formatter.write_str("repository task project policy is unavailable")
            }
            Self::Journal(error) => write!(formatter, "journal heads are unavailable: {error}"),
            Self::Steering(error) => {
                write!(formatter, "indexed steering heads are invalid: {error}")
            }
            Self::Board(error) => write!(formatter, "board entry is invalid: {error}"),
        }
    }
}

impl Error for AuthorityReadError {}

fn read_authorities(
    read: &JournalRead<'_>,
    thread: &str,
    projects: Option<&board::ProjectPolicy>,
) -> Result<ObservedAuthorities, std::convert::Infallible> {
    let residency = read
        .residency_head()
        .map_err(AuthorityReadError::Journal)
        .and_then(|head| {
            head.map(journal::JournalHead::into_body)
                .ok_or(AuthorityReadError::ResidencyMissing)
        });
    let lane = residency
        .as_ref()
        .ok()
        .and_then(|residency| lane_for_thread(residency, thread))
        .cloned();
    let Some(lane) = lane else {
        return Ok(ObservedAuthorities {
            lane: None,
            residency_error: residency.err(),
            ledger: None,
            own_claim: None,
            peer_claims: None,
        });
    };
    let actor = Actor::Lane(lane.lane);
    let ledger = read
        .pending_steering(Some(lane.lane))
        .map_err(AuthorityReadError::Journal)
        .and_then(|heads| {
            steer::Ledger::from_authority_heads(heads.iter().map(|head| head.body()))
                .map_err(AuthorityReadError::Steering)
        });
    let own_claim = read
        .active_claim_for_actor(&actor)
        .map_err(AuthorityReadError::Journal)
        .and_then(|heads| {
            heads.map_or(Ok(None), |heads| {
                let Some(projects) = projects else {
                    return Err(AuthorityReadError::ProjectPolicyUnavailable);
                };
                let (statement, workflow, claim, lease) = heads.into_bodies();
                Entry::new(statement, workflow, claim, lease, projects)
                    .map_err(AuthorityReadError::Board)
                    .map(Some)
            })
        });
    let peer_claims = read
        .active_claim_count_excluding(&actor)
        .map_err(AuthorityReadError::Journal);
    Ok(ObservedAuthorities {
        lane: Some(lane),
        residency_error: residency.err(),
        ledger: Some(ledger),
        own_claim: Some(own_claim),
        peer_claims: Some(peer_claims),
    })
}

fn apply_observed(recall: &mut RecallAuthorities, thread: &str, observed: ObservedAuthorities) {
    if let Some(error) = observed.residency_error {
        unknown(recall, "journal", "residency-head", &error);
        unavailable_probe(
            recall,
            "steering",
            "pending-thread-lane",
            "the current residency head is unavailable",
        );
        unavailable_probe(
            recall,
            "board",
            "exact-actor-claim",
            "the current residency head is unavailable",
        );
        unavailable_probe(
            recall,
            "board",
            "active-peer-count",
            "the current residency head is unavailable",
        );
        return;
    }
    let Some(lane) = observed.lane else {
        unavailable_probe(
            recall,
            "steering",
            "thread-lane",
            &format!("no current lane session matches live thread {thread}"),
        );
        unavailable_probe(
            recall,
            "board",
            "thread-actor",
            &format!("no current lane session matches live thread {thread}"),
        );
        unavailable_probe(
            recall,
            "board",
            "active-peer-count",
            &format!("no current lane session matches live thread {thread}"),
        );
        return;
    };
    recall.held_work.lane = Some(lane.lane.to_string());

    if let Some(ledger) = observed.ledger {
        match ledger {
            Ok(ledger) => {
                recall.asks = ledger
                    .pending(lane.lane)
                    .into_iter()
                    .map(|directive| OutstandingAskView {
                        lane: lane.lane.to_string(),
                        directive: directive.directive().to_string(),
                        key: directive.key().to_string(),
                        stage: format!("{:?}", directive.stage()).to_ascii_lowercase(),
                        text: directive.text().to_owned(),
                        attachments: directive.attachments().to_vec(),
                        authored: directive
                            .published()
                            .map_or_else(String::new, |at| at.to_string()),
                        expires: directive.expires().to_string(),
                        preview: directive
                            .text()
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .map(|line| line.trim().to_owned()),
                    })
                    .collect();
            }
            Err(error) => unknown(recall, "steering", "pending-thread-lane", &error),
        }
    }

    if let Some(own_claim) = observed.own_claim {
        match own_claim {
            Ok(Some(entry)) => recall.held_work.own_claim = Some(held_claim(&entry)),
            Ok(None) => {}
            Err(error) => unknown(recall, "board", "exact-actor-claim", &error),
        }
    }
    if let Some(peer_claims) = observed.peer_claims {
        match peer_claims {
            Ok(count) => recall.held_work.peer_claims = Some(count),
            Err(error) => unknown(recall, "board", "active-peer-count", &error),
        }
    }
}

fn unavailable_probes(recall: &mut RecallAuthorities, detail: &str) {
    unavailable_probe(recall, "steering", "pending-thread-lane", detail);
    unavailable_probe(recall, "board", "exact-actor-claim", detail);
    unavailable_probe(recall, "board", "active-peer-count", detail);
}

fn unavailable_probe(recall: &mut RecallAuthorities, authority: &str, probe: &str, detail: &str) {
    recall.unknowns.push(AuthorityUnknown {
        authority: authority.to_owned(),
        probe: probe.to_owned(),
        detail: detail.to_owned(),
    });
}

fn held_claim(entry: &Entry) -> HeldClaimView {
    HeldClaimView {
        handle: entry.handle().to_string(),
        project: entry.statement().project.clone(),
        phase: entry.workflow().phase.to_string(),
        status: entry.workflow().status.clone(),
        title: entry.statement().title.clone(),
        lease_expiry: entry.lease().map(|lease| lease.expiry.to_string()),
    }
}

fn lane_for_thread<'a>(
    residency: &'a ResidencyHead,
    thread: &str,
) -> Option<&'a wire::LaneResidency> {
    residency.lanes.iter().find(|lane| {
        lane.session.as_ref().is_some_and(|session| {
            source::canonical_thread(&session.identity)
                .as_deref()
                .is_some_and(|identity| identity == thread)
        })
    })
}

fn live_thread(environment: &Environment, recall: &mut RecallAuthorities) -> Option<String> {
    let populated = environment
        .vendors
        .iter()
        .filter(|vendor| vendor.identity.is_some())
        .collect::<Vec<_>>();
    let vendor = if let Some(preferred) = environment.preferred_vendor.as_deref() {
        let preferred = preferred.to_string_lossy();
        let Some(vendor) = environment
            .vendors
            .iter()
            .find(|vendor| vendor.vendor.name() == preferred)
        else {
            unavailable_probe(
                recall,
                "environment",
                "live-thread-identity",
                &format!(
                    "SPICE_AGENT_VENDOR names unknown vendor {preferred:?}; transcript selectors cannot replace the admitted live vendor identity"
                ),
            );
            return None;
        };
        if vendor.identity.is_none() {
            unavailable_probe(
                recall,
                "environment",
                "live-thread-identity",
                &format!(
                    "SPICE_AGENT_VENDOR selects {} but {} is absent; another vendor identity cannot replace it",
                    vendor.vendor.name(),
                    vendor.vendor.thread_id_environment()
                ),
            );
            return None;
        }
        vendor
    } else {
        match populated.as_slice() {
            [vendor] => *vendor,
            [] => {
                unavailable_probe(
                    recall,
                    "environment",
                    "live-thread-identity",
                    "no live vendor thread identity is present; transcript selectors are evidence, not current-actor credentials",
                );
                return None;
            }
            vendors => {
                unavailable_probe(
                    recall,
                    "environment",
                    "live-thread-identity",
                    &format!(
                        "multiple live vendor identities are present without SPICE_AGENT_VENDOR: {}; current authority remains unknown",
                        vendors
                            .iter()
                            .map(|vendor| vendor.vendor.thread_id_environment())
                            .collect::<Vec<_>>()
                            .join(",")
                    ),
                );
                return None;
            }
        }
    };
    let raw = vendor
        .identity
        .as_deref()
        .expect("the selected vendor environment carries an identity")
        .to_string_lossy();
    match source::canonical_thread(&raw) {
        Some(thread) => Some(thread),
        None => {
            unavailable_probe(
                recall,
                "environment",
                "live-thread-identity",
                &format!(
                    "{} contains malformed live vendor identity {raw:?}; transcript selectors cannot replace it",
                    vendor.vendor.thread_id_environment()
                ),
            );
            None
        }
    }
}

fn observe_transcript_threads(
    sources: &[source::ResolvedSource],
    live_thread: Option<&str>,
    recall: &mut RecallAuthorities,
) {
    let selected = sources
        .iter()
        .filter_map(|source| source.thread.clone())
        .collect::<BTreeSet<_>>();
    if selected.len() > 1 {
        unavailable_probe(
            recall,
            "transcript",
            "selected-thread-identity",
            &format!(
                "more than one transcript thread is selected as evidence: {}; current authority remains bound only to the live vendor identity",
                selected.iter().cloned().collect::<Vec<_>>().join(",")
            ),
        );
    }
    let Some(live_thread) = live_thread else {
        return;
    };
    let foreign = selected
        .iter()
        .filter(|thread| thread.as_str() != live_thread)
        .cloned()
        .collect::<Vec<_>>();
    if !foreign.is_empty() {
        unavailable_probe(
            recall,
            "transcript",
            "live-thread-mismatch",
            &format!(
                "selected transcript thread evidence {} differs from live thread {live_thread}; transcript evidence is preserved, but steering and held-work authority remain bound to the live thread",
                foreign.join(",")
            ),
        );
    }
}

fn unknown(
    recall: &mut RecallAuthorities,
    authority: &str,
    probe: &str,
    error: &(dyn fmt::Display + '_),
) {
    recall.unknowns.push(AuthorityUnknown {
        authority: authority.to_owned(),
        probe: probe.to_owned(),
        detail: error.to_string(),
    });
}
