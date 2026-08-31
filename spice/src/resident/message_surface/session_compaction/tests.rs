use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let serial = FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-session-compaction-{}-{serial}-{label}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).expect("fixture directory exists");
        Self(path)
    }

    fn transcript(&self) -> PathBuf {
        self.0.join("rollout.jsonl")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Messages(Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for Messages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut messages = self.0.lock().expect("message lock");
        if let Some(existing) = messages
            .iter()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        {
            return if existing == &message {
                Ok(false)
            } else {
                Err(canvas::stream::MessagePublishError::IdentityConflict)
            };
        }
        messages.push(message);
        Ok(true)
    }
}

#[test]
fn real_codex_rollout_compaction_is_published_once_from_complete_jsonl() {
    let scratch = Scratch::new("codex");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        concat!(
            "{\"timestamp\":\"2030-04-12T18:44:08.000Z\",\"type\":\"response_item\",",
            "\"payload\":{\"type\":\"message\",\"role\":\"assistant\",",
            "\"content\":[{\"type\":\"output_text\",\"text\":\"before\"}]}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"compacted\",",
            "\"payload\":{\"message\":\"\",\"replacement_history\":[",
            "{\"type\":\"message\",\"role\":\"user\",\"content\":[]},",
            "{\"type\":\"compaction\",\"encrypted_content\":\"ciphertext\"}],",
            "\"window_number\":7}}\n",
            "{\"timestamp\":\"2030-04-12T18:44:09.272Z\",\"type\":\"event_msg\",",
            "\"payload\":{\"type\":\"context_compacted\"}}\n",
        ),
    )
    .expect("real-format Codex transcript is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    let records = scan(&mut cursor).expect("session transcript scans");
    assert_eq!(records.len(), 1, "only the completed boundary is visible");
    assert_eq!(
        records[0].authored.as_deref(),
        Some("2030-04-12T18:44:09.271Z")
    );
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));

    let publisher = Messages::default();
    publish(
        records.into_iter().next().expect("one compaction"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        &publisher,
        &mut MessageState::default(),
    )
    .expect("typed boundary publishes");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert_eq!(messages[0].text, "Context compacted");
    drop(messages);

    assert!(
        scan(&mut cursor)
            .expect("unchanged transcript scans")
            .is_empty(),
        "the cursor does not replay a consumed boundary",
    );
}

#[test]
fn incomplete_boundary_waits_for_its_jsonl_delimiter() {
    let scratch = Scratch::new("partial");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"comp",
    )
    .expect("partial record is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Codex);

    assert!(scan(&mut cursor).expect("partial scan succeeds").is_empty());
    assert_eq!(cursor.next, 0, "pending bytes never advance the cursor");
    append(&transcript, b"acted\",\"payload\":{}}\n");
    assert_eq!(scan(&mut cursor).expect("completed scan succeeds").len(), 1,);
}

#[test]
fn claude_preserved_boundary_uses_the_same_typed_projection() {
    let scratch = Scratch::new("claude");
    let transcript = scratch.transcript();
    fs::write(
        &transcript,
        b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    )
    .expect("Claude boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    let records = scan(&mut cursor).expect("Claude session scans");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].authored, None);
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));

    let publisher = Messages::default();
    publish(
        records.into_iter().next().expect("one compaction"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163e5"),
        uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        &publisher,
        &mut MessageState::default(),
    )
    .expect("timestamp-less Claude boundary publishes");
    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(messages[0].authored_synthetic);
    assert_eq!(
        messages[0].locus.member,
        session_member("018f7777-7777-7444-8888-999999999999"),
        "synthetic chronology does not erase the preserved-session identity",
    );
    assert_eq!(
        messages[0].authored,
        "2000-01-01T00:00:00.000Z"
            .parse::<wire::Instant>()
            .expect("session synthetic authored instant parses"),
    );
}

#[test]
fn timestamp_less_claude_boundary_survives_live_poll_and_resident_reload_once() {
    const THREAD: &str = "018f7777-7777-7444-8888-999999999999";

    let scratch = Scratch::new("claude-synchronize");
    let transcript = scratch
        .0
        .join("projects")
        .join(format!("session-{THREAD}.jsonl"));
    fs::create_dir_all(transcript.parent().expect("transcript parent"))
        .expect("Claude project directory exists");
    fs::write(&transcript, b"").expect("empty Claude session exists at initial sync");
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163e5");
    let author = uuid("018f82d4-6f59-7d06-92e3-bce9131163ef");
    let residency = session_residency(lane, author, THREAD);
    let publisher = Messages::default();
    let mut state = observed_state(lane);
    let mut diagnostic = Vec::new();
    let mut compactions = SessionCompactions::new(claude_environment(&scratch.0));

    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("initial synchronization remains live before a boundary");
    append(
        &transcript,
        b"{\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    );
    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("live polling publishes the timestamp-less boundary");
    compactions
        .synchronize(&residency, &publisher, &mut state, &mut diagnostic)
        .expect("an unchanged live poll remains quiet");

    let mut reloaded = SessionCompactions::new(claude_environment(&scratch.0));
    reloaded
        .synchronize(
            &residency,
            &publisher,
            &mut observed_state(lane),
            &mut diagnostic,
        )
        .expect("resident reload replays the same durable identity without fault");

    let messages = publisher.0.lock().expect("message lock");
    assert_eq!(messages.len(), 1, "poll and reload retain one boundary");
    assert_eq!(messages[0].activity, wire::AgentActivityKind::Compaction);
    assert!(messages[0].authored_synthetic);
    assert_eq!(
        messages[0].observed,
        Some(observation_instant()),
        "the durable lane observation places the late session boundary",
    );
    assert!(
        diagnostic.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostic)
    );
}

fn observed_state(lane: CanonicalUuid) -> MessageState {
    MessageState {
        previews: CallPreviews::default(),
        observations: MessageObservations(vec![MessageObservation {
            lane,
            source: uuid("018f82d4-6f59-7d06-92e3-bce9131163bb"),
            from: wire::body::TranscriptPosition {
                member: 1,
                offset: 0,
            },
            to: wire::body::TranscriptPosition {
                member: 1,
                offset: 1,
            },
            at: observation_instant(),
        }]),
    }
}

fn observation_instant() -> wire::Instant {
    "2030-04-12T18:44:09.000Z"
        .parse()
        .expect("fixture observation instant")
}

fn claude_environment(home: &Path) -> crate::session_command::Environment {
    crate::session_command::Environment::new(
        vec![crate::session_command::VendorEnvironment::new(
            &spice::extension::CLAUDE,
            None,
            Some(home.as_os_str().to_owned()),
        )],
        None,
        None,
    )
}

fn session_residency(
    lane: CanonicalUuid,
    author: CanonicalUuid,
    thread: &str,
) -> wire::ResidencyHead {
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163aa");
    wire::ResidencyHead {
        epoch,
        state: wire::ResidentState::Seated,
        topology_revision: 1,
        lanes: vec![wire::LaneResidency {
            lane,
            lineage_id: lane,
            name: None,
            worktree: "/tmp/session-compaction".to_owned(),
            life: "drive".to_owned(),
            vendor: "claude".parse().expect("fixture vendor"),
            occupant: Some(author),
            lineage: vec![author],
            capabilities: Vec::new(),
            session: Some(wire::LaneSession {
                lane,
                occupant: author,
                worktree: "/tmp/session-compaction".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                launch: None,
                epoch,
                stream: "stdout".to_owned(),
                identity: thread.to_owned(),
                locus: wire::TapeLocus {
                    member: 1,
                    offset: 0,
                    length: 1,
                },
            }),
            disposition: "occupied".to_owned(),
        }],
        subscriptions: Vec::new(),
    }
}

fn cursor(path: &Path, dialect: listen::Dialect) -> Cursor {
    Cursor {
        thread: "018f7777-7777-7444-8888-999999999999".to_owned(),
        path: path.to_path_buf(),
        dialect,
        author: uuid("018f82d4-6f59-7d06-92e3-bce9131163ef"),
        member: session_member("018f7777-7777-7444-8888-999999999999"),
        next: 0,
    }
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .expect("transcript opens")
        .write_all(bytes)
        .expect("transcript append succeeds");
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}
