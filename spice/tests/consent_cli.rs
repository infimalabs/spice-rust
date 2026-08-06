#![cfg(unix)]

use std::fs;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use serde::de::DeserializeOwned;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::body::{ConsentAsked, ConsentGranted, ConsentQueueCause, ConsentQueued};
use wire::intent::{ConsentAnswer, ConsentPolicyAnswer, ConsentQueueAnswer};
use wire::{
    Actor, CanonicalUuid, Command as WireCommand, CommandKind, FactKind, Instant, Outcome,
    Producer, Refusal, ReplyPosition, Response, WireVersion, Witness,
};

#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const REFUSAL_EXIT: i32 = 2;
static NEXT: AtomicU64 = AtomicU64::new(0);

const POLICY: &str = r#"
[consent]
posture = "queue"

[[consent.rule]]
name = "bounded"
tool = "shell:*"
answer = "allow"
budget = 1

[[consent.rule]]
name = "review"
tool = "deploy:*"
answer = "queue"
"#;

struct Fixture {
    root: PathBuf,
    journal: PathBuf,
    canvas: PathBuf,
    lane: CanonicalUuid,
    spent: CanonicalUuid,
    renew: CanonicalUuid,
    allow: CanonicalUuid,
    deny: CanonicalUuid,
    pressure: CanonicalUuid,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-consent-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".git")).expect("fixture Git directory");
        staged_app::seed(&root);
        fs::write(root.join("spice.toml"), POLICY).expect("tracked policy");
        let runtime = root.join(".git/.spice");
        let fixture = Self {
            journal: runtime.join("data/spicejournal.sqlite3"),
            canvas: runtime.join("canvas"),
            root,
            lane: uuid(1),
            spent: uuid(2),
            renew: uuid(3),
            allow: uuid(4),
            deny: uuid(5),
            pressure: uuid(6),
        };
        seed_consent(&fixture);
        fixture
    }

    fn invoke(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(&self.root)
            .output()
            .expect("spice consent command launches")
    }

    fn discovery(&self) -> canvas::Discovery {
        wire::decode(&fs::read(&self.canvas).expect("discovery reads")).expect("discovery decodes")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            // A fixture resident asks the kernel for a free port: the fixed default
            // belongs to the operator's one resident, and concurrent fixtures on a
            // shared machine cannot all have it.
            .args(["--json", "up", "--port", "0"])
            .current_dir(&fixture.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("resident starts");
        let mut resident = Self { child: Some(child) };
        let watched = fixture.canvas.parent().expect("canvas has runtime parent");
        fs::create_dir_all(watched).expect("runtime directory");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || fixture.canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => panic!("resident exited before readiness"),
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn consent_queue_rulings_reconnects_and_restart_are_durable() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);

    let initial = text_reply::<ConsentQueueAnswer>(&fixture.invoke(&["consent"]));
    assert_eq!(initial.pending.len(), 4);
    assert_eq!(initial.budgets[0].remaining, 0);
    assert_eq!(
        initial
            .pending
            .iter()
            .filter(|pending| {
                matches!(
                    pending.cause,
                    ConsentQueueCause::BudgetExhausted { bound: 1 }
                )
            })
            .count(),
        2
    );

    let policy = text_reply::<ConsentPolicyAnswer>(&fixture.invoke(&["consent", "policy"]));
    assert_eq!(policy.rules.len(), 2);
    assert_eq!(policy.rules[0].remaining, Some(0));

    let reconnected = json_reply::<ConsentQueueAnswer>(&fixture.invoke(&["--json", "consent"]));
    assert_eq!(reconnected.body, initial);
    assert!(matches!(reconnected.position, ReplyPosition::Snapshot(_)));

    let allowed = text_reply::<ConsentAnswer>(&fixture.invoke(&[
        "consent",
        "allow",
        &fixture.allow.to_string(),
    ]));
    assert_eq!(allowed.answer, wire::Answer::Granted);

    let denied = json_reply::<ConsentAnswer>(&fixture.invoke(&[
        "--json",
        "consent",
        "deny",
        &fixture.deny.to_string(),
    ]));
    assert_eq!(denied.body.answer, wire::Answer::Denied);
    assert!(matches!(denied.position, ReplyPosition::Committed(_)));

    let renewed = json_reply::<ConsentAnswer>(&fixture.invoke(&[
        "--json",
        "consent",
        "allow",
        &fixture.renew.to_string(),
    ]));
    assert_eq!(renewed.body.answer, wire::Answer::Granted);

    let pressured = json_reply::<ConsentQueueAnswer>(&fixture.invoke(&["consent", "--json"]));
    assert_eq!(
        pressured
            .body
            .pending
            .iter()
            .map(|pending| pending.request)
            .collect::<Vec<_>>(),
        vec![fixture.pressure]
    );
    assert_eq!(pressured.body.budgets[0].remaining, 1);
    assert_eq!(
        pressured.body.pending[0].budget.as_ref().unwrap().remaining,
        1
    );

    mismatched_ruling(&fixture);
    let after_mismatch = text_reply::<ConsentQueueAnswer>(&fixture.invoke(&["consent"]));
    assert_eq!(after_mismatch.pending.len(), 1);
    assert_eq!(after_mismatch.pending[0].request, fixture.pressure);

    let duplicate = fixture.invoke(&["--json", "consent", "allow", &fixture.renew.to_string()]);
    assert_eq!(duplicate.status.code(), Some(REFUSAL_EXIT));
    let Outcome::Refusal(refusal) =
        wire::decode::<Outcome<ConsentAnswer>>(&duplicate.stderr).expect("duplicate is typed")
    else {
        panic!("duplicate decision did not refuse")
    };
    assert_eq!(refusal.reason, "consent::request-not-held");

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    let restored = text_reply::<ConsentQueueAnswer>(&fixture.invoke(&["consent"]));
    assert_eq!(restored, after_mismatch);

    let final_renewal = text_reply::<ConsentAnswer>(&fixture.invoke(&[
        "consent",
        "allow",
        &fixture.pressure.to_string(),
    ]));
    assert_eq!(final_renewal.answer, wire::Answer::Granted);
    assert!(
        text_reply::<ConsentQueueAnswer>(&fixture.invoke(&["consent"]))
            .pending
            .is_empty()
    );
    restarted.stop(&fixture);

    let snapshot = facts(&fixture);
    assert_eq!(kind_count(&snapshot, FactKind::ConsentGranted), 4);
    assert_eq!(kind_count(&snapshot, FactKind::ConsentDenied), 1);
    assert_eq!(
        journal::Journal::open(&fixture.journal)
            .expect("journal opens")
            .verify()
            .expect("authority verifies")
            .receipts,
        4
    );
}

fn seed_consent(fixture: &Fixture) {
    fs::create_dir_all(fixture.journal.parent().expect("journal parent"))
        .expect("journal directory");
    let journal = journal::Journal::open(&fixture.journal).expect("fixture journal opens");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let mut write = JournalWrite::new(
        Actor::Lane(fixture.lane),
        "consent:fixture".parse().expect("fixture origin"),
        "2030-04-12T18:44:09.271Z"
            .parse::<Instant>()
            .expect("fixture instant"),
        Producer::CommandDoor,
    );
    let spent = ask(&mut write, fixture.spent, fixture.lane, "shell:first");
    write
        .fact(
            ConsentGranted {
                request: fixture.spent,
                lane: fixture.lane,
                rule: "bounded".to_owned(),
                budget: Some(0),
            },
            FactCause::Pending(spent),
        )
        .expect("bounded grant");
    queue_budget(&mut write, fixture.renew, fixture.lane, "shell:second");
    queue_policy(&mut write, fixture.allow, fixture.lane, "deploy:allow");
    queue_policy(&mut write, fixture.deny, fixture.lane, "deploy:deny");
    queue_budget(&mut write, fixture.pressure, fixture.lane, "shell:pressure");
    assert!(matches!(
        bus.commit(write).expect("consent seed reaches writer"),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().expect("fixture bus stops");
}

fn ask(
    write: &mut JournalWrite,
    request: CanonicalUuid,
    lane: CanonicalUuid,
    operation: &str,
) -> journal::PendingFact {
    write
        .fact(
            ConsentAsked {
                request,
                lane,
                operation: operation.to_owned(),
            },
            FactCause::Root,
        )
        .expect("consent ask")
}

fn queue_budget(
    write: &mut JournalWrite,
    request: CanonicalUuid,
    lane: CanonicalUuid,
    operation: &str,
) {
    let asked = ask(write, request, lane, operation);
    write
        .fact(
            ConsentQueued {
                request,
                lane,
                rule: "bounded".to_owned(),
                cause: ConsentQueueCause::BudgetExhausted { bound: 1 },
            },
            FactCause::Pending(asked),
        )
        .expect("budget queue");
}

fn queue_policy(
    write: &mut JournalWrite,
    request: CanonicalUuid,
    lane: CanonicalUuid,
    operation: &str,
) {
    let asked = ask(write, request, lane, operation);
    write
        .fact(
            ConsentQueued {
                request,
                lane,
                rule: "review".to_owned(),
                cause: ConsentQueueCause::Policy {},
            },
            FactCause::Pending(asked),
        )
        .expect("policy queue");
}

fn mismatched_ruling(fixture: &Fixture) {
    let discovery = fixture.discovery();
    let authority = discovery
        .endpoint
        .strip_prefix("http://")
        .expect("loopback endpoint");
    let stream = TcpStream::connect(authority).expect("resident connects");
    let mut request = format!("ws://{authority}/socket")
        .into_client_request()
        .expect("socket request");
    request
        .headers_mut()
        .insert(ORIGIN, discovery.endpoint.parse().expect("origin header"));
    let (mut socket, _) = tungstenite::client(request, stream).expect("socket opens");
    send(
        &mut socket,
        &wire::Hello {
            wire: WireVersion::CURRENT,
            capability: discovery.capability,
            epoch: discovery.epoch,
        },
    );
    let _: wire::Accepted = receive(&mut socket);
    let id = uuid(20);
    send(
        &mut socket,
        &WireCommand {
            v: WireVersion::CURRENT,
            id,
            epoch: discovery.epoch,
            kind: CommandKind::ConsentDecide,
            expected: vec![
                Witness::Consent {
                    request: fixture.renew,
                    answer: wire::Answer::Queued,
                },
                Witness::Rule {
                    name: "bounded".parse().expect("rule name"),
                },
                Witness::Budget {
                    rule: "bounded".parse().expect("rule name"),
                    remaining: 1,
                },
            ],
            body: wire::intent::ConsentDecide {
                request: fixture.pressure,
                answer: wire::Answer::Granted,
            },
        },
    );
    let response: Response<serde_json::Value> = receive(&mut socket);
    assert_eq!(response.id, id);
    let Outcome::Refusal(Refusal { reason, .. }) = response.outcome else {
        panic!("mismatched ruling did not refuse")
    };
    assert_eq!(reason, "consent::witness-moved");
}

fn send<T: serde::Serialize>(socket: &mut WebSocket<TcpStream>, value: &T) {
    socket
        .send(Message::Binary(
            wire::encode(value).expect("value encodes").into(),
        ))
        .expect("socket sends");
}

fn receive<T: DeserializeOwned>(socket: &mut WebSocket<TcpStream>) -> T {
    loop {
        match socket.read().expect("socket reads") {
            Message::Binary(bytes) => return wire::decode(&bytes).expect("binary value decodes"),
            Message::Text(text) => {
                return wire::decode(text.as_bytes()).expect("text value decodes");
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Close(_) => panic!("resident closed before its answer"),
        }
    }
}

fn text_reply<T: DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    wire::decode(&output.stdout).expect("text reply body is typed")
}

fn json_reply<T: DeserializeOwned>(output: &Output) -> wire::Reply<T> {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<T>>(&output.stdout).expect("JSON outcome is typed")
    else {
        panic!("JSON command did not reply")
    };
    reply
}

fn facts(fixture: &Fixture) -> journal::FactSnapshot {
    journal::JournalReader::open(&fixture.journal)
        .expect("journal reader opens")
        .fact_snapshot()
        .expect("fact snapshot")
}

fn kind_count(snapshot: &journal::FactSnapshot, kind: FactKind) -> usize {
    snapshot
        .facts()
        .iter()
        .filter(|fact| fact.fact.kind() == kind)
        .count()
}

fn uuid(last: u8) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
        .parse()
        .expect("fixture UUID")
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
