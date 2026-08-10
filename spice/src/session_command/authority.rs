use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use board::Entry;
use journal::{JournalRead, JournalReader};
use spice::repository::Repository;
use wire::{Actor, ClaimDisposition, ResidencyHead};

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

    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            unknown(&mut briefing, "repository", "current-directory", &error);
            return briefing;
        }
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            unknown(&mut briefing, "repository", "discover", &error);
            return briefing;
        }
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => {
            unknown(&mut briefing, "repository", "admit-state", &error);
            return briefing;
        }
    };
    let config = match spice::config::load_repository(&repository, &[]) {
        Ok(config) => config,
        Err(error) => {
            unknown(&mut briefing, "configuration", "load-repository", &error);
            return briefing;
        }
    };
    let projects = config.root().tasks().project_policy();
    let reader = match JournalReader::open(state.journal()) {
        Ok(reader) => reader,
        Err(error) => {
            unknown(&mut briefing, "journal", "open-read-only", &error);
            return briefing;
        }
    };

    let observed = match reader.read_with(|read| read_authorities(read, &projects)) {
        Ok(observed) => observed,
        Err(error) => {
            unknown(&mut briefing, "journal", "read-snapshot", &error);
            return briefing;
        }
    };

    let Some(thread) = thread else {
        return briefing;
    };
    let Some(lane) = lane_for_thread(&observed.residency, &thread) else {
        briefing.unknowns.push(AuthorityUnknown {
            authority: "steering".to_owned(),
            probe: "thread-lane".to_owned(),
            detail: format!("no current lane session matches thread {thread}"),
        });
        return briefing;
    };
    briefing.held_work.lane = Some(lane.lane.to_string());

    briefing.asks = observed
        .ledger
        .pending(lane.lane)
        .into_iter()
        .map(|directive| OutstandingAskView {
            lane: lane.lane.to_string(),
            directive: directive.directive().to_string(),
            key: directive.key().to_string(),
            stage: format!("{:?}", directive.stage()).to_ascii_lowercase(),
            text: directive.text().to_owned(),
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

    let actor = Actor::Lane(lane.lane);
    let mut peer_claims = 0_usize;
    for entry in &observed.entries {
        let Some(claim) = entry.claim() else {
            continue;
        };
        if claim.disposition != ClaimDisposition::Active {
            continue;
        }
        if claim.actor == actor {
            if briefing.held_work.own_claim.is_none() {
                briefing.held_work.own_claim = Some(held_claim(entry));
            }
        } else {
            peer_claims += 1;
        }
    }
    briefing.held_work.peer_claims = Some(peer_claims);
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
                "journal-snapshot".to_owned(),
                "steering-ledger".to_owned(),
                "board-claims".to_owned(),
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
    residency: ResidencyHead,
    ledger: steer::Ledger,
    entries: Vec<Entry>,
}

#[derive(Debug)]
enum AuthorityReadError {
    ResidencyMissing,
    Journal(journal::GuardError),
    Snapshot(journal::SnapshotError),
    Steering(steer::directives::ReplayError),
    Board(board::TaskError),
}

impl fmt::Display for AuthorityReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResidencyMissing => {
                formatter.write_str("journal snapshot has no current residency head")
            }
            Self::Journal(error) => write!(formatter, "journal heads are unavailable: {error}"),
            Self::Snapshot(error) => write!(formatter, "fact snapshot is unavailable: {error}"),
            Self::Steering(error) => write!(formatter, "steering ledger is invalid: {error}"),
            Self::Board(error) => write!(formatter, "board entry is invalid: {error}"),
        }
    }
}

impl Error for AuthorityReadError {}

fn read_authorities(
    read: &JournalRead<'_>,
    projects: &board::ProjectPolicy,
) -> Result<ObservedAuthorities, AuthorityReadError> {
    let facts = read.fact_snapshot().map_err(AuthorityReadError::Snapshot)?;
    let ledger = steer::Ledger::from_snapshot(&facts).map_err(AuthorityReadError::Steering)?;
    let residency = read
        .residency_head()
        .map_err(AuthorityReadError::Journal)?
        .ok_or(AuthorityReadError::ResidencyMissing)?
        .into_body();
    let entries = read
        .task_heads()
        .map_err(AuthorityReadError::Journal)?
        .into_iter()
        .map(|heads| {
            let (statement, workflow, claim, lease) = heads.into_bodies();
            Entry::new(statement, workflow, claim, lease, projects)
                .map_err(AuthorityReadError::Board)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ObservedAuthorities {
        residency,
        ledger,
        entries,
    })
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
