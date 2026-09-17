use super::*;

/// `sockaddr_un.sun_path` on Darwin, the tightest platform bound.
const SUN_PATH_BYTES: usize = 104;

use listen::{Dialect, Locus};

/// How long a test waits for the driver to answer before it fails.
const ANSWER_BOUND: std::time::Duration = std::time::Duration::from_secs(2);

/// The driver's command stream: one request per line, with a wake per line.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>, Arc<std::sync::Condvar>);

impl Sink {
    fn frames(&self) -> Vec<serde_json::Value> {
        self.answered(0)
    }

    /// Frames the driver has written, once it has written `frames` of them.
    fn answered(&self, frames: usize) -> Vec<serde_json::Value> {
        let written = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (written, _) = self
            .1
            .wait_timeout_while(written, ANSWER_BOUND, |written| {
                written.iter().filter(|byte| **byte == b'\n').count() < frames
            })
            .unwrap_or_else(PoisonError::into_inner);
        written
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("one JSON request per line"))
            .collect()
    }
}

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut written = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        written.extend_from_slice(bytes);
        self.1.notify_all();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-command-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        fs::create_dir_all(&root).expect("scratch directory");
        Self(root)
    }

    fn inbox(&self) -> PathBuf {
        self.0.join("lane")
    }

    fn terminal(&self) -> Terminal {
        Terminal::new(
            "/bin/sh",
            &self.0,
            sill::Environment::empty(),
            super::super::terminal::Jobs::new(self.0.join("jobs")),
        )
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn decoded(bytes: &[u8]) -> Decoded {
    listen::decode(Dialect::Omp, bytes, Locus::new(1, 0, bytes.len() as u64))
        .expect("matching locus")
}

/// Release a command holding on a line from the fifo. The hold is a `read`,
/// not a `cat` to the end: on macOS a writer that opens and closes while the
/// reader's own open completes can leave the reader without its end (about
/// once in 3000 under load), while a line is always delivered.
fn release(fifo: &Path) {
    fs::write(fifo, "\n").expect("the fifo takes a line");
}

/// Every job of the scratch has ended, so none outlives the test process
/// whose thread keeps it.
fn settled(scratch: &Scratch) {
    let jobs = super::super::terminal::Jobs::new(scratch.0.join("jobs"));
    for entry in fs::read_dir(scratch.0.join("jobs")).expect("the jobs directory") {
        let id = entry.expect("a job directory").file_name();
        let job = jobs
            .open(id.to_str().expect("a job id"))
            .expect("the job opens");
        let _ = job.observe(&Control::new(), ANSWER_BOUND);
        assert!(job.ended().is_some(), "job {} never ends", job.id());
    }
}

fn steer(sequence: u64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("spice-steer-{sequence}"),
        "type": "prompt",
        "message": message,
        "streamingBehavior": "steer",
    })
}

#[test]
fn the_inbox_is_interjected_once_per_change_and_again_after_a_compaction() {
    let scratch = Scratch::new("changes");
    let inbox = scratch.inbox();
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), &inbox, None);
    driver
        .open(b"{\"id\":\"spice-state\",\"type\":\"get_state\"}\n")
        .expect("open");

    // Missing, then empty: nothing to say.
    driver.deliver_inbox();
    fs::write(&inbox, b"").expect("empty inbox");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 0);

    fs::write(&inbox, b"<key>\ndirective=d1\nstop\n</key>\n").expect("inbox");
    driver.deliver_inbox();
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 1);

    fs::write(&inbox, b"<key>\ndirective=d2\ngo\n</key>\n").expect("inbox");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 2);

    // The agent lost its context, so the standing inbox is said again.
    driver.observe(&decoded(
        br#"{"type":"auto_compaction_end","result":{"summary":"so far"},"aborted":false}"#,
    ));
    assert_eq!(driver.interjections(), 3);

    // A turn boundary that is not the end changes nothing.
    driver.observe(&decoded(br#"{"type":"agent_start"}"#));
    assert_eq!(driver.interjections(), 3);

    assert_eq!(
        sink.frames(),
        [
            serde_json::json!({"id": "spice-state", "type": "get_state"}),
            steer(1, "<key>\ndirective=d1\nstop\n</key>\n"),
            steer(2, "<key>\ndirective=d2\ngo\n</key>\n"),
            steer(3, "<key>\ndirective=d2\ngo\n</key>\n"),
        ]
    );
}

#[test]
fn a_prompt_nobody_can_compose_is_declined_while_notices_pass() {
    let scratch = Scratch::new("dialog");
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), scratch.inbox(), None);
    assert!(
        driver
            .observe(&decoded(
                br#"{"type":"extension_ui_request","id":"7","method":"notify","message":"hi"}"#,
            ))
            .is_none()
    );
    assert!(
        driver
            .observe(&decoded(
                br#"{"type":"extension_ui_request","id":"8","method":"select","title":"Which?","options":["a","b"]}"#,
            ))
            .is_none()
    );
    assert_eq!(
        sink.frames(),
        [serde_json::json!({
            "type": "extension_ui_response",
            "id": "8",
            "cancelled": true,
        })]
    );
}

#[test]
fn a_shell_call_runs_in_the_lanes_terminal_and_answers_with_what_it_said() {
    let scratch = Scratch::new("shell");
    let sink = Sink::default();
    let terminal = scratch.terminal();
    let driver = Driver::new(sink.clone(), scratch.inbox(), Some(terminal));
    assert!(
        driver
            .observe(&decoded(
                br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_1","toolName":"shell","arguments":{"command":"printf hi; exit 3"}}"#,
            ))
            .is_none()
    );
    let frames = sink.answered(1);
    assert_eq!(frames.len(), 1, "{frames:?}");
    let text = frames[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(
        text.starts_with("hi\n\nWall time: ") && text.ends_with("Command exited with code 3"),
        "{text}"
    );
    assert_eq!(frames[0]["result"]["details"]["exitCode"], 3);
    assert_eq!(frames[0]["result"]["isError"], true);
}

#[test]
fn a_call_the_lane_cannot_run_is_refused_at_once() {
    let scratch = Scratch::new("refused");
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), scratch.inbox(), None);
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_1","toolName":"shell","arguments":{"command":"true"}}"#,
    ));
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h2","toolCallId":"call_2","toolName":"deploy","arguments":{}}"#,
    ));
    let terminal = scratch.terminal();
    let driver = Driver::new(sink.clone(), scratch.inbox(), Some(terminal));
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h3","toolCallId":"call_3","toolName":"shell","arguments":{"timeout":5}}"#,
    ));
    let refused = |id: &str, reason: &str| {
        serde_json::json!({
            "type": "host_tool_result",
            "id": id,
            "result": {"content": [{"type": "text", "text": reason}]},
            "isError": true,
        })
    };
    assert_eq!(
        sink.frames(),
        [
            refused("h1", "this lane has no terminal"),
            refused("h2", "deploy is not a tool this lane registered"),
            refused("h3", "shell needs a command"),
        ]
    );
}

#[test]
fn a_withdrawn_call_and_a_drain_end_the_commands_they_cover() {
    let scratch = Scratch::new("withdrawn");
    let sink = Sink::default();
    let terminal = scratch.terminal();
    let driver = Driver::new(sink.clone(), scratch.inbox(), Some(terminal));
    // `cat` on a closed stdin ends on its own; a fifo nobody opens does
    // not, so the command holds until it is ended.
    let fifo = scratch.0.join("hold");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(status.success());
    let call = |id: &str| {
        format!(
            r#"{{"type":"host_tool_call","id":"{id}","toolCallId":"call_{id}","toolName":"shell","arguments":{{"command":"cat {}"}}}}"#,
            fifo.display()
        )
    };
    driver.observe(&decoded(call("h1").as_bytes()));
    driver.observe(&decoded(call("h2").as_bytes()));
    driver.observe(&decoded(
        br#"{"type":"host_tool_cancel","id":"h3","targetId":"h1"}"#,
    ));
    let first = sink.answered(1);
    assert_eq!(first.len(), 1, "only the withdrawn command has ended");
    assert_eq!(first[0]["id"], "h1");
    assert!(
        first[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .ends_with("Command aborted")
    );

    // A drain ends the other command too, then ends the input, so its
    // answer has nowhere to go: omp has already withdrawn every call.
    driver.abort();
    assert!(
        driver
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );
    let all = sink.answered(2);
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[1],
        serde_json::json!({"id": "spice-abort", "type": "abort"})
    );
    settled(&scratch);
}

#[test]
fn a_message_backgrounds_the_running_command_and_its_ending_is_delivered_as_steering() {
    let scratch = Scratch::new("steered");
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), scratch.inbox(), Some(scratch.terminal()));
    let fifo = scratch.0.join("hold");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_1","toolName":"shell","arguments":{"command":"read held < hold"}}"#,
    ));
    fs::write(scratch.inbox(), "look at this").expect("inbox renders");
    driver.deliver_inbox();

    // The steer and the interrupted call's answer are written by two
    // threads, so only their presence is fixed.
    let frames = sink.answered(2);
    assert!(frames.contains(&steer(1, "look at this")), "{frames:?}");
    let answer = frames
        .iter()
        .find(|frame| frame["id"] == "h1")
        .expect("the call is answered");
    let text = answer["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    let (notice, job) = text
        .split_once("Backgrounded as job ")
        .expect("the answer names the job");
    assert_eq!(
        notice,
        "Backgrounded early to handle an incoming message; the command keeps running.\n\n"
    );
    let job = job.trim_end_matches("; result will be delivered automatically.");
    assert_eq!(
        answer["result"]["details"]["exitCode"],
        serde_json::Value::Null
    );
    assert!(
        driver
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    );

    // One line through the fifo ends the read, so the job ends on its own
    // and its ending is delivered in omp's notice.
    release(&fifo);
    let frames = sink.answered(3);
    assert_eq!(frames.len(), 3, "{frames:?}");
    let delivery = frames
        .iter()
        .find(|frame| frame["id"] == "spice-steer-2")
        .and_then(|frame| frame["message"].as_str())
        .expect("the ending is delivered as the second steer");
    assert!(
        delivery.starts_with(&format!(
            "<system-notice>\nBackground job {job} has completed. Resume your work using the \
             result below.\n(no output)\n\nWall time: "
        )) && delivery.ends_with(" seconds\n</system-notice>"),
        "{delivery}"
    );
    driver.observe(&decoded(
        format!(
            r#"{{"type":"host_tool_call","id":"h2","toolCallId":"call_2","toolName":"job","arguments":{{"op":"logs","id":"{job}"}}}}"#
        )
        .as_bytes(),
    ));
    let frames = sink.answered(4);
    let logs = frames[3]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(
        logs.starts_with(&format!(
            "[job {job}] read held < hold\n(no output)\n\nWall time: "
        )),
        "{logs}"
    );
    assert_eq!(frames[3]["result"]["details"]["exitCode"], 0);
}

#[test]
fn a_confirm_is_stated_as_one_request_and_answered_once_consent_rules() {
    let scratch = Scratch::new("confirm");
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), scratch.inbox(), None);
    let ask = driver
        .observe(&decoded(
            br#"{"type":"extension_ui_request","id":"9","method":"confirm","title":"Deploy","message":"Deploy to prod?"}"#,
        ))
        .expect("a confirm raises a request");
    assert_eq!(ask.operation, "Deploy: Deploy to prod?");
    assert!(
        sink.frames().is_empty(),
        "the dialog holds until consent rules"
    );

    driver.answer(super::super::super::new_uuid(), true);
    assert!(
        sink.frames().is_empty(),
        "a request nobody raised here says nothing"
    );

    driver.answer(ask.request, false);
    driver.answer(ask.request, true);
    assert_eq!(
        sink.frames(),
        [serde_json::json!({
            "type": "extension_ui_response",
            "id": "9",
            "confirmed": false,
        })],
        "one request answers its dialog exactly once"
    );

    let untitled = driver
        .observe(&decoded(
            br#"{"type":"extension_ui_request","id":"10","method":"confirm","title":"","message":"Sure?"}"#,
        ))
        .expect("a confirm raises a request");
    assert_eq!(untitled.operation, "Sure?");
}

#[test]
fn a_kick_names_either_the_inbox_or_one_consent_answer() {
    let request = super::super::super::new_uuid();
    let answer = Kick::Answer {
        request,
        confirmed: true,
    };
    let bytes = serde_json::to_vec(&answer).expect("a kick serializes");
    assert!(bytes.len() <= KICK_BYTES, "{}", bytes.len());
    assert_eq!(
        serde_json::from_slice::<Kick>(&bytes).expect("a kick reads back"),
        answer
    );
    assert_eq!(
        serde_json::to_string(&Kick::Inbox).expect("a kick serializes"),
        r#"{"kind":"inbox"}"#
    );
    assert_eq!(
        serde_json::to_string(&Kick::Abort).expect("a kick serializes"),
        r#"{"kind":"abort"}"#
    );
}

#[test]
fn a_drain_interrupts_the_turn_and_ends_the_input() {
    let scratch = Scratch::new("abort");
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), scratch.inbox(), None);
    driver.abort();
    assert_eq!(
        sink.frames(),
        [serde_json::json!({"id": "spice-abort", "type": "abort"})]
    );
    assert_eq!(
        driver.open(b"late\n").expect_err("closed").kind(),
        io::ErrorKind::BrokenPipe
    );

    let door_path = kick_path_beneath(Path::new("/tmp"), &format!("abort-{}", std::process::id()));
    let door = std::os::unix::net::UnixDatagram::bind(&door_path).expect("door binds");
    abort(&door_path).expect("a bound door takes the interrupt");
    let mut kick = [0u8; KICK_BYTES];
    let length = door.recv(&mut kick).expect("the interrupt arrives");
    assert_eq!(
        serde_json::from_slice::<Kick>(&kick[..length]).expect("a kick"),
        Kick::Abort
    );
    drop(door);
    fs::remove_file(door_path).expect("door cleanup");
    assert!(
        abort(&scratch.0.join("nobody.sock")).is_err(),
        "without a door there is nobody to interrupt"
    );
}

#[test]
fn the_end_of_the_turn_closes_the_stream_and_later_deliveries_are_dropped() {
    let scratch = Scratch::new("closed");
    let inbox = scratch.inbox();
    let sink = Sink::default();
    let driver = Driver::new(sink.clone(), &inbox, None);
    driver.observe(&decoded(br#"{"type":"agent_end","messages":[]}"#));

    fs::write(&inbox, b"<key>\ndirective=d1\nstop\n</key>\n").expect("inbox");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 1, "the attempt is counted");
    assert!(sink.frames().is_empty(), "nothing reaches a closed stream");
    assert_eq!(
        driver.open(b"late\n").expect_err("closed").kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn the_kick_door_is_a_short_content_addressed_leaf() {
    let identity = blake3::hash(b"lane").to_string();
    let path = kick_path_beneath(Path::new("/tmp"), &identity);
    assert!(
        path.as_os_str().len() < SUN_PATH_BYTES,
        "{}",
        path.display()
    );
    assert!(path.file_name().is_some_and(|name| {
        name.to_string_lossy().starts_with("spice-k-") && name.to_string_lossy().ends_with(".sock")
    }));
}
