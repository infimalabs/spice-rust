use super::*;

#[test]
fn typed_task_commands_retain_the_ambient_lane_principal() {
    let fixture = Fixture::new("lane-task-principal");
    configure_fake_vendor(&fixture);
    let mut config = fs::read_to_string(fixture.root.join("spice.toml"))
        .expect("task fixture configuration reads");
    config.push_str("\n[tasks]\nstems = [\"spice\"]\n");
    fs::write(fixture.root.join("spice.toml"), config).expect("task fixture project policy writes");
    initialize_task_git_fixture(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added_lane = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));
    let reviewed = finish_noted_task(&fixture, added_lane.lane);
    assert_oops_has_no_next(&fixture, added_lane.lane);
    finish_dependent_task(&fixture, added_lane.lane, &reviewed.task);

    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added_lane.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

fn assert_recovered_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    ancestor: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
) {
    let discovery = fixture.discovery();
    let snapshot =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    let row = &snapshot.lanes[0];
    assert_eq!((row.lane, row.disposition.as_str()), (lane, "occupied"));
    assert!(row.bound);
    assert_eq!(
        (row.lineage.first(), row.lineage.len()),
        (Some(&ancestor), 2)
    );
    assert_eq!(row.occupant, row.lineage.last().copied());
    let journal = journal::Journal::open(fixture.journal()).expect("restored journal opens");
    let head = journal
        .read(|read| read.residency_head())
        .expect("restored residency reads")
        .expect("restored residency exists")
        .into_body();
    assert_eq!(head.epoch, discovery.epoch);
    assert_eq!(head.lanes[0].capabilities, [capability]);
    assert_eq!(
        head.lanes[0]
            .session
            .as_ref()
            .map(|session| session.identity.as_str()),
        Some(SESSION_ID)
    );
    assert_latest_launch_resumed(fixture);
    let facts = journal.fact_snapshot().expect("restored facts read");
    let kinds = facts
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&FactKind::LaneDegraded));
    assert!(kinds.contains(&FactKind::LaneRenewed));
    drop(journal);
    let recall = reply_body::<wire::intent::RecallAnswer>(&invoke_args(
        fixture,
        &["recall"],
        true,
        Some(lane),
    ));
    assert_eq!(recall.lane, Some(lane));
    assert!(
        recall
            .recent
            .iter()
            .any(|entry| entry.kind == FactKind::LaneRenewed)
    );
}

#[test]
fn replacement_resident_reconstructs_durable_occupied_lane() {
    let fixture = Fixture::new("lane-restart-occupied");
    configure_fake_vendor(&fixture);
    let mut initial = RunningResident::spawn(&fixture);
    initial.assert_ready(&fixture);
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "initial down failed: {:?}",
        down.stderr
    );
    assert!(initial.wait().status.success(), "initial resident failed");
    let lane = uuid("00000000-0000-4000-8000-000000000025");
    let capability = uuid("00000000-0000-4000-8000-000000000026");
    let ancestor = uuid("00000000-0000-4000-8000-000000000027");
    seed_occupied_lane(&fixture, lane, capability, ancestor);
    let mut replacement = RunningResident::spawn(&fixture);
    replacement.assert_ready(&fixture);
    assert_recovered_lane(&fixture, lane, ancestor, capability);
    let drained = invoke_args(&fixture, &["lane", "drain", &lane.to_string()], true, None);
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "replacement down failed: {:?}",
        down.stderr
    );
    assert!(
        replacement.wait().status.success(),
        "replacement resident failed"
    );
}

#[test]
fn drain_records_an_occupant_that_already_exited() {
    let fixture = Fixture::new("lane-exited");
    let exited = exiting_vendor(&fixture, 7);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    exited.join().expect("vendor exit is observed");
    let drained = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    ));
    assert_eq!(
        (drained.from.as_str(), drained.to.as_str()),
        ("occupied", "vacant")
    );
    let journal = journal::Journal::open(fixture.journal()).expect("exited lane journal opens");
    let facts = journal.fact_snapshot().expect("exited lane facts read");
    let exited = facts
        .facts()
        .iter()
        .find(|fact| fact.fact.kind() == FactKind::LaneExited)
        .expect("self-exited occupant is recorded");
    assert_eq!(exited.fact.body()["code"], 7);
    drop(journal);
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn resident_capture_is_byte_exact_and_listenable_through_terminal_residue() {
    const COMPLETE: &str = concat!(
        r#"{"type":"assistant","message":{"type":"message","#,
        r#""role":"assistant","content":[{"type":"text","text":"done"}],"#,
        r#""usage":{"input_tokens":7,"cache_read_input_tokens":5,"#,
        r#""cache_creation_input_tokens":3,"output_tokens":2}}}"#,
    );
    const TERMINAL: &str = r#"{"type":"assistant","message":"#;

    let fixture = Fixture::new("lane-capture");
    let exited = recording_vendor(&fixture, COMPLETE, TERMINAL);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    exited.join().expect("recording vendor exit is observed");
    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );

    let tape_root = fixture
        .root
        .join(".git/.spice-rs/tape")
        .join(added.lane.to_string())
        .join("claude");
    let tape = tape::Tape::open_existing(&tape_root).expect("resident transcript tape opens");
    let extents = tape.extents().expect("resident transcript extents read");
    assert_eq!(extents.len(), 1);
    let bytes = tape
        .read(tape::Locus::new(extents[0].member, 0, extents[0].length))
        .expect("resident transcript reads");
    assert_eq!(
        bytes,
        format!("{SESSION_RECORD}\n{COMPLETE}\n{TERMINAL}").as_bytes()
    );

    let replay = listen::retro(
        fixture.root.join(".git/.spice-rs/tape"),
        listen::RetroSelector::Lane(added.lane.to_string()),
    )
    .expect("resident transcript replays");
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        listen::Observation::Event {
            event: listen::SurfaceEvent::Prose { text, .. },
            ..
        } if text == "done"
    )));
    assert!(replay.observations.iter().any(|observation| matches!(
        observation,
        listen::Observation::Residue {
            residue: listen::SurfaceResidue::Truncated {},
            ..
        }
    )));

    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn missing_and_duplicate_declarations_refuse_and_fully_unwind_lane_add() {
    for (name, duplicate, expected) in [
        ("missing", false, "lane::session-missing"),
        ("duplicate", true, "lane::session-duplicate"),
    ] {
        let fixture = Fixture::new(&format!("lane-session-{name}"));
        let stopped = fixture.root.join("vendor-stopped");
        let vendor_pid = fixture.root.join("vendor-pid");
        let announced = if duplicate {
            format!(
                concat!("printf '%s\\n' {}\n", "printf '%s\\n' {}\n"),
                shell_word(SESSION_RECORD),
                shell_word(SESSION_RECORD),
            )
        } else {
            String::new()
        };
        let script =
            vendor_prologue(&stopped, &vendor_pid) + &announced + "while :; do sleep 1; done\n";
        configure_vendor(&fixture, &script);
        let mut resident = RunningResident::spawn(&fixture);
        resident.assert_ready(&fixture);

        let started = Monotonic::now();
        let added = invoke_args(&fixture, &["lane", "add", "--vendor", "claude"], true, None);
        assert_eq!(refusal_reason(&added), expected);
        assert!(started.elapsed() < LANE_DEADLINE_BOUND);
        let lanes =
            reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
        assert!(lanes.lanes.is_empty());
        // The refusal returns without waiting for the vendor's TERM trap, so the
        // stop is observed rather than read at the return.
        let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
        assert_eq!(
            stop, "stopped",
            "refused vendor records bounded termination"
        );

        let down = invoke(&fixture, "down");
        assert!(down.status.success(), "down failed: {:?}", down.stderr);
        assert!(resident.wait().status.success(), "resident failed");
    }
}

#[test]
fn mismatched_resume_and_moved_tape_refuse_without_advancing_the_vacant_lane() {
    let fixture = Fixture::new("lane-session-resume-refusals");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--vendor", "claude"],
        true,
        None,
    ));
    let drained = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        drained.status.success(),
        "drain failed: {:?}",
        drained.stderr
    );

    let stopped = fixture.root.join("mismatched-vendor-stopped");
    let vendor_pid = fixture.root.join("mismatched-vendor-pid");
    configure_vendor(
        &fixture,
        &(vendor_prologue(&stopped, &vendor_pid)
            + &format!(
                concat!("printf '%s\\n' {}\n", "while :; do sleep 1; done\n"),
                shell_word(MISMATCHED_SESSION_RECORD),
            )),
    );
    let mismatch = invoke_args(
        &fixture,
        &["lane", "wake", &added.lane.to_string()],
        true,
        None,
    );
    assert_eq!(refusal_reason(&mismatch), "lane::session-mismatched");
    // The refusal returns without waiting for the vendor's TERM trap, so the
    // stop is observed rather than read at the return.
    let stop = wait_for_vendor_stop(&fixture.cleanup, &vendor_pid, &stopped);
    assert_eq!(stop, "stopped", "mismatched vendor was terminated");
    let vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    assert_eq!(
        (
            vacant.lanes[0].occupant,
            vacant.lanes[0].lineage.len(),
            vacant.lanes[0].bound,
        ),
        (None, 1, true)
    );

    let tape = fixture
        .root
        .join(".git/.spice-rs/tape")
        .join(added.lane.to_string())
        .join("claude");
    fs::rename(&tape, tape.with_extension("moved")).expect("fixture tape moves");
    let moved = invoke_args(
        &fixture,
        &["lane", "wake", &added.lane.to_string()],
        true,
        None,
    );
    assert_eq!(refusal_reason(&moved), "lane::session-tape-moved");
    let still_vacant =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    assert_eq!(
        (
            still_vacant.lanes[0].occupant,
            still_vacant.lanes[0].lineage.len()
        ),
        (None, 1)
    );

    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}
