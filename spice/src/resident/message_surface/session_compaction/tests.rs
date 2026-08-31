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
        self.0.lock().expect("message lock").push(message);
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
        b"{\"timestamp\":\"2030-04-12T18:44:09.271Z\",\"type\":\"system\",\"subtype\":\"compact_boundary\"}\n",
    )
    .expect("Claude boundary is written");
    let mut cursor = cursor(&transcript, listen::Dialect::Claude);
    let records = scan(&mut cursor).expect("Claude session scans");

    assert_eq!(records.len(), 1);
    assert!(matches!(records[0].decoded, listen::Event::Compaction(_)));
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
