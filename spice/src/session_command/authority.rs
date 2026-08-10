use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::Entry;
use journal::{JournalRead, JournalReader};
use spice::repository::Repository;
use wire::{Actor, ResidencyHead};

use super::model::{
    AuthorityFoldView, AuthorityUnknown, BriefingAuthorities, DecisionPriorityView, HeldClaimView,
    HeldWorkView, OutstandingAskView,
};
use super::{Environment, source};

pub(crate) fn observe(
    sources: &[source::ResolvedSource],
    environment: &Environment,
) -> BriefingAuthorities {
    let mut briefing = empty_briefing_authorities();

    let thread = exact_thread(sources, environment, &mut briefing);
    briefing.held_work.thread.clone_from(&thread);

    observe_current_authorities(thread.as_deref(), &mut briefing);
    briefing
}

fn empty_briefing_authorities() -> BriefingAuthorities {
    BriefingAuthorities {
        fold: AuthorityFoldView {
            mode: "bounded-read-only".to_owned(),
            probes: vec![
                "transcript-thread".to_owned(),
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

fn observe_current_authorities(thread: Option<&str>, briefing: &mut BriefingAuthorities) {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            unknown(briefing, "repository", "current-directory", &error);
            unavailable_probes(briefing, "current directory is unavailable");
            return;
        }
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            unknown(briefing, "repository", "discover", &error);
            unavailable_probes(briefing, "repository discovery failed");
            return;
        }
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => {
            unknown(briefing, "repository", "admit-state", &error);
            unavailable_probes(briefing, "repository state is unavailable");
            return;
        }
    };
    let projects = match spice::config::load_repository(&repository, &[]) {
        Ok(config) => Some(config.root().tasks().project_policy().clone()),
        Err(error) => {
            unknown(briefing, "configuration", "load-repository", &error);
            None
        }
    };
    let reader = match JournalReader::open(state.journal()) {
        Ok(reader) => reader,
        Err(error) => {
            unknown(briefing, "journal", "open-read-only", &error);
            unavailable_probes(briefing, "read-only journal open failed");
            return;
        }
    };
    match reader.read_with(|read| read_authorities(read, thread, projects.as_ref())) {
        Ok(observed) => apply_observed(briefing, thread, observed),
        Err(error) => {
            unknown(briefing, "journal", "read-transaction", &error);
            unavailable_probes(briefing, "journal read transaction failed");
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
    thread: Option<&str>,
    projects: Option<&board::ProjectPolicy>,
) -> Result<ObservedAuthorities, std::convert::Infallible> {
    let residency = read
        .residency_head()
        .map_err(AuthorityReadError::Journal)
        .and_then(|head| {
            head.map(journal::JournalHead::into_body)
                .ok_or(AuthorityReadError::ResidencyMissing)
        });
    let lane = thread.and_then(|thread| {
        residency
            .as_ref()
            .ok()
            .and_then(|residency| lane_for_thread(residency, thread))
            .cloned()
    });
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

fn apply_observed(
    briefing: &mut BriefingAuthorities,
    thread: Option<&str>,
    observed: ObservedAuthorities,
) {
    if let Some(error) = observed.residency_error {
        unknown(briefing, "journal", "residency-head", &error);
        unavailable_probe(
            briefing,
            "steering",
            "pending-thread-lane",
            "the current residency head is unavailable",
        );
        unavailable_probe(
            briefing,
            "board",
            "exact-actor-claim",
            "the current residency head is unavailable",
        );
        unavailable_probe(
            briefing,
            "board",
            "active-peer-count",
            "the current residency head is unavailable",
        );
        return;
    }
    let Some(lane) = observed.lane else {
        if let Some(thread) = thread {
            unavailable_probe(
                briefing,
                "steering",
                "thread-lane",
                &format!("no current lane session matches thread {thread}"),
            );
            unavailable_probe(
                briefing,
                "board",
                "thread-actor",
                &format!("no current lane session matches thread {thread}"),
            );
        }
        return;
    };
    briefing.held_work.lane = Some(lane.lane.to_string());

    if let Some(ledger) = observed.ledger {
        match ledger {
            Ok(ledger) => {
                briefing.asks = ledger
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
            Err(error) => unknown(briefing, "steering", "pending-thread-lane", &error),
        }
    }

    if let Some(own_claim) = observed.own_claim {
        match own_claim {
            Ok(Some(entry)) => briefing.held_work.own_claim = Some(held_claim(&entry)),
            Ok(None) => {}
            Err(error) => unknown(briefing, "board", "exact-actor-claim", &error),
        }
    }
    if let Some(peer_claims) = observed.peer_claims {
        match peer_claims {
            Ok(count) => briefing.held_work.peer_claims = Some(count),
            Err(error) => unknown(briefing, "board", "active-peer-count", &error),
        }
    }
}

fn unavailable_probes(briefing: &mut BriefingAuthorities, detail: &str) {
    unavailable_probe(briefing, "steering", "pending-thread-lane", detail);
    unavailable_probe(briefing, "board", "exact-actor-claim", detail);
    unavailable_probe(briefing, "board", "active-peer-count", detail);
}

fn unavailable_probe(
    briefing: &mut BriefingAuthorities,
    authority: &str,
    probe: &str,
    detail: &str,
) {
    briefing.unknowns.push(AuthorityUnknown {
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

fn exact_thread(
    sources: &[source::ResolvedSource],
    environment: &Environment,
    briefing: &mut BriefingAuthorities,
) -> Option<String> {
    let mut threads = sources
        .iter()
        .filter_map(|source| source.thread.clone())
        .collect::<BTreeSet<_>>();
    if threads.is_empty() {
        threads.extend(environment.vendors.iter().filter_map(|vendor| {
            vendor
                .identity
                .as_ref()
                .and_then(|identity| source::canonical_thread(&identity.to_string_lossy()))
        }));
    }
    match threads.len() {
        1 => threads.into_iter().next(),
        0 => {
            briefing.unknowns.push(AuthorityUnknown {
                authority: "transcript".to_owned(),
                probe: "exact-thread".to_owned(),
                detail: "no ambient or resolved transcript thread identity is available".to_owned(),
            });
            None
        }
        _ => {
            briefing.unknowns.push(AuthorityUnknown {
                authority: "transcript".to_owned(),
                probe: "exact-thread".to_owned(),
                detail: format!(
                    "more than one transcript thread is in scope: {}",
                    threads.into_iter().collect::<Vec<_>>().join(",")
                ),
            });
            None
        }
    }
}

fn unknown(
    briefing: &mut BriefingAuthorities,
    authority: &str,
    probe: &str,
    error: &(dyn fmt::Display + '_),
) {
    briefing.unknowns.push(AuthorityUnknown {
        authority: authority.to_owned(),
        probe: probe.to_owned(),
        detail: error.to_string(),
    });
}
