use super::*;
use crate::{Bus, CommitOutcome};
use journal::crossing::{Actor, Instant, Producer};
use journal::{FactCause, JournalWrite};

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .unwrap()
}

fn write() -> JournalWrite {
    JournalWrite::new(
        Actor::Spice,
        format!("lane:{}", uuid(1)).parse().unwrap(),
        Instant::new("2030-04-12T18:44:09Z".parse().unwrap()).unwrap(),
        Producer::ResidentRecognition,
    )
}

fn declaration(occupant: u64, offset: u64) -> body::LaneSessionDeclared {
    body::LaneSessionDeclared {
        lane: uuid(1),
        occupant: uuid(occupant),
        worktree: format!("/worktrees/{occupant}"),
        vendor: "codex".parse().unwrap(),
        launch: None,
        epoch: uuid(8),
        stream: "session".to_owned(),
        identity: format!("session-{occupant}"),
        member: 0,
        offset,
        length: 1,
    }
}

fn ingestion(source: u64, from: u64, to: u64) -> body::JournalIngested {
    body::JournalIngested {
        lane: uuid(1),
        vendor: "codex".parse().unwrap(),
        stream: "session".to_owned(),
        source: uuid(source),
        supersedes: None,
        from: body::TranscriptPosition {
            member: 0,
            offset: from,
        },
        to: body::TranscriptPosition {
            member: 0,
            offset: to,
        },
        usage: Vec::new(),
        non_usage_records: 1,
        residue_records: 0,
    }
}

#[test]
fn sessions_bind_only_to_the_first_subsequent_matching_ingestion() {
    let mut journal = Journal::in_memory().unwrap();
    let mut facts = write();
    facts.fact(ingestion(9, 0, 30), FactCause::Root).unwrap();
    facts.fact(declaration(3, 10), FactCause::Root).unwrap();
    facts.fact(declaration(4, 20), FactCause::Root).unwrap();
    for mismatch in 0..3 {
        let mut batch = ingestion(9, 0, 30);
        match mismatch {
            0 => batch.lane = uuid(2),
            1 => batch.vendor = "claude".parse().unwrap(),
            _ => batch.stream = "stderr".to_owned(),
        }
        facts.fact(batch, FactCause::Root).unwrap();
    }
    facts.fact(ingestion(6, 10, 20), FactCause::Root).unwrap();
    facts.fact(declaration(5, 10), FactCause::Root).unwrap();
    facts.fact(ingestion(7, 0, 30), FactCause::Root).unwrap();
    journal.apply(facts).unwrap();
    let sessions = TranscriptSessions::from_snapshot(&journal.fact_snapshot().unwrap());
    assert!(sessions.at(uuid(1), uuid(9), 0, 20).is_none());
    assert!(sessions.at(uuid(1), uuid(6), 0, 9).is_none());
    assert_eq!(
        sessions.at(uuid(1), uuid(6), 0, 20).unwrap().occupant,
        uuid(3)
    );
    assert_eq!(
        sessions.at(uuid(1), uuid(7), 0, 10).unwrap().occupant,
        uuid(5)
    );
    assert_eq!(
        sessions.at(uuid(1), uuid(7), 0, 20).unwrap().occupant,
        uuid(4)
    );
}

#[test]
fn reader_sessions_advance_across_recovery_pages_and_later_commits() {
    let mut journal = Journal::in_memory().unwrap();
    let mut facts = write();
    let count = journal::MAX_RETAINED_FACT_PAGE_ROWS as u64 + 2;
    for offset in 0..count {
        facts
            .fact(declaration(offset + 100, offset), FactCause::Root)
            .unwrap();
    }
    journal.apply(facts).unwrap();
    let bus = Bus::start(journal).unwrap();
    let reader = bus.reader();
    assert!(
        reader
            .transcript_session(uuid(1), uuid(6), 0, count - 1)
            .unwrap()
            .is_none()
    );
    let mut facts = write();
    facts.fact(ingestion(6, 0, count), FactCause::Root).unwrap();
    assert!(matches!(
        bus.commit(facts).unwrap(),
        CommitOutcome::Committed(_)
    ));
    for current in [reader.clone(), bus.reader()] {
        for offset in [0, count - 1] {
            let session = current
                .transcript_session(uuid(1), uuid(6), 0, offset)
                .unwrap()
                .unwrap();
            assert_eq!(session.occupant, uuid(offset + 100));
            assert_eq!(
                session.worktree,
                PathBuf::from(format!("/worktrees/{}", offset + 100))
            );
        }
    }
    // A later declaration at the same position wins within the same source.
    let mut facts = write();
    facts.fact(declaration(5, 0), FactCause::Root).unwrap();
    facts.fact(ingestion(6, 0, 1), FactCause::Root).unwrap();
    assert!(matches!(
        bus.commit(facts).unwrap(),
        CommitOutcome::Committed(_)
    ));
    assert_eq!(
        reader
            .transcript_session(uuid(1), uuid(6), 0, 0)
            .unwrap()
            .unwrap()
            .occupant,
        uuid(5)
    );
    bus.shutdown().unwrap();
}
