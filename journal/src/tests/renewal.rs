use rusqlite::params;
use wire::{LaneRenewalRecord, LaneRenewalState, RenewalIdentity};

use super::*;

const LINEAGE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
const PREDECESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
const SUCCESSOR: &str = "018f82d4-6f59-7d06-92e3-bce913116304";

fn predecessor() -> RenewalIdentity {
    RenewalIdentity {
        occupant: uuid(PREDECESSOR),
        session: "session-predecessor".to_owned(),
        thread: "thread-predecessor".to_owned(),
    }
}

fn successor() -> RenewalIdentity {
    RenewalIdentity {
        occupant: uuid(SUCCESSOR),
        session: "session-successor".to_owned(),
        thread: "thread-successor".to_owned(),
    }
}

fn states() -> Vec<LaneRenewalState> {
    vec![
        LaneRenewalState::Requested {
            predecessor: predecessor(),
            team_slot: 0,
        },
        LaneRenewalState::HandoffPending {
            predecessor: predecessor(),
            team_slot: 0,
        },
        LaneRenewalState::SuccessorStarted {
            predecessor: predecessor(),
            successor: successor(),
            team_slot: 0,
        },
        LaneRenewalState::LaunchRefused {
            predecessor: predecessor(),
            team_slot: 0,
            reason: "selected vendor refused launch".to_owned(),
        },
    ]
}

#[test]
fn renewal_absence_and_every_durable_state_replay_after_reopen() {
    let scratch = Scratch::new();
    let path = scratch.path().join("renewal-replay.sqlite3");
    let mut journal = authority_file_fixture(&path);

    let initial = journal
        .read(|read| read.residency_head())
        .expect("initial residency reads")
        .expect("initial residency exists");
    assert!(initial.body().renewals.is_empty());

    for state in states() {
        let expected = LaneRenewalRecord {
            lineage_id: uuid(LINEAGE),
            lane: uuid(LANE),
            revision: 41,
            state,
        };
        let mut head = residency_head();
        head.topology_revision = 41;
        head.renewals = vec![expected.clone()];
        journal
            .connection
            .execute(
                "UPDATE residency_head SET body = ?1 WHERE singleton = 1",
                params![wire::encode(&head).expect("renewal head encodes")],
            )
            .expect("renewal head persists");
        drop(journal);

        journal = Journal::open(&path).expect("renewal journal reopens");
        let replayed = journal
            .read(|read| read.residency_head())
            .expect("replayed residency reads")
            .expect("replayed residency exists");
        assert_eq!(replayed.body().renewals, vec![expected]);
        journal
            .verify()
            .expect("replayed renewal authority verifies");
    }
}
