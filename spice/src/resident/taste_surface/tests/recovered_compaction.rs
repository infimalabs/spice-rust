use super::*;

const THREAD: &str = "018f7777-7777-7444-8888-999999999999";

impl Fixture {
    fn attach_session(&self) {
        let observed = current_residency(&self.bus, self.epoch).expect("current residency");
        let mut next = observed.body().clone();
        let row = next.lanes.first_mut().expect("fixture lane");
        let session = wire::LaneSession {
            lane: self.lane,
            occupant: self.occupant,
            worktree: row.worktree.clone(),
            vendor: row.vendor.clone(),
            launch: None,
            epoch: self.epoch,
            stream: "stdout".to_owned(),
            identity: THREAD.to_owned(),
            locus: wire::TapeLocus {
                member: 7,
                offset: 0,
                length: 64,
            },
        };
        row.session = Some(session.clone());
        let mut proposal = write(Actor::Spice, self.epoch, Producer::ResidentRecognition)
            .expect("session proposal");
        let declared = proposal
            .fact(
                crate::resident::lane_surface::session_fact(&session),
                FactCause::Existing(observed.position()),
            )
            .expect("session declaration fact");
        proposal.residency_head(declared, move |_| next);
        commit(&self.bus, proposal).expect("session authority commits");
    }
}

#[test]
fn recovered_session_compaction_is_exact_durable_and_restart_idempotent() {
    let fixture = Fixture::new("recovered-compaction");
    fixture.attach_session();
    let bag = bag(&[("polling", &["polling"], "Wait.")]);
    fixture.recognize(
        &bag,
        &prose_record(fixture.lane, fixture.occupant, "polling before recovery"),
    );
    let locus = listen::Locus::new(1_u64 << 63 | 77, 400, 32);

    assert!(
        !crate::resident::lane_surface::recover_session_compaction(
            &fixture.bus,
            fixture.epoch,
            fixture.lane,
            uuid(99),
            THREAD,
            locus,
        )
        .expect("a stale occupant is ignored"),
    );
    assert!(
        crate::resident::lane_surface::recover_session_compaction(
            &fixture.bus,
            fixture.epoch,
            fixture.lane,
            fixture.occupant,
            THREAD,
            locus,
        )
        .expect("the exact session boundary commits"),
    );
    let snapshot = fixture.bus.fact_snapshot().expect("snapshot reads");
    let compacted = snapshot
        .facts()
        .iter()
        .filter_map(|envelope| {
            envelope
                .fact
                .read::<wire::body::LaneCompacted>()
                .map(|body| (envelope.seq, body))
        })
        .collect::<Vec<_>>();
    assert_eq!(compacted.len(), 1);
    assert_eq!(compacted[0].1.lane, fixture.lane);
    assert_eq!(compacted[0].1.occupant, fixture.occupant);
    assert_eq!(compacted[0].1.stream, format!("session:{THREAD}"));
    assert_eq!(compacted[0].1.member, locus.member);
    assert_eq!(compacted[0].1.offset, locus.offset);
    assert_eq!(compacted[0].1.length, locus.length);
    let facts = fixture.bus.task_snapshot().expect("task snapshot reads");
    assert_eq!(
        context_anchor(facts.facts(), fixture.lane, fixture.occupant)
            .expect("recovered boundary is the context anchor"),
        compacted[0].0,
    );
    fixture.recognize(
        &bag,
        &prose_record(
            fixture.lane,
            fixture.occupant,
            "polling after recovered compaction",
        ),
    );
    let reminders = fixture.reminders();
    assert_eq!(reminders.len(), 2);
    assert_eq!(reminders[0].compaction, fixture.anchor.get());
    assert_eq!(reminders[1].compaction, compacted[0].0.get());

    let fixture = fixture.restart();
    assert!(
        !crate::resident::lane_surface::recover_session_compaction(
            &fixture.bus,
            fixture.epoch,
            fixture.lane,
            fixture.occupant,
            THREAD,
            locus,
        )
        .expect("replayed recovery is idempotent"),
    );
    assert_eq!(
        fixture
            .bus
            .fact_snapshot()
            .expect("snapshot reads")
            .facts()
            .iter()
            .filter(|envelope| envelope.fact.read::<wire::body::LaneCompacted>().is_some())
            .count(),
        1,
    );
    fixture.finish();
}
