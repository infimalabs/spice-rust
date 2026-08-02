use std::sync::atomic::{AtomicU64, Ordering};

use wire::intent::{StateAnswer, UpChange};
use wire::{ResidentState, Seq, SeqRange};

use super::*;
use crate::cli::{ExitOutcome, JournalAccess, ResultClass};
use crate::render::{self, Form};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

const COMMAND_CONTRACT: Contract = Contract {
    path: &["fixture", "command"],
    syntax: "fixture command",
    principal: Principal::Operator,
    journal: JournalAccess::Command,
    reply_position: ReplyPositionLaw::Committed,
    facts: &[],
    guards: &[Guard::Resident, Guard::OperatorCapability],
    tools: &[],
    result: ResultClass::Reply,
    outcomes: ExitOutcome::ALL,
    battery: "command-client::fixture",
};

struct Fixture {
    root: PathBuf,
    canvas: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-command-client-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture runtime directory");
        let canvas = root.join("canvas");
        Self { root, canvas }
    }

    fn publish(&self) -> canvas::Discovery {
        let discovery = canvas::Discovery {
            endpoint: "http://127.0.0.1:48123".to_owned(),
            capability: uuid(1),
            epoch: uuid(2),
        };
        discovery
            .publish(&self.canvas)
            .expect("fixture discovery publishes");
        discovery
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("00000000-0000-4000-8000-{last:012x}")
        .parse()
        .expect("canonical fixture UUID")
}

fn mapped() -> MappedCommand {
    MappedCommand::new(
        &COMMAND_CONTRACT,
        &["fixture", "command"],
        CommandKind::UpChange,
        vec![Witness::Transition {
            from: ResidentState::Seated,
            to: ResidentState::Dark,
        }],
        &UpChange {
            state: ResidentState::Dark,
        },
    )
    .expect("fixture command maps")
}

fn response(
    id: CanonicalUuid,
    epoch: CanonicalUuid,
    position: ReplyPosition,
    body: serde_json::Value,
) -> Vec<u8> {
    wire::encode(&Response {
        v: wire::WireVersion::CURRENT,
        id,
        epoch,
        outcome: Outcome::Reply(Reply { position, body }),
    })
    .expect("fixture response encodes")
}

fn committed() -> ReplyPosition {
    ReplyPosition::Committed(SeqRange {
        first: Seq::new(1).expect("fixture position"),
        last: Seq::new(2).expect("fixture position"),
    })
}

fn state_body() -> serde_json::Value {
    serde_json::to_value(StateAnswer {
        from: ResidentState::Seated,
        to: ResidentState::Dark,
    })
    .expect("fixture answer encodes")
}

#[test]
fn mapping_rejects_the_wrong_contract_and_incomplete_body_before_discovery() {
    let wrong_path = MappedCommand::selected(&COMMAND_CONTRACT, SelectedCommand::Down);
    let Err(Outcome::Fault(fault)) = wrong_path else {
        panic!("wrong path must fail mapping");
    };
    assert_eq!(fault.reason, "command::mapping-unsupported");

    let incomplete = MappedCommand::new(
        &COMMAND_CONTRACT,
        &["fixture", "command"],
        CommandKind::UpChange,
        vec![Witness::Transition {
            from: ResidentState::Seated,
            to: ResidentState::Dark,
        }],
        &serde_json::json!({}),
    );
    let Err(Outcome::Fault(fault)) = incomplete else {
        panic!("incomplete body must fail mapping");
    };
    assert_eq!(fault.reason, "command::body-incomplete");
}

#[test]
fn mapping_enforces_the_wire_witness_law() {
    let missing = MappedCommand::new(
        &COMMAND_CONTRACT,
        &["fixture", "command"],
        CommandKind::UpChange,
        Vec::new(),
        &UpChange {
            state: ResidentState::Dark,
        },
    );
    let Err(Outcome::Fault(fault)) = missing else {
        panic!("missing witness must fail mapping");
    };
    assert_eq!(fault.reason, "command::witness-law");
}

#[test]
fn every_landed_task_mutation_leaf_uses_one_current_command_kind() {
    let expected = [
        (&["board"][..], CommandKind::BoardRead),
        (&["board", "graph"][..], CommandKind::BoardGraph),
        (&["board", "handout"][..], CommandKind::BoardHandout),
        (&["task", "next"][..], CommandKind::ClaimWin),
        (&["task", "capture"][..], CommandKind::ClaimWin),
        (&["task", "add"][..], CommandKind::TaskAdd),
        (&["task", "done"][..], CommandKind::TaskDone),
        (&["task", "review"][..], CommandKind::TaskReview),
        (&["task", "note"][..], CommandKind::TaskNote),
        (&["task", "defer"][..], CommandKind::TaskDefer),
        (&["task", "wake"][..], CommandKind::TaskWake),
        (&["task", "free"][..], CommandKind::ClaimFree),
        (&["task", "oops"][..], CommandKind::TaskSystemAdd),
        (&["task", "depends"][..], CommandKind::TaskDepends),
        (&["doc"][..], CommandKind::DocApply),
        (&["doc", "ledger"][..], CommandKind::DocLedger),
        (&["taste", "add"][..], CommandKind::TaskAdd),
        (&["taste", "drop"][..], CommandKind::TaskAdd),
    ];
    assert_eq!(
        kind_for(&["taste"]),
        None,
        "the taste read authenticates without entering the mutation mapper"
    );

    for (path, kind) in expected {
        assert_eq!(
            kind_for(path),
            Some(kind),
            "current mapping moved: {path:?}"
        );
        assert_eq!(
            crate::cli::landing(path),
            crate::cli::Landing::Landed,
            "mapped family leaf is not visible: {path:?}",
        );
    }
    let mapped_family_leaves = crate::cli::contracts()
        .iter()
        .filter(|contract| matches!(contract.path[0], "board" | "task" | "doc" | "taste"))
        .filter(|contract| kind_for(contract.path).is_some())
        .count();
    assert_eq!(mapped_family_leaves, expected.len());
}

#[test]
fn discovery_replacement_is_detected_by_file_identity() {
    let fixture = Fixture::new("replacement");
    let original = fixture.publish();
    let read = DiscoveryRead::open(&fixture.canvas).expect("discovery opens");
    let replacement = canvas::Discovery {
        capability: uuid(3),
        epoch: uuid(4),
        ..original
    };
    replacement
        .publish(&fixture.canvas)
        .expect("replacement publishes");

    let Err(Outcome::Fault(fault)) = read.verify_current() else {
        panic!("replacement must invalidate the captured discovery");
    };
    assert_eq!(fault.reason, "canvas::discovery-replaced");
}

#[test]
fn canvas_opens_the_exact_origin_and_never_crosses_the_capability() {
    let fixture = Fixture::new("canvas-open");
    let discovery = fixture.publish();
    let mut launched = None;

    let outcome = open_canvas_with(
        &fixture.canvas,
        || {},
        |endpoint| {
            launched = Some(endpoint.to_owned());
            Ok(())
        },
    );

    assert_eq!(launched.as_deref(), Some(discovery.endpoint.as_str()));
    let Outcome::Reply(Reply { position, body }) = &outcome else {
        panic!("current Canvas discovery must open: {outcome:?}");
    };
    assert_eq!(*position, ReplyPosition::Direct {});
    assert_eq!(
        body,
        &wire::CanvasOpen {
            endpoint: discovery.endpoint,
            epoch: discovery.epoch,
        }
    );

    let capability = discovery.capability.to_string();
    let text = render::render(&outcome, Form::Text).expect("Canvas receipt renders as text");
    let text_body: wire::CanvasOpen =
        wire::decode(text.document()).expect("text carries the typed body");
    assert_eq!(text_body, *body);
    assert!(!String::from_utf8_lossy(text.document()).contains(&capability));

    let json = render::render(&outcome, Form::Json).expect("Canvas receipt renders as JSON");
    let json_outcome: Outcome<wire::CanvasOpen> =
        wire::decode(json.document()).expect("JSON carries the typed outcome");
    assert_eq!(json_outcome, outcome);
    assert!(!String::from_utf8_lossy(json.document()).contains(&capability));
}

#[test]
fn canvas_refuses_missing_malformed_and_nonloopback_discovery_without_launching() {
    let fixture = Fixture::new("canvas-refusals");
    let mut launches = 0_u8;
    let missing = open_canvas_with(
        &fixture.canvas,
        || {},
        |_| {
            launches += 1;
            Ok(())
        },
    );
    let Outcome::Fault(missing) = missing else {
        panic!("missing discovery must fault");
    };
    assert_eq!(missing.reason, "canvas::discovery-unavailable");

    fs::write(&fixture.canvas, b"not current discovery").expect("malformed fixture writes");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&fixture.canvas, fs::Permissions::from_mode(0o600))
            .expect("malformed fixture becomes owner-only");
    }
    let malformed = open_canvas_with(
        &fixture.canvas,
        || {},
        |_| {
            launches += 1;
            Ok(())
        },
    );
    let Outcome::Fault(malformed) = malformed else {
        panic!("malformed discovery must fault");
    };
    assert_eq!(malformed.reason, "canvas::discovery-malformed");

    canvas::Discovery {
        endpoint: "http://192.0.2.1:48123".to_owned(),
        capability: uuid(3),
        epoch: uuid(4),
    }
    .publish(&fixture.canvas)
    .expect("nonloopback fixture publishes");
    let nonloopback = open_canvas_with(
        &fixture.canvas,
        || {},
        |_| {
            launches += 1;
            Ok(())
        },
    );
    let Outcome::Fault(nonloopback) = nonloopback else {
        panic!("nonloopback discovery must fault");
    };
    assert_eq!(nonloopback.reason, "canvas::endpoint-not-loopback");
    assert_eq!(launches, 0, "no refused discovery reached the launcher");
}

#[test]
fn canvas_rechecks_discovery_identity_immediately_before_launch() {
    let fixture = Fixture::new("canvas-stale");
    let original = fixture.publish();
    let replacement = canvas::Discovery {
        capability: uuid(5),
        epoch: uuid(6),
        ..original.clone()
    };
    let mut launched = false;

    let outcome = open_canvas_with(
        &fixture.canvas,
        || {
            replacement
                .publish(&fixture.canvas)
                .expect("replacement publishes before the launch check");
        },
        |_| {
            launched = true;
            Ok(())
        },
    );

    let Outcome::Fault(fault) = outcome else {
        panic!("replaced discovery must fault");
    };
    assert_eq!(fault.reason, "canvas::discovery-replaced");
    assert!(!launched, "stale discovery must not reach the launcher");
    assert!(!fault.detail.contains(&original.capability.to_string()));
}

#[test]
fn canvas_launcher_interruption_is_one_typed_fault() {
    let fixture = Fixture::new("canvas-launch-interruption");
    let discovery = fixture.publish();

    let outcome = open_canvas_with(
        &fixture.canvas,
        || {},
        |_| Err("fixture launcher was interrupted".to_owned()),
    );

    let Outcome::Fault(fault) = outcome else {
        panic!("launcher interruption must fault");
    };
    assert_eq!(fault.reason, "canvas::launch-failed");
    assert_eq!(fault.detail, "fixture launcher was interrupted");
    assert!(!fault.detail.contains(&discovery.capability.to_string()));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn canvas_platform_launcher_has_one_fixed_program_and_exact_endpoint() {
    use std::ffi::OsStr;

    let endpoint = "http://127.0.0.1:48123";
    let command = browser_command(endpoint).expect("this platform has one launcher");
    #[cfg(target_os = "macos")]
    {
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new("-u"), OsStr::new(endpoint)]
        );
    }
    #[cfg(target_os = "linux")]
    {
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/xdg-open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [OsStr::new(endpoint)]
        );
    }
}

#[cfg(unix)]
#[test]
fn discovery_symlinks_and_public_modes_are_refused() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let fixture = Fixture::new("insecure");
    let discovery = fixture.publish();
    fs::set_permissions(&fixture.canvas, fs::Permissions::from_mode(0o644))
        .expect("fixture mode changes");
    let Err(Outcome::Fault(fault)) = DiscoveryRead::open(&fixture.canvas) else {
        panic!("public discovery must be refused");
    };
    assert_eq!(fault.reason, "canvas::discovery-insecure");

    fs::remove_file(&fixture.canvas).expect("public fixture removes");
    let target = fixture.root.join("target");
    discovery.publish(&target).expect("target publishes");
    symlink(&target, &fixture.canvas).expect("fixture symlink");
    let Err(Outcome::Fault(fault)) = DiscoveryRead::open(&fixture.canvas) else {
        panic!("symlink discovery must be refused");
    };
    assert_eq!(fault.reason, "canvas::discovery-insecure");
}

#[cfg(unix)]
#[test]
fn discovery_fifo_substitution_is_nonblocking_and_refused_from_the_descriptor() {
    use rustix::fs::{OFlags, fcntl_getfl};
    use std::os::unix::fs::OpenOptionsExt as _;

    let fixture = Fixture::new("fifo-substitution");
    let discovery = fixture.publish();
    let mut anchor = None;

    let started = Instant::now();
    let outcome = DiscoveryRead::open_with(&fixture.canvas, || {
        fs::remove_file(&fixture.canvas).expect("regular discovery removes");
        let status = ProcessCommand::new("mkfifo")
            .arg(&fixture.canvas)
            .status()
            .expect("mkfifo launches");
        assert!(status.success(), "mkfifo creates the substitution");
        anchor = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&fixture.canvas)
                .expect("fixture anchor opens without waiting"),
        );
    });

    let Err(Outcome::Fault(fault)) = outcome else {
        panic!("FIFO substitution must fault");
    };
    assert_eq!(fault.reason, "canvas::discovery-insecure");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "FIFO substitution must not wait for a writer"
    );
    assert!(!fault.detail.contains(&discovery.capability.to_string()));

    let opened = open_discovery(&fixture.canvas).expect("FIFO descriptor opens");
    let flags = fcntl_getfl(&opened).expect("FIFO descriptor flags read");
    assert!(flags.contains(OFlags::NONBLOCK));
    drop(anchor);
}

#[test]
fn response_binds_identity_epoch_position_and_answer_shape() {
    let mapped = mapped();
    let epoch = uuid(5);
    let bytes = response(mapped.id, epoch, committed(), state_body());
    let outcome = decode_response(
        &bytes,
        &COMMAND_CONTRACT,
        mapped.id,
        mapped.kind,
        epoch,
        uuid(6),
    );
    assert!(matches!(
        outcome,
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(_),
            body: CommandAnswer::State(StateAnswer {
                to: ResidentState::Dark,
                ..
            })
        })
    ));

    let replay = decode_response(
        &bytes,
        &COMMAND_CONTRACT,
        uuid(7),
        mapped.kind,
        epoch,
        uuid(6),
    );
    let Outcome::Fault(fault) = replay else {
        panic!("a replayed response cannot answer a different command");
    };
    assert_eq!(fault.reason, "command::answer-mismatch");

    let snapshot = response(
        mapped.id,
        epoch,
        ReplyPosition::Snapshot(Seq::new(1).expect("fixture position")),
        state_body(),
    );
    let Outcome::Fault(fault) = decode_response(
        &snapshot,
        &COMMAND_CONTRACT,
        mapped.id,
        mapped.kind,
        epoch,
        uuid(6),
    ) else {
        panic!("snapshot cannot answer a mutating contract");
    };
    assert_eq!(fault.reason, "command::position-mismatch");

    let malformed = response(
        mapped.id,
        epoch,
        committed(),
        serde_json::json!({ "from": "seated" }),
    );
    let Outcome::Fault(fault) = decode_response(
        &malformed,
        &COMMAND_CONTRACT,
        mapped.id,
        mapped.kind,
        epoch,
        uuid(6),
    ) else {
        panic!("wrong answer shape must fault");
    };
    assert_eq!(fault.reason, "command::answer-binding");
}

#[test]
fn remote_outcomes_cannot_echo_the_discovery_capability() {
    let capability = uuid(8);
    let discovery = canvas::Discovery {
        endpoint: "http://127.0.0.1:48123".to_owned(),
        capability,
        epoch: uuid(9),
    };
    let refusal = Refusal {
        reason: "hello::fixture".to_owned(),
        witness: None,
        expected: None,
        observed: Some(capability.to_string()),
        remedy: "use current discovery".to_owned(),
    };
    let opening = wire::encode(&refusal).expect("fixture refusal encodes");
    let Err(Outcome::Fault(fault)) = decode_opening(&opening, &discovery) else {
        panic!("opening capability echo must fault");
    };
    assert_eq!(fault.reason, "canvas::capability-disclosed");
    assert!(!fault.detail.contains(&capability.to_string()));

    let mapped = mapped();
    let response = wire::encode(&Response {
        v: wire::WireVersion::CURRENT,
        id: mapped.id,
        epoch: discovery.epoch,
        outcome: Outcome::<serde_json::Value>::Refusal(refusal),
    })
    .expect("fixture response encodes");
    let Outcome::Fault(fault) = decode_response(
        &response,
        &COMMAND_CONTRACT,
        mapped.id,
        mapped.kind,
        discovery.epoch,
        capability,
    ) else {
        panic!("response capability echo must fault");
    };
    assert_eq!(fault.reason, "canvas::capability-disclosed");
    assert!(!fault.detail.contains(&capability.to_string()));
}
