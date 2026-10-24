use std::fs;

use lane::occupant::DetachedWait;

use super::*;

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131165{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn paths(root: &Path) -> RuntimePaths {
    RuntimePaths {
        worktree: root.to_path_buf(),
        common: root.join("common"),
        runtime: root.join("runtime"),
        data: root.join("data"),
        journal: root.join("journal"),
        resident: root.join("resident"),
        serve: root.join("serve"),
        lane_serve: root.join("lane-serve"),
        degraded: root.join("degraded"),
        tape: root.join("tape"),
        supervisors: root.join("supervisors"),
        steering_media: root.join("steering-media"),
        stage: root.join("stage"),
    }
}

fn terminal_state(
    root: &Path,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    session: wire::LaneSession,
    completion: PersistedCompletion,
) -> State {
    State {
        version: STATE_VERSION,
        supervisor: std::process::id(),
        worktree: root.to_string_lossy().into_owned(),
        vendor: "omp".to_owned(),
        lane,
        occupant,
        phase: Phase::Exited,
        group: Some(412),
        session: Some(session),
        git: Some("fixture-head".to_owned()),
        started_millis: Some(now_millis().expect("fixture time")),
        runtime_millis: Some(4 * 60 * 60 * 1_000),
        grace_millis: Some(20_000),
        reap_millis: Some(20_000),
        completion: CompletionField::some(completion),
        failure: None,
    }
}

#[test]
fn replacement_adopts_running_and_terminal_supervisors_across_request_versions() {
    let root = std::env::temp_dir().join(format!(
        "spice-terminal-supervisor-{}-{}",
        std::process::id(),
        now_millis().expect("fixture time")
    ));
    let paths = paths(&root);
    let lane = uuid(1);
    let occupant = uuid(2);
    let directory = supervisor_directory(&paths, lane, occupant);
    fs::create_dir_all(&directory).expect("supervisor directory");
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: root.to_string_lossy().into_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
        epoch: uuid(3),
        stream: "stdout".to_owned(),
        identity: "terminal-session".to_owned(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    };
    let row = wire::LaneResidency {
        lane,
        team: lane,
        name: None,
        worktree: root.to_string_lossy().into_owned(),
        life: "drive".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        fast_mode: false,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![uuid(4)],
        session: Some(session),
        disposition: "occupied".to_owned(),
    };

    // State v2 and the request-v3 release's identical state must both
    // survive resident replacement, including subsequent supervisor writes.
    for version in [2, 3] {
        for expected in [
            PersistedCompletion::Exited { status_raw: 0 },
            PersistedCompletion::TimedOut { status_raw: 0 },
            PersistedCompletion::Cancelled { status_raw: 7 << 8 },
        ] {
            let mut terminal = terminal_state(
                &root,
                lane,
                occupant,
                row.session.clone().expect("fixture session"),
                expected.clone(),
            );
            terminal.version = version;
            let mut running = terminal.clone();
            running.phase = Phase::Running;
            running.completion = CompletionField::none();
            let path = directory.join("state.json");
            publish_json(&path, &running).expect("running state publishes");
            let adopted = adopt(&paths, &row)
                .expect("running state reads")
                .expect("running supervisor is adoptable");
            let Adoption::Running(adopted) = adopted else {
                panic!("live supervisor was not adopted");
            };
            assert_eq!(adopted.occupant, occupant);
            assert_eq!(adopted.group, 412);
            assert_eq!(
                adopted.session,
                row.session.clone().expect("fixture session")
            );
            let DetachedWait::Running(process) = adopted
                .process
                .wait_for(Duration::ZERO)
                .expect("adopted supervisor remains running")
            else {
                panic!("live supervisor was mistaken for a terminal state");
            };

            publish_json(&path, &terminal).expect("terminal state publishes");
            let outcome = process.wait().expect("adopted supervisor completion reads");
            assert_eq!(PersistedCompletion::capture(outcome.completion()), expected);
            let adopted = adopt(&paths, &row)
                .expect("terminal state reads")
                .expect("terminal supervisor is adoptable");
            let Adoption::Exited(adopted) = adopted else {
                panic!("terminal supervisor was mistaken for a live launch");
            };
            assert_eq!(adopted.occupant, occupant);
            assert_eq!(adopted.group, 412);
            let outcome = adopted.process.wait().expect("terminal status reads");
            assert_eq!(PersistedCompletion::capture(outcome.completion()), expected);
        }
    }
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn every_typed_completion_round_trips_without_conflating_code_zero() {
    let root = std::env::temp_dir();
    let lane = uuid(5);
    let occupant = uuid(6);
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: root.to_string_lossy().into_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
        epoch: uuid(7),
        stream: "stdout".to_owned(),
        identity: "round-trip-session".to_owned(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    };
    let completions = [
        PersistedCompletion::Exited { status_raw: 0 },
        PersistedCompletion::TimedOut { status_raw: 0 },
        PersistedCompletion::Cancelled { status_raw: 7 << 8 },
    ];

    for expected in completions {
        let state = terminal_state(&root, lane, occupant, session.clone(), expected.clone());
        let bytes = serde_json::to_vec(&state).expect("state encodes");
        let decoded: State = serde_json::from_slice(&bytes).expect("state decodes");
        let decoded = decoded.validate().expect("state validates");
        assert_eq!(decoded.completion.as_ref(), Some(&expected));
    }
    for phase in [Phase::Starting, Phase::Running, Phase::Failed] {
        let mut state = terminal_state(
            &root,
            lane,
            occupant,
            session.clone(),
            PersistedCompletion::Exited { status_raw: 0 },
        );
        state.phase = phase;
        state.completion = CompletionField::none();
        let value = serde_json::to_value(&state).expect("nonterminal state encodes");
        assert!(
            value
                .get("completion")
                .is_some_and(serde_json::Value::is_null),
            "{phase:?} state omitted its required null completion"
        );
        serde_json::from_value::<State>(value)
            .expect("required null completion decodes")
            .validate()
            .expect("nonterminal null completion validates");
    }
}

#[test]
fn current_state_refuses_missing_unknown_and_phase_mismatched_completion() {
    let root = std::env::temp_dir().join(format!(
        "spice-invalid-supervisor-{}-{}",
        std::process::id(),
        now_millis().expect("fixture time")
    ));
    fs::create_dir_all(&root).expect("fixture directory creates");
    let lane = uuid(8);
    let occupant = uuid(9);
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: root.to_string_lossy().into_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        launch: wire::body::LaneLaunchIdentity::new("sonnet", "high"),
        epoch: uuid(10),
        stream: "stdout".to_owned(),
        identity: "invalid-session".to_owned(),
        locus: wire::TapeLocus {
            member: 0,
            offset: 0,
            length: 1,
        },
    };
    let valid = serde_json::to_value(terminal_state(
        &root,
        lane,
        occupant,
        session,
        PersistedCompletion::Exited { status_raw: 0 },
    ))
    .expect("valid state encodes");
    let cases = [
        ("missing", {
            let mut value = valid.clone();
            value
                .as_object_mut()
                .expect("state is an object")
                .remove("completion");
            value
        }),
        ("exited-null", {
            let mut value = valid.clone();
            value["completion"] = serde_json::Value::Null;
            value
        }),
        ("running-terminal", {
            let mut value = valid.clone();
            value["phase"] = serde_json::Value::String("running".to_owned());
            value
        }),
        ("unknown-kind", {
            let mut value = valid.clone();
            value["completion"]["kind"] = serde_json::Value::String("status_only".to_owned());
            value
        }),
        ("status-only", {
            let mut value = valid.clone();
            value
                .as_object_mut()
                .expect("state is an object")
                .remove("completion");
            value["status_raw"] = serde_json::Value::from(0);
            value
        }),
    ];
    let path = root.join("state.json");

    for version in [2, 3] {
        for (label, value) in &cases {
            let mut value = value.clone();
            value["version"] = serde_json::Value::from(version);
            fs::write(
                &path,
                serde_json::to_vec(&value).expect("invalid fixture encodes"),
            )
            .expect("invalid fixture writes");
            let error = read_state(&path)
                .err()
                .unwrap_or_else(|| panic!("v{version} {label} state reached adoption"));
            assert_eq!(
                error.kind(),
                io::ErrorKind::InvalidData,
                "v{version} {label}"
            );
        }
    }
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn contract_v1_request_is_refused_before_state_publication() {
    let root = std::env::temp_dir().join(format!(
        "spice-v1-supervisor-request-{}-{}",
        std::process::id(),
        now_millis().expect("fixture time")
    ));
    fs::create_dir_all(&root).expect("fixture directory creates");
    let request_path = root.join("request.json");
    let state_path = root.join("state.json");
    publish_json(
        &request_path,
        &Request {
            version: 1,
            worktree: root.to_string_lossy().into_owned(),
            vendor: "omp".to_owned(),
            epoch: uuid(3),
            lane: uuid(1),
            occupant: uuid(2),
            resume: None,
            fast_mode: false,
            model: None,
            effort: None,
            state: state_path.clone(),
        },
    )
    .expect("legacy request fixture publishes");

    let error = run(&request_path, &Ambient::default()).expect_err("v1 request refuses");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("unsupported supervisor request version 1")
    );
    assert!(!state_path.exists(), "refused request published state");
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn unsupported_state_versions_are_refused_without_a_legacy_execution_path() {
    let root = std::env::temp_dir().join(format!(
        "spice-v1-supervisor-{}-{}",
        std::process::id(),
        now_millis().expect("fixture time")
    ));
    fs::create_dir_all(&root).expect("fixture directory creates");
    let state = root.join("state.json");
    for version in [1, 4] {
        fs::write(
            &state,
            serde_json::to_vec(&serde_json::json!({
                "version": version,
                "phase": "exited",
                "status_raw": 0,
            }))
            .expect("unsupported fixture encodes"),
        )
        .expect("unsupported fixture writes");

        let error = match read_state(&state) {
            Ok(_) => panic!("v{version} state has no current execution path"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(&format!("contract v{version}")),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&format!(
                "current Spice {} contract v{STATE_VERSION}",
                env!("CARGO_PKG_VERSION"),
            )),
            "{diagnostic}"
        );
    }
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn launch_outcomes_publish_their_cli_text_not_debug_syntax() {
    assert_eq!(
        outcome_error(store_fault(
            "lane::native-shell-unavailable",
            "the supervised environment has no native SHELL",
        ))
        .to_string(),
        "lane::native-shell-unavailable; the supervised environment has no native SHELL"
    );
    let refusal = wire::Outcome::Refusal(wire::Refusal::bare(
        wire::RefusalDiagnostic::new("config::value-invalid", None, None, None)
            .with_detail("vendor.omp.binary must be a string"),
    ));
    assert_eq!(
        outcome_error(refusal).to_string(),
        "config::value-invalid; vendor.omp.binary must be a string"
    );
}
