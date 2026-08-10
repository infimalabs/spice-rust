#[test]
fn lane_lifecycle_and_recall_cross_the_resident_in_both_forms() {
    let fixture = Fixture::new("lane-lifecycle");
    configure_fake_vendor(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);
    let (added, capability) = seat_and_recall(&fixture);
    resident = drain_and_restart(&fixture, resident, added.lane, capability);
    wake_and_finish(&fixture, resident, added.lane);
}

fn add_lane_and_assert_git(fixture: &Fixture, expected: &str) -> wire::intent::LaneChangeAnswer {
    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));
    assert_eq!(added.git.as_deref(), Some(expected));
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    assert_eq!(lanes.lanes[0].git.as_deref(), Some(expected));
    assert_session_binding(fixture, &lanes.lanes[0], lanes.epoch);
    added
}

fn stop_resident(fixture: &Fixture, resident: RunningResident) {
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(fixture, &["lane"], true, None));
    for row in lanes.lanes.iter().filter(|row| row.occupant.is_some()) {
        let drained = invoke_args(
            fixture,
            &["lane", "drain", &row.lane.to_string()],
            true,
            None,
        );
        assert!(
            drained.status.success(),
            "final drain failed: {:?}",
            drained.stderr
        );
    }
    let down = invoke(fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn fresh_and_resumed_launches_publish_the_exact_updated_and_current_notes() {
    let fixture = Fixture::new("launch-git-updated-current");
    configure_fake_vendor(&fixture);
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    let baseline = peer_commit(&fixture, "peer.txt", "peer\n");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added = add_lane_and_assert_git(&fixture, "updated working tree to the current baseline");
    assert_eq!(git_stdout(&fixture.root, &["rev-parse", "HEAD"]), baseline);
    assert_eq!(
        fs::read_to_string(fixture.root.join("peer.txt")).expect("advanced peer file reads"),
        "peer\n"
    );
    let drained = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    ));
    assert_eq!(drained.git, None, "drain is not a launch transition");
    let woke = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "wake", &added.lane.to_string()],
        true,
        None,
    ));
    assert_eq!(woke.git.as_deref(), Some("current"));
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    assert_eq!(lanes.lanes[0].git.as_deref(), Some("current"));
    assert_latest_launch_resumed(&fixture);
    stop_resident(&fixture, resident);
}

#[test]
fn dirty_launch_publishes_its_skip_and_retains_uncommitted_work() {
    let fixture = Fixture::new("launch-git-dirty");
    configure_fake_vendor(&fixture);
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    let before = git_stdout(&fixture.root, &["rev-parse", "HEAD"]);
    fs::write(fixture.root.join("README.md"), "lane edit\n").expect("uncommitted lane work writes");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    add_lane_and_assert_git(&fixture, "skipped:dirty");
    assert_eq!(git_stdout(&fixture.root, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        fs::read_to_string(fixture.root.join("README.md")).expect("lane edit reads"),
        "lane edit\n"
    );
    stop_resident(&fixture, resident);
}

#[test]
fn divergent_launch_publishes_its_skip_and_retains_both_histories() {
    let fixture = Fixture::new("launch-git-diverged");
    configure_fake_vendor(&fixture);
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    fs::write(fixture.root.join("lane.txt"), "lane\n").expect("lane commit candidate writes");
    fixture_git(&fixture, &["add", "lane.txt"]);
    fixture_git(&fixture, &["commit", "-m", "lane commit"]);
    let lane = git_stdout(&fixture.root, &["rev-parse", "HEAD"]);
    peer_commit(&fixture, "peer.txt", "peer\n");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    add_lane_and_assert_git(&fixture, "skipped:diverged");
    assert_eq!(git_stdout(&fixture.root, &["rev-parse", "HEAD"]), lane);
    assert_eq!(
        fs::read_to_string(fixture.root.join("lane.txt")).expect("lane commit reads"),
        "lane\n"
    );
    assert!(
        !fixture.root.join("peer.txt").exists(),
        "divergent launch merged the remote history"
    );
    stop_resident(&fixture, resident);
}

#[test]
fn unfetchable_launch_publishes_its_skip_and_retains_the_lane_head() {
    let fixture = Fixture::new("launch-git-unfetchable");
    configure_fake_vendor(&fixture);
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    fs::write(fixture.root.join("lane.txt"), "lane\n").expect("lane commit candidate writes");
    fixture_git(&fixture, &["add", "lane.txt"]);
    fixture_git(&fixture, &["commit", "-m", "lane commit"]);
    let before = git_stdout(&fixture.root, &["rev-parse", "HEAD"]);
    let unreachable = fixture.root.join("no-such-remote.git");
    fixture_git(
        &fixture,
        &[
            "remote",
            "set-url",
            "origin",
            &unreachable.display().to_string(),
        ],
    );
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    add_lane_and_assert_git(&fixture, "skipped:fetch-failed");
    assert_eq!(git_stdout(&fixture.root, &["rev-parse", "HEAD"]), before);
    assert_eq!(
        fs::read_to_string(fixture.root.join("lane.txt")).expect("lane commit reads"),
        "lane\n"
    );
    stop_resident(&fixture, resident);
}

#[test]
fn predecessor_import_launches_the_successor_through_the_shared_occupant_path() {
    let fixture = Fixture::linked("lane-import-launch");
    configure_fake_vendor(&fixture);
    let successor = configure_successor_worktree(&fixture);
    let successor = fs::canonicalize(successor).expect("successor worktree canonicalizes");
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &[
            "lane", "add", "--name", "Ava", "--life", "Drive", "--vendor", "claude",
        ],
        true,
        None,
    ));
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

    let imported = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &[
            "lane",
            "import",
            &added.lane.to_string(),
            &successor.display().to_string(),
        ],
        true,
        None,
    ));
    assert_eq!(
        (imported.from.as_str(), imported.to.as_str()),
        (
            fs::canonicalize(&fixture.root)
                .expect("predecessor worktree canonicalizes")
                .display()
                .to_string()
                .as_str(),
            successor.display().to_string().as_str(),
        )
    );
    let lanes =
        reply_body::<wire::intent::LaneAnswer>(&invoke_args(&fixture, &["lane"], true, None));
    let row = &lanes.lanes[0];
    assert_eq!(row.name.as_deref(), Some("Ava"));
    assert_eq!(row.worktree, successor.display().to_string());
    assert_eq!(row.disposition, "occupied");
    assert_eq!(row.lineage.len(), 2);
    assert_eq!(row.occupant, row.lineage.last().copied());
    assert!(row.bound, "import preserves resumable session presence");
    assert_eq!(row.model.as_deref(), Some("sonnet"));
    assert_eq!(row.effort.as_deref(), Some("high"));
    assert_eq!(row.session.as_deref(), Some(SESSION_ID));
    let safe = wire::encode(&lanes).expect("imported safe lanes encode");
    assert!(!String::from_utf8_lossy(&safe).contains("capabilities"));
    assert_latest_launch_resumed(&fixture);
    let journal = journal::Journal::open(fixture.journal()).expect("import journal opens");
    let kinds = journal
        .fact_snapshot()
        .expect("import facts read")
        .facts()
        .iter()
        .map(|fact| fact.fact.kind())
        .collect::<Vec<_>>();
    for kind in [
        FactKind::LaneImported,
        FactKind::LaneSessionDeclared,
        FactKind::LaneStarted,
    ] {
        assert!(kinds.contains(&kind), "import recorded no {kind:?}");
    }
    drop(journal);

    let final_drain = invoke_args(
        &fixture,
        &["lane", "drain", &added.lane.to_string()],
        true,
        None,
    );
    assert!(
        final_drain.status.success(),
        "final drain failed: {:?}",
        final_drain.stderr
    );
    let down = invoke(&fixture, "down");
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

#[test]
fn manual_hidden_task_creation_refuses_for_operator_and_lane_processes() {
    let fixture = Fixture::new("hidden-task-creation");
    configure_fake_vendor(&fixture);
    let mut config = fs::read_to_string(fixture.root.join("spice-rust.toml"))
        .expect("task fixture configuration reads");
    config.push_str("\n[tasks]\nhidden_stems = [\"scratch\"]\n");
    fs::write(fixture.root.join("spice-rust.toml"), config)
        .expect("task fixture project policy writes");
    initialize_task_git_fixture(&fixture);
    establish_task_baseline(&fixture);
    let mut resident = RunningResident::spawn(&fixture);
    resident.assert_ready(&fixture);

    let added_lane = reply_body::<wire::intent::LaneChangeAnswer>(&invoke_args(
        &fixture,
        &["lane", "add", "--life", "Drive", "--vendor", "claude"],
        true,
        None,
    ));
    fs::write(fixture.root.join("capture.txt"), "one loose task commit\n")
        .expect("capture candidate writes");
    fixture_git(&fixture, &["add", "capture.txt"]);
    fixture_git(&fixture, &["commit", "-m", "capture candidate"]);
    for (arguments, actor) in [
        (
            vec!["task", "add", "Manual oops", "--project", ".oops"],
            None,
        ),
        (
            vec![
                "task",
                "add",
                "Manual configured hidden",
                "--project",
                ".scratch.audit",
            ],
            Some(added_lane.lane),
        ),
        (
            vec![
                "task",
                "capture",
                "--title",
                "Captured oops",
                "--project",
                ".oops.correctness",
            ],
            None,
        ),
        (
            vec![
                "task",
                "capture",
                "--title",
                "Captured configured hidden",
                "--project",
                ".scratch.audit",
            ],
            Some(added_lane.lane),
        ),
    ] {
        let before = journal_cursor(&fixture);
        let refused = invoke_args(&fixture, &arguments, true, actor);
        assert_eq!(refusal_reason(&refused), "task::project-system-reserved",);
        assert_eq!(
            journal_cursor(&fixture),
            before,
            "{arguments:?} wrote facts"
        );
    }

    let system_task = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        &fixture,
        &[
            "task",
            "oops",
            "System-owned oops",
            "--origin",
            "fixture:hidden-task-creation",
        ],
        true,
        None,
    ));
    let visible =
        reply_body::<wire::intent::BoardAnswer>(&invoke_args(&fixture, &["board"], true, None));
    let hidden = visible
        .tasks
        .iter()
        .find(|task| task.statement.body.handle == system_task.task)
        .expect("system-created hidden task remains typed board evidence");
    assert_eq!(hidden.statement.body.project, ".oops");
    assert_eq!(hidden.visibility.hidden_stem(), Some("oops"));

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

fn add_next_action_task(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    title: &str,
) -> wire::intent::TaskAnswer {
    reply_body(&invoke_args(
        fixture,
        &[
            "task",
            "add",
            title,
            "--project",
            "spice.client",
            "--acceptance",
            "typed task can advance through its fixture workflow",
            "--origin",
            "fixture:lane-task-principal",
        ],
        true,
        Some(lane),
    ))
}

fn finish_noted_task(fixture: &Fixture, lane: wire::CanonicalUuid) -> wire::intent::TaskAnswer {
    let added_task = add_next_action_task(fixture, lane, "Preserve typed command lane principal");
    let implementation = reply_body::<wire::intent::ClaimAnswer>(&invoke_printed_next(
        fixture,
        added_task
            .next
            .as_deref()
            .expect("new eligible task prints its allocator action"),
        lane,
    ));
    assert_eq!(
        (implementation.task, implementation.holder),
        (added_task.task.clone(), Actor::Lane(lane))
    );
    let noted = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        fixture,
        &[
            "task",
            "note",
            &added_task.task.to_string(),
            "next-action lifecycle note",
        ],
        true,
        Some(lane),
    ));
    assert_eq!(
        noted.next, implementation.next,
        "a note preserves the held task's admitted transition"
    );
    let completed = reply_body::<wire::intent::TaskAnswer>(&invoke_printed_next(
        fixture,
        noted
            .next
            .as_deref()
            .expect("note on a held implementation prints its admitted completion"),
        lane,
    ));
    assert_eq!(&completed.task, &added_task.task);
    finish_review(fixture, lane, &completed, &added_task.task)
}

fn finish_review(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    completed: &wire::intent::TaskAnswer,
    task: &wire::TaskHandle,
) -> wire::intent::TaskAnswer {
    let review = reply_body::<wire::intent::ClaimAnswer>(&invoke_printed_next(
        fixture,
        completed
            .next
            .as_deref()
            .expect("advanced task prints its review allocator action"),
        lane,
    ));
    assert_eq!(
        (review.task, review.holder),
        (task.clone(), Actor::Lane(lane))
    );
    let reviewed = reply_body::<wire::intent::TaskAnswer>(&invoke_printed_next(
        fixture,
        review
            .next
            .as_deref()
            .expect("review claim prints its admitted verdict command"),
        lane,
    ));
    assert_eq!(
        (&reviewed.task, reviewed.status.as_str()),
        (task, "completed")
    );
    assert!(
        reviewed.next.is_none(),
        "a completed task must not advertise another transition"
    );
    reviewed
}

fn assert_oops_has_no_next(fixture: &Fixture, lane: wire::CanonicalUuid) {
    let oops = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        fixture,
        &[
            "task",
            "oops",
            "next-action fixture observation",
            "--origin",
            "fixture:lane-task-principal",
        ],
        true,
        Some(lane),
    ));
    assert!(
        oops.next.is_none(),
        "hidden oops work has no runnable transition"
    );
}

fn finish_dependent_task(fixture: &Fixture, lane: wire::CanonicalUuid, after: &wire::TaskHandle) {
    let dependent = add_next_action_task(fixture, lane, "Execute dependency edit next action");
    let dependent_claim = reply_body::<wire::intent::ClaimAnswer>(&invoke_printed_next(
        fixture,
        dependent
            .next
            .as_deref()
            .expect("second eligible task prints its allocator action"),
        lane,
    ));
    let edited = reply_body::<wire::intent::TaskAnswer>(&invoke_args(
        fixture,
        &[
            "task",
            "depends",
            &dependent.task.to_string(),
            &after.to_string(),
        ],
        true,
        Some(lane),
    ));
    assert_eq!(
        edited.next, dependent_claim.next,
        "editing a held task preserves its admitted completion"
    );
    let dependent_done = reply_body::<wire::intent::TaskAnswer>(&invoke_printed_next(
        fixture,
        edited
            .next
            .as_deref()
            .expect("dependency edit prints the held task's admitted completion"),
        lane,
    ));
    let reviewed = finish_review(fixture, lane, &dependent_done, &dependent.task);
    assert!(reviewed.next.is_none());
}
