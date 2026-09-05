use super::*;
use crate::{FactCause, JournalGuard, JournalWrite};

fn boundary() -> wire::body::LaneCompacted {
    wire::body::LaneCompacted {
        lane: uuid("018f82d4-6f59-7d06-92e3-bce913116301"),
        occupant: uuid("018f82d4-6f59-7d06-92e3-bce913116302"),
        stream: "session:fixture".to_owned(),
        member: (1_u64 << 63) | 77,
        offset: 400,
        length: 32,
    }
}

fn proposal(producer: wire::Producer) -> JournalWrite {
    JournalWrite::new(
        Actor::Spice,
        "resident:018f82d4-6f59-7d06-92e3-bce913116303"
            .parse()
            .expect("fixture origin"),
        "2026-09-05T08:00:00.000Z".parse().expect("fixture instant"),
        producer,
    )
}

#[test]
fn compaction_recovery_reads_only_compactions_and_matches_the_complete_boundary() {
    let mut journal = authority_fixture();
    let expected = boundary();
    let mut write = proposal(wire::Producer::ResidentRecognition);
    write
        .fact(expected.clone(), FactCause::Root)
        .expect("compaction fact");
    journal.apply(write).expect("compaction commits");
    let mut write = proposal(wire::Producer::CommandDoor);
    // Unrelated history must not be decoded for each recovered boundary.
    for _ in 0..2_048 {
        write
            .fact(
                wire::body::UpSeated {
                    epoch: uuid("018f82d4-6f59-7d06-92e3-bce913116303"),
                },
                FactCause::Root,
            )
            .expect("unrelated fact");
    }
    journal.apply(write).expect("recovery history commits");
    reset_indexed_read_metrics();
    assert!(
        journal
            .read(|read| read.lane_compaction_recorded(&expected))
            .unwrap()
    );
    let metrics = indexed_read_metrics();
    assert_eq!(metrics.queries, 1);
    assert_eq!(metrics.rows, 1);
    assert!(metrics.decoded_bytes > 0);

    for field in 0..6 {
        let mut other = expected.clone();
        match field {
            0 => other.lane = other.occupant,
            1 => other.occupant = other.lane,
            2 => other.stream.push_str("-other"),
            3 => other.member += 1,
            4 => other.offset += 1,
            5 => other.length += 1,
            _ => unreachable!(),
        }
        assert!(
            !journal
                .read(|read| read.lane_compaction_recorded(&other))
                .unwrap()
        );
    }

    let mut duplicate = proposal(wire::Producer::ResidentRecognition);
    let guarded = expected.clone();
    duplicate.guard(JournalGuard::new(move |read| {
        if read.lane_compaction_recorded(&guarded)? {
            Err(crate::GuardError::refused("lane::compaction-recorded"))
        } else {
            Ok(())
        }
    }));
    duplicate
        .fact(expected, FactCause::Root)
        .expect("duplicate proposal");
    let cursor = journal.read(|read| read.cursor()).unwrap();
    journal
        .apply(duplicate)
        .expect_err("transaction guard rejects a duplicate");
    assert_eq!(journal.read(|read| read.cursor()).unwrap(), cursor);
}

#[test]
fn compaction_recovery_refuses_noncanonical_compaction_evidence() {
    let mut journal = authority_fixture();
    let expected = boundary();
    let mut write = proposal(wire::Producer::ResidentRecognition);
    write
        .fact(expected.clone(), FactCause::Root)
        .expect("compaction fact");
    journal.apply(write).expect("compaction commits");
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = CAST(CAST(envelope AS TEXT) || ' ' AS BLOB) WHERE kind = ?1",
            [FactKind::LaneCompacted.as_str()],
        )
        .expect("corrupt canonical bytes");
    journal
        .read(|read| read.lane_compaction_recorded(&expected))
        .expect_err("noncanonical evidence cannot prove recovery");
}
