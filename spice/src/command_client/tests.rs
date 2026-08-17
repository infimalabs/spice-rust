use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use sill::{Acquisition, Possession};
use spice::resident_lock::holder_record;

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
    publisher: Option<Possession>,
}

impl Fixture {
    /// A runtime whose published records have a resident still holding the
    /// lock, because that is the only state in which a client may dial them.
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-command-client-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fixture runtime directory");
        let canvas = root.join("canvas");
        let lock = LockFile::new(
            LockName::new("resident").expect("resident is a non-empty lock name"),
            resident_lock_path(&canvas),
        );
        let Acquisition::Acquired(publisher) = lock.try_acquire().expect("fixture resident lock")
        else {
            panic!("a fixture runtime nothing else has seen cannot be contended");
        };
        Self {
            root,
            canvas,
            publisher: Some(publisher),
        }
    }

    /// Lose the publisher the way a killed resident does.
    ///
    /// Dropping the possession releases the kernel lock and nothing else: the
    /// record it published stays exactly as written, and so does the holder
    /// line in the lock file naming a pid. `ResidentLock`'s own `Drop`, which
    /// truncates that line, is deliberately not run here — a fixture that
    /// cleaned up after itself would only prove the orderly path.
    fn release_publisher(&mut self) {
        self.publisher = None;
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
        self.hold(discovery.epoch);
        discovery
    }

    /// Record the generation this fixture's resident published.
    ///
    /// A real resident writes this line the instant it takes the lock and
    /// rewrites it with its own generation before publishing anything, so a
    /// fixture holding the lock without one is a state no resident produces —
    /// and every case here that expects an admission would be asserting
    /// against a record the client is right to refuse.
    fn hold(&self, epoch: CanonicalUuid) {
        self.publisher
            .as_ref()
            .expect("fixture resident still holds its lock")
            .rewrite_record(holder_record(std::process::id(), Some(epoch)).as_bytes())
            .expect("fixture holder record writes");
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
        (&["task", "list"][..], CommandKind::BoardRead),
        (&["task", "next"][..], CommandKind::ClaimWin),
        (&["task", "recover"][..], CommandKind::ClaimWin),
        (&["task", "status"][..], CommandKind::BoardRead),
        (&["task", "reclaim"][..], CommandKind::ClaimRenew),
        (&["task", "capture"][..], CommandKind::ClaimWin),
        (&["task", "add"][..], CommandKind::TaskAdd),
        (&["task", "done"][..], CommandKind::TaskDone),
        (&["task", "review"][..], CommandKind::TaskReview),
        (&["task", "note"][..], CommandKind::TaskNote),
        (&["task", "defer"][..], CommandKind::TaskDefer),
        (&["task", "wake"][..], CommandKind::TaskWake),
        (&["task", "free"][..], CommandKind::ClaimFree),
        (&["task", "oops"][..], CommandKind::TaskOopsAdd),
        (&["task", "depends"][..], CommandKind::TaskDepends),
        (&["task", "modify"][..], CommandKind::TaskEdit),
        (&["task", "delete"][..], CommandKind::TaskDelete),
        (&["doc"][..], CommandKind::DocApply),
        (&["doc", "ledger"][..], CommandKind::DocLedger),
        (&["taste", "add"][..], CommandKind::TaskAdd),
        (&["taste", "drop"][..], CommandKind::TaskAdd),
        (&["taste", "mine"][..], CommandKind::TasteProposalAdd),
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
fn a_transport_failure_names_the_elapsed_bound_before_it_names_a_direction() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("classifier peer binds");
    let address = listener.local_addr().expect("classifier peer address");
    let bound = Duration::from_secs(3);
    let mut stream = DeadlineStream::connect(&address, bound).expect("classifier peer accepts");

    for (recorded, assumed, reason) in [
        (None, IoDirection::Read, "canvas::receive-failed"),
        (None, IoDirection::Write, "canvas::send-failed"),
        (
            Some((IoDirection::Read, false)),
            IoDirection::Write,
            "canvas::receive-failed",
        ),
        (
            Some((IoDirection::Write, false)),
            IoDirection::Read,
            "canvas::send-failed",
        ),
        (
            Some((IoDirection::Read, true)),
            IoDirection::Read,
            "canvas::deadline-elapsed",
        ),
        (
            Some((IoDirection::Write, true)),
            IoDirection::Read,
            "canvas::deadline-elapsed",
        ),
    ] {
        stream.last_failure = recorded.map(|(direction, deadline_elapsed)| IoFailure {
            direction,
            deadline_elapsed,
        });
        let Outcome::Fault(fault) = transport_fault(&stream, "peer detail", assumed) else {
            panic!("a transport failure must fault");
        };
        assert_eq!(fault.reason, reason);
        assert_eq!(fault.class, FaultClass::Environment);
        assert!(fault.detail.contains("peer detail"), "{fault:?}");
        assert_eq!(
            fault.detail.contains("3s"),
            reason == "canvas::deadline-elapsed",
            "only an elapsed bound names itself: {fault:?}"
        );
    }
}

/// A command whose answer contends with the host declares the wider bound.
///
/// Every other command keeps the bound an answer composed from held state
/// needs, so an unanswering peer is still detected as promptly as before.
#[test]
fn the_answer_bound_registry_widens_exactly_the_non_held_state_commands() {
    let contended = [
        CommandKind::LaneSeat,
        CommandKind::LaneState,
        CommandKind::ReleasePublish,
        CommandKind::UpChange,
    ];
    for kind in CommandKind::ALL {
        assert_eq!(
            crate::cli::answer_bound(*kind),
            if contended.contains(kind) {
                Duration::from_secs(20)
            } else {
                Duration::from_secs(3)
            },
            "{kind} declares the wrong answer bound",
        );
    }
}

#[test]
fn discovery_replacement_is_detected_by_file_identity() {
    let fixture = Fixture::new("replacement");
    let original = fixture.publish();
    let read = DiscoveryRead::open(&fixture.canvas, &fixture.root.join("resident"))
        .expect("discovery opens");
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
fn canvas_refuses_missing_malformed_and_unresolved_discovery_without_launching() {
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

    // A name is not a socket. The address a client dials has to be the one the
    // resident published, not whatever a resolver answers with at the moment
    // the client asks, so an endpoint that needs resolving is refused unread.
    canvas::Discovery {
        endpoint: "http://canvas.example:48123".to_owned(),
        capability: uuid(3),
        epoch: uuid(4),
    }
    .publish(&fixture.canvas)
    .expect("unresolved fixture publishes");
    let unresolved = open_canvas_with(
        &fixture.canvas,
        || {},
        |_| {
            launches += 1;
            Ok(())
        },
    );
    let Outcome::Fault(unresolved) = unresolved else {
        panic!("named-host discovery must fault");
    };
    assert_eq!(unresolved.reason, "canvas::endpoint-invalid");
    assert_eq!(launches, 0, "no refused discovery reached the launcher");
}

/// A surface the operator bound off loopback is still opened, not refused.
///
/// The client once required a loopback endpoint, which would now refuse the
/// exact address `spice up --host` was told to bind: the resident would be
/// running and reachable while every local command declined to dial it. The
/// endpoint has to be one literal socket, and past that the client's business
/// is whether the discovery is current, not which interface it names.
#[test]
fn canvas_opens_a_discovery_naming_an_address_the_operator_bound() {
    let fixture = Fixture::new("canvas-routable");
    let mut opened = None;
    canvas::Discovery {
        endpoint: "http://192.0.2.1:48123".to_owned(),
        capability: uuid(3),
        epoch: uuid(4),
    }
    .publish(&fixture.canvas)
    .expect("routable fixture publishes");
    fixture.hold(uuid(4));

    let outcome = open_canvas_with(
        &fixture.canvas,
        || {},
        |endpoint| {
            opened = Some(endpoint.to_owned());
            Ok(())
        },
    );

    assert!(
        matches!(outcome, Outcome::Reply(_)),
        "a routable discovery opens: {outcome:?}"
    );
    assert_eq!(opened.as_deref(), Some("http://192.0.2.1:48123"));
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

include!("tests/late.rs");
