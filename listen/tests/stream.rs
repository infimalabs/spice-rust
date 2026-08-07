use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use listen::{
    Checkpoint, Dialect, EventKind, Position, RecordPayload, Scan, ScanError, Span, scan,
};
use tape::Tape;

const PROSE: &[u8] =
    br#"{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"working"}}"#;
const CLAUDE_MULTI_EVENT: &[u8] = br#"{"type":"assistant","message":{"type":"message","role":"assistant","content":[{"type":"text","text":"working"},{"type":"tool_use","id":"tool-1","name":"shell","input":{"command":"cargo test"}}]}}"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-listen-stream-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("scratch root is exclusively created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn remove(self) {
        fs::remove_dir_all(&self.0).expect("scratch root is explicitly removed");
        std::mem::forget(self);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn line(bytes: &[u8]) -> Vec<u8> {
    let mut line = bytes.to_vec();
    line.push(b'\n');
    line
}

#[test]
fn complete_events_and_residue_advance_while_a_partial_record_does_not() {
    let scratch = Scratch::new("checkpoint");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let prose = line(PROSE);
    let residue = b"not-json\n";
    let partial = br#"{"type":"turn.failed"#;
    tape.tee(&prose).expect("typed record is durable");
    tape.tee(residue).expect("residue record is durable");
    tape.tee(partial).expect("partial record is durable");

    let first = scan(scratch.path(), Dialect::Codex, None).expect("initial scan succeeds");

    assert_eq!(first.records.len(), 2, "both complete records advance");
    assert_eq!(
        first.checkpoint.next,
        Position {
            member: 1,
            offset: (prose.len() + residue.len()) as u64,
        },
        "pending bytes are excluded from the next-record checkpoint"
    );
    assert_eq!(first.pending.len(), 1);
    let checkpoint_json = serde_json::to_string(&first.checkpoint).expect("checkpoint serializes");
    assert_eq!(
        serde_json::from_str::<Checkpoint>(&checkpoint_json).expect("checkpoint reopens"),
        first.checkpoint
    );
    let mut malformed: serde_json::Value =
        serde_json::from_str(&checkpoint_json).expect("checkpoint is JSON");
    malformed["source"] = "NOT-A-SOURCE".into();
    assert!(
        serde_json::from_value::<Checkpoint>(malformed).is_err(),
        "checkpoint deserialization does not admit another source spelling"
    );
    match &first.records[0].payload {
        RecordPayload::Events(events) => {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].ordinal, 0);
            assert_eq!(events[0].event.kind(), EventKind::Prose);
            assert_eq!(
                events[0].spans,
                vec![Span {
                    member: 1,
                    offset: 0,
                    length: prose.len() as u64,
                }]
            );
        }
        RecordPayload::Residue(_) => panic!("typed prose became residue"),
    }
    assert!(matches!(
        first.records[1].payload,
        RecordPayload::Residue(_)
    ));

    let suffix = b",\"error\":{\"message\":\"stopped\"}}\n";
    tape.tee(suffix).expect("partial record completes");
    let resumed = scan(scratch.path(), Dialect::Codex, Some(&first.checkpoint))
        .expect("scan resumes at the prior complete boundary");
    assert_eq!(resumed.records.len(), 1);
    assert!(resumed.pending.is_empty());
    assert_eq!(resumed.records[0].next, resumed.checkpoint);
    scratch.remove();
}

#[test]
fn one_record_can_cross_a_seal_without_losing_identity_or_exact_spans() {
    let scratch = Scratch::new("cross-member");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let split = PROSE.len() / 2;
    tape.tee(&PROSE[..split]).expect("record prefix is durable");
    let source = tape.source_id().expect("source reads before sealing");
    tape.seal().expect("prefix member seals");
    tape.tee(&line(&PROSE[split..]))
        .expect("record suffix is durable");

    let scanned = scan(scratch.path(), Dialect::Codex, None).expect("cross-member scan succeeds");

    assert_eq!(scanned.checkpoint.source, source);
    let RecordPayload::Events(events) = &scanned.records[0].payload else {
        panic!("cross-member prose became residue");
    };
    assert_eq!(events[0].spans.len(), 2);
    assert_eq!(events[0].spans[0].member, 1);
    assert_eq!(events[0].spans[1].member, 2);
    scratch.remove();
}

#[test]
fn same_generation_retreat_and_gap_are_refused() {
    let scratch = Scratch::new("continuity");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    let complete = line(PROSE);
    tape.tee(&complete).expect("record is durable");
    let scanned = scan(scratch.path(), Dialect::Codex, None).expect("initial scan succeeds");

    fs::write(scratch.path().join("000000000001.plain"), b"short")
        .expect("fixture retreats the active member");
    assert!(matches!(
        scan(scratch.path(), Dialect::Codex, Some(&scanned.checkpoint)),
        Err(ScanError::CheckpointRetreated { .. })
    ));

    let gap = Checkpoint {
        source: scanned.checkpoint.source.clone(),
        next: Position {
            member: 3,
            offset: 0,
        },
    };
    assert!(matches!(
        scan(scratch.path(), Dialect::Codex, Some(&gap)),
        Err(ScanError::CheckpointGap { .. })
    ));
    scratch.remove();
}

#[test]
fn a_replacement_generation_can_only_begin_at_byte_zero() {
    let scratch = Scratch::new("replacement");
    let path = scratch.path().join("selected");
    let tape = Tape::open(&path).expect("first generation opens");
    tape.tee(&line(PROSE)).expect("first generation grows");
    let old = scan(&path, Dialect::Codex, None).expect("first generation scans");
    fs::remove_dir_all(&path).expect("first generation is removed");
    let replacement = Tape::open(&path).expect("replacement generation opens");

    assert!(matches!(
        scan(&path, Dialect::Codex, Some(&old.checkpoint)),
        Err(ScanError::SourceChanged { .. })
    ));

    let reset = Checkpoint {
        source: old.checkpoint.source,
        next: Position::start(),
    };
    let restarted = scan(&path, Dialect::Codex, Some(&reset))
        .expect("another generation may begin only at byte zero");
    assert_eq!(
        restarted.checkpoint.source,
        replacement.source_id().expect("replacement source reads")
    );
    scratch.remove();
}

#[test]
fn missing_malformed_and_unreadable_source_identity_all_refuse_scanning() {
    for (label, damage) in [("missing", 0_u8), ("malformed", 1_u8), ("unreadable", 2_u8)] {
        let scratch = Scratch::new(label);
        Tape::open(scratch.path()).expect("fixture tape opens");
        let source = scratch.path().join("source");
        fs::remove_file(&source).expect("fixture removes current identity");
        match damage {
            0 => {}
            1 => fs::write(&source, b"not a tape identity\n").expect("malformed identity writes"),
            2 => fs::create_dir(&source).expect("directory obstructs identity read"),
            _ => unreachable!(),
        }

        assert!(
            matches!(
                scan(scratch.path(), Dialect::Codex, None),
                Err(ScanError::Tape { .. })
            ),
            "{label} identity did not refuse ingestion"
        );
        scratch.remove();
    }
}

/// This consumer seam compiles using public listen records and checkpoints;
/// it has neither provider JSON nor an effort-owned persistence shape.
fn consume_public_scan(scan: Scan) -> Vec<(Checkpoint, u64, EventKind, Vec<Span>)> {
    scan.records
        .into_iter()
        .flat_map(|record| match record.payload {
            RecordPayload::Events(events) => events
                .into_iter()
                .map(|event| {
                    (
                        record.next.clone(),
                        event.ordinal,
                        event.event.kind(),
                        event.spans,
                    )
                })
                .collect(),
            RecordPayload::Residue(_) => Vec::new(),
        })
        .collect()
}

#[test]
fn consumers_receive_only_public_typed_events_and_checkpoints() {
    let scratch = Scratch::new("public-seam");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    tape.tee(&line(PROSE)).expect("record is durable");

    let consumed = consume_public_scan(
        scan(scratch.path(), Dialect::Codex, None).expect("public scan succeeds"),
    );

    assert_eq!(consumed.len(), 1);
    assert_eq!((consumed[0].1, consumed[0].2), (0, EventKind::Prose));
    scratch.remove();
}

#[test]
fn event_ordinals_distinguish_events_sharing_the_same_record_spans() {
    let scratch = Scratch::new("ordinals");
    let tape = Tape::open(scratch.path()).expect("tape opens");
    tape.tee(&line(CLAUDE_MULTI_EVENT))
        .expect("multi-event record is durable");

    let scanned = scan(scratch.path(), Dialect::Claude, None).expect("record scans");
    let RecordPayload::Events(events) = &scanned.records[0].payload else {
        panic!("current Claude record became residue");
    };

    assert_eq!(
        events
            .iter()
            .map(|event| (event.ordinal, event.event.kind()))
            .collect::<Vec<_>>(),
        vec![(0, EventKind::Prose), (1, EventKind::Tool)]
    );
    assert_eq!(events[0].spans, events[1].spans);
    scratch.remove();
}
