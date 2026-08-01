use listen::{Canary, Decoded, EventKind};
use wire::{CanonicalUuid, Instant, body};

fn lane() -> CanonicalUuid {
    "019fbbde-5105-75a3-a76a-722afde1e407"
        .parse()
        .expect("canonical lane UUID")
}

fn instant(text: &str) -> Instant {
    text.parse().expect("canonical instant")
}

#[test]
fn present_and_not_yet_due_kinds_do_not_alarm() {
    let window = instant("2026-08-01T06:00:00.000Z");
    let observations = [Decoded::Events(Vec::new())];

    assert!(
        Canary::due_alarms(
            lane(),
            &[EventKind::Tool],
            &observations,
            window,
            instant("2026-08-01T05:59:59.999Z"),
        )
        .is_empty()
    );

    let record = br#"{"type":"item.started","item":{"id":"call-1","type":"command_execution","command":"cargo test","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#;
    let present = listen::decode(
        listen::Dialect::Codex,
        record,
        listen::Locus::new(0, 0, record.len() as u64),
    )
    .expect("fixture locus length fits");
    assert!(Canary::due_alarms(lane(), &[EventKind::Tool], &[present], window, window,).is_empty());
}

#[test]
fn due_absence_emits_one_typed_alarm_per_kind() {
    let lane = lane();
    let window = instant("2026-08-01T06:00:00.000Z");

    assert_eq!(
        Canary::due_alarms(
            lane,
            &[EventKind::Tool, EventKind::Tool, EventKind::Usage],
            &[],
            window,
            instant("2026-08-01T06:00:00.001Z"),
        ),
        vec![
            body::CanaryAlarmed {
                lane,
                expected: "tool".to_owned(),
                window,
            },
            body::CanaryAlarmed {
                lane,
                expected: "usage".to_owned(),
                window,
            },
        ]
    );
}

#[test]
fn event_kind_spellings_are_stable() {
    assert_eq!(
        EventKind::ALL.map(EventKind::as_str),
        [
            "prose",
            "reasoning",
            "tool",
            "usage",
            "compaction",
            "failure",
        ]
    );
}

#[test]
fn activity_without_decoded_context_is_a_typed_contradiction() {
    let lane = lane();
    let residue_record = br#"{"type":"future.record"}"#;
    let residue = listen::decode(
        listen::Dialect::Codex,
        residue_record,
        listen::Locus::new(0, 0, residue_record.len() as u64),
    )
    .expect("fixture locus length fits");
    let activity = residue_record.len() as u64;
    let decoded = residue.events().len() as u64;
    assert!(residue.residue().is_some());

    assert_eq!(
        Canary::contradiction(lane, activity, decoded),
        Some(body::CanaryContradicted {
            lane,
            activity,
            decoded: 0,
        })
    );
    assert_eq!(Canary::contradiction(lane, 0, 0), None);
    assert_eq!(Canary::contradiction(lane, 41, 3), None);
}
