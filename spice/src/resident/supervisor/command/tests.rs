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

struct Scratch(PathBuf, Arc<Mutex<Option<Vec<u8>>>>);

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
        Self(root, Arc::new(Mutex::new(None)))
    }

    fn publish(&self, rendering: impl AsRef<[u8]>) {
        *self.1.lock().unwrap_or_else(PoisonError::into_inner) = Some(rendering.as_ref().to_vec());
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
/// The seal the fixture driver's lane wraps its steering in.
const SEAL: &str = "Sk9DLj12R";

fn make_driver(sink: Sink, scratch: &Scratch, terminal: Option<Terminal>) -> Arc<Driver<Sink>> {
    let source = Arc::clone(&scratch.1);
    Driver::with_inbox_reader(
        sink,
        move || {
            source
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        },
        SEAL.parse().expect("the fixture seal is key-shaped"),
        terminal,
    )
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

/// The lane has heard how the job ended.
fn heard(scratch: &Scratch, job: &str) {
    let directory = scratch.0.join("jobs").join(job);
    let deadline = std::time::Instant::now() + ANSWER_BOUND;
    loop {
        let (sender, receiver) = std::sync::mpsc::channel();
        let observer = crate::resident::daemon::observer::DirectObserver::arm(&directory)
            .expect("the job directory is watched");
        crate::resident::daemon::observer::wait_for_change(observer, sender, drop);
        if directory.join("delivered").exists() {
            return;
        }
        receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or_else(|_| panic!("job {job} is never heard"));
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
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    driver
        .open(b"{\"id\":\"spice-state\",\"type\":\"get_state\"}\n")
        .expect("open");

    // Missing, then empty: nothing to say.
    driver.deliver_inbox();
    scratch.publish(b"");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 0);
    // The agent lost its context, so it hears how to acknowledge inline even
    // with nothing pending.
    driver.observe(&decoded(
        br#"{"type":"auto_compaction_end","result":{"summary":"empty"},"aborted":false}"#,
    ));
    assert_eq!(driver.interjections(), 1);

    scratch.publish(b"<key>\ndirective=d1 key=1kJK6N3R stage=new expires=never\nstop\n</key>\n");
    driver.deliver_inbox();
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 2);

    scratch.publish(b"<key>\ndirective=d2 key=1kJK6M0V stage=new expires=never\ngo\n</key>\n");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 3);

    // The agent lost its context, so the standing inbox is said again.
    driver.observe(&decoded(
        br#"{"type":"auto_compaction_end","result":{"summary":"so far"},"aborted":false}"#,
    ));
    assert_eq!(driver.interjections(), 4);

    // A turn boundary that is not the end changes nothing.
    driver.observe(&decoded(br#"{"type":"agent_start"}"#));
    assert_eq!(driver.interjections(), 4);

    assert_eq!(
        sink.frames(),
        [
            serde_json::json!({"id": "spice-state", "type": "get_state"}),
            steer(1, steer::inbox::POST_COMPACTION_NOTICE),
            steer(
                2,
                "<key>\ndirective=d1 key=1kJK6N3R stage=new expires=never\nstop\n</key>\n"
            ),
            steer(
                3,
                "<key>\ndirective=d2 key=1kJK6M0V stage=new expires=never\ngo\n</key>\n"
            ),
            steer(
                4,
                &steer::inbox::post_compaction(
                    "<key>\ndirective=d2 key=1kJK6M0V stage=new expires=never\ngo\n</key>\n"
                ),
            ),
        ]
    );
}

#[test]
fn a_prompt_nobody_can_compose_is_declined_while_notices_pass() {
    let scratch = Scratch::new("dialog");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
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
    let driver = make_driver(sink.clone(), &scratch, Some(terminal));
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
    // The answer told the lane how the command ended.
    assert!(scratch.terminal().undelivered().is_empty());
}

#[test]
fn a_call_the_lane_cannot_run_is_refused_at_once() {
    let scratch = Scratch::new("refused");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_1","toolName":"shell","arguments":{"command":"true"}}"#,
    ));
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h2","toolCallId":"call_2","toolName":"deploy","arguments":{}}"#,
    ));
    let terminal = scratch.terminal();
    let driver = make_driver(sink.clone(), &scratch, Some(terminal));
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
    let driver = make_driver(sink.clone(), &scratch, Some(terminal));
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

/// A rendering carrying one directive the lane has not heard, long enough for
/// the exact-key recovery pointer to replace its body on a reminder.
const NEWS: &str = "<seal>\ndirective=d1 key=1kJK6N3R stage=new expires=never\n\
                    look at this and answer it before the next call, because the operator is \
                    waiting on the answer; include the exact outcome, keep the running command \
                    healthy, and make the ACK or NAK visible before continuing with more tools\n\
                    </seal>\n";

const THREE_PENDING: &str = "<seal>\n\
    directive=d1 key=1kJK6N3R stage=new expires=never\none\n\
    directive=d2 key=1kJK6M0V stage=new expires=never\ntwo\n\
    directive=d3 key=1kJK7Sqy stage=new expires=never\nthree\n\
    </seal>\n";

const ONE_PENDING: &str =
    "<seal>\ndirective=d3 key=1kJK7Sqy stage=new expires=never\nthree\n</seal>\n";

#[test]
fn hidden_ack_attempts_receive_one_private_visible_prose_nudge() {
    let scratch = Scratch::new("hidden-ack");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    scratch.publish(THREE_PENDING);
    driver.deliver_inbox();

    let attempt = decoded(
        br#"{"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking","thinking":"private details. ACK 1kJK6N3R: done. NAK 1kJK6M0V: declined."}]}}"#,
    );
    driver.observe(&attempt);
    let frames = sink.answered(2);
    let nudge = frames[1]["message"].as_str().expect("nudge message");
    assert!(nudge.starts_with("<system-notice>\n"));
    assert!(nudge.ends_with("\n</system-notice>"));
    assert!(nudge.contains("1kJK6N3R"));
    assert!(nudge.contains("1kJK6M0V"));
    assert!(!nudge.contains("1kJK7Sqy"));
    assert!(!nudge.contains("private details"));

    driver.observe(&attempt);
    driver.observe(&decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"1kJK7Sqy bare; `ACK 1kJK7Sqy: quoted`; ACK 1kJKZZZZ: unknown."}]}}"#,
    ));
    assert_eq!(
        sink.frames().len(),
        2,
        "repetition and non-attempts stay silent"
    );

    scratch.publish(b"");
    driver.observe(&decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"ACK 1kJK7Sqy: no longer pending."}]}}"#,
    ));
    assert_eq!(sink.frames().len(), 2, "retired keys stay silent");

    scratch.publish(ONE_PENDING);
    driver.deliver_inbox();
    let remaining_attempt = decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"NAK 1kJK7Sqy: declined."}]}}"#,
    );
    driver.observe(&remaining_attempt);
    assert_eq!(sink.answered(4).len(), 4);

    driver.observe(&decoded(br#"{"type":"compaction"}"#));
    driver.observe(&remaining_attempt);
    assert_eq!(
        sink.answered(6).len(),
        6,
        "a lost context receives one fresh correction"
    );
}

/// A visible header naming a key the occupant does not hold hears so once, by
/// the exact token it wrote; keys it holds and mentions it did not author stay
/// silent.
#[test]
fn a_visible_header_naming_no_held_key_receives_one_private_correction() {
    let scratch = Scratch::new("misdirected-ack");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    scratch.publish(format!(
        "<{SEAL}>\n\
         directive=d1 key=dTrSSg90C stage=new expires=never\nfirst\n\
         directive=d2 key=dTtrHz4R3 stage=new expires=never\nsecond\n\
         </{SEAL}>\n"
    ));
    driver.deliver_inbox();
    // The first directive retires; the occupant was told it in this context.
    scratch.publish(format!(
        "<{SEAL}>\ndirective=d2 key=dTtrHz4R3 stage=new expires=never\nsecond\n</{SEAL}>\n"
    ));

    let reply = decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ACK dTrSSg90C is done. ACK dTrHz4R3: looking.\n\nACK dTtrHz4R3: fine. ACK Sk9DLj12R: seal. `ACK dTrHz4Rx: quoted`"}]}}"#,
    );
    driver.observe(&reply);
    let frames = sink.answered(2);
    assert_eq!(frames.len(), 2, "{frames:?}");
    let notice = frames[1]["message"].as_str().expect("correction text");
    assert!(notice.contains("dTrHz4R3"), "{notice}");
    assert!(notice.contains("dTtrHz4R3"), "{notice}");
    for held in ["dTrSSg90C", "Sk9DLj12R", "dTrHz4Rx"] {
        assert!(!notice.contains(held), "{held} stays silent: {notice}");
    }

    driver.observe(&reply);
    driver.observe(&decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"ACK dTrHz4R4: reasoning is not prose."}]}}"#,
    ));
    assert_eq!(
        sink.frames().len(),
        2,
        "a corrected token and reasoning stay silent"
    );

    driver.observe(&decoded(br#"{"type":"compaction"}"#));
    driver.observe(&reply);
    let frames = sink.frames();
    assert_eq!(
        frames.len(),
        4,
        "one pending replay and one typo correction"
    );
    let notice = frames[3]["message"].as_str().expect("correction text");
    assert!(notice.contains("dTrHz4R3"), "{notice}");
    assert!(
        !notice.contains("dTrSSg90C"),
        "a retired key delivered before compaction stays recognized: {notice}"
    );
}

#[test]
fn delivered_key_recognition_belongs_only_to_the_successfully_told_occupant() {
    let scratch = Scratch::new("recognition-lifetime");
    let rendering =
        format!("<{SEAL}>\ndirective=d1 key=dTrSSg90C stage=new expires=never\nfirst\n</{SEAL}>\n");
    let reply = decoded(
        br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"ACK dTrSSg90C: repeated."}]}}"#,
    );
    let sink = Sink::default();
    let told = make_driver(sink.clone(), &scratch, None);
    scratch.publish(&rendering);
    told.deliver_inbox();
    scratch.publish(b"");
    told.observe(&reply);
    assert_eq!(
        sink.frames().len(),
        1,
        "a delivered retired key stays silent"
    );

    let fresh_sink = Sink::default();
    let fresh = make_driver(fresh_sink.clone(), &scratch, None);
    fresh.observe(&reply);
    let frames = fresh_sink.frames();
    assert_eq!(frames.len(), 1, "a new occupant receives a correction");
    assert!(frames[0]["message"].as_str().unwrap().contains("dTrSSg90C"));

    let failed_sink = Sink::default();
    let failed = make_driver(failed_sink.clone(), &scratch, None);
    *failed.stdin.lock().unwrap_or_else(PoisonError::into_inner) = None;
    scratch.publish(&rendering);
    failed.deliver_inbox();
    scratch.publish(b"");
    *failed.stdin.lock().unwrap_or_else(PoisonError::into_inner) = Some(failed_sink.clone());
    failed.observe(&reply);
    let frames = failed_sink.frames();
    assert_eq!(frames.len(), 1, "failed delivery grants no recognition");
    assert!(frames[0]["message"].as_str().unwrap().contains("dTrSSg90C"));
}

#[test]
fn a_resend_of_heard_direction_costs_a_pointer_and_leaves_the_running_command_alone() {
    let scratch = Scratch::new("restated");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
    let fifo = scratch.0.join("hold");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo")
            .success()
    );
    scratch.publish(NEWS);
    driver.deliver_inbox();
    driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"h1","toolCallId":"call_1","toolName":"shell","arguments":{"command":"read held < hold"}}"#,
    ));

    // The same key said again, as a reminder is, waits for the turn and
    // points at the telling the occupant is still holding.
    let reminder = NEWS.replace("stage=new", "stage=reminded");
    scratch.publish(&reminder);
    driver.deliver_inbox();
    let pointer = steer::inbox::nudge(&reminder, |_| Some(steer::inbox::Told::Silent));
    let frames = sink.answered(2);
    assert_eq!(frames, [steer(1, NEWS), steer(2, &pointer)]);
    assert!(
        pointer.len() < reminder.len(),
        "a pointer that costs the telling is not a pointer",
    );
    assert_eq!(
        driver
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1
    );

    // A second key is news, so the call answers now, and the news is told in
    // full beside the pointer that replaced the heard one.
    let more = reminder.replace(
        "</seal>\n",
        "directive=d2 key=1kJK6M0V stage=new expires=never\ngo\n</seal>\n",
    );
    scratch.publish(&more);
    driver.deliver_inbox();
    let told = steer::inbox::nudge(&more, |key| {
        (key == "1kJK6N3R").then_some(steer::inbox::Told::Silent)
    });
    let frames = sink.answered(4);
    assert!(frames.contains(&steer(3, &told)), "{frames:?}");
    assert!(
        frames.iter().any(|frame| frame["id"] == "h1"),
        "the call is answered: {frames:?}"
    );
    release(&fifo);
    settled(&scratch);
}

/// The lane that reported a stalled ACK scanner had written its ACK headers
/// only in reasoning: eighteen tool-only records followed the telling. The
/// resend must say it saw no visible reply, and only visible prose changes
/// that.
#[test]
fn a_resend_says_whether_any_visible_reply_followed_the_telling() {
    let scratch = Scratch::new("replied");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    scratch.publish(NEWS);
    driver.deliver_inbox();

    driver.observe(&decoded(
        br#"{"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking","thinking":"ACK 1kJK6N3R: planned but never written"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"x"}}]}}"#,
    ));
    let reminder = NEWS.replace("stage=new", "stage=reminded");
    scratch.publish(&reminder);
    driver.deliver_inbox();

    driver.observe(&decoded(
        br#"{"type":"message_end","message":{"role":"assistant","stopReason":"toolUse","content":[{"type":"text","text":"Reading the next file."}]}}"#,
    ));
    let again = reminder.replace("expires=never", "expires=later");
    scratch.publish(&again);
    driver.deliver_inbox();

    let pointer = |rendering: &str, told| steer::inbox::nudge(rendering, |_| Some(told));
    let frames = sink.answered(4);
    assert_eq!(frames[0], steer(1, NEWS));
    assert!(
        frames[1]["message"]
            .as_str()
            .is_some_and(|message| message.contains("1kJK6N3R")),
        "the hidden attempt receives its private correction: {frames:?}"
    );
    assert_eq!(
        frames[2],
        steer(3, &pointer(&reminder, steer::inbox::Told::Silent))
    );
    assert_eq!(
        frames[3],
        steer(4, &pointer(&again, steer::inbox::Told::Replied))
    );
}

#[test]
fn a_compaction_spends_the_telling_again_because_the_context_lost_it() {
    let scratch = Scratch::new("compacted");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
    scratch.publish(NEWS);
    driver.deliver_inbox();
    driver.observe(&decoded(br#"{"type":"compaction"}"#));
    let frames = sink.answered(2);
    assert_eq!(
        frames,
        [
            steer(1, NEWS),
            steer(2, &steer::inbox::post_compaction(NEWS))
        ],
        "a context that dropped the telling gets the full telling plus recovery syntax",
    );
}

#[test]
fn an_ending_nobody_heard_reaches_the_lanes_next_occupant_once() {
    let scratch = Scratch::new("caught-up");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
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
    scratch.publish(NEWS);
    driver.deliver_inbox();
    let frames = sink.answered(2);
    let job = frames
        .iter()
        .find(|frame| frame["id"] == "h1")
        .and_then(|answer| answer["result"]["content"][0]["text"].as_str())
        .and_then(|text| text.split_once("Backgrounded as job "))
        .map(|(_, job)| {
            job.trim_end_matches("; result will be delivered automatically.")
                .to_owned()
        })
        .expect("the answer names the job");

    // The turn ends before the command does, so nobody hears its ending.
    driver.close();
    release(&fifo);
    settled(&scratch);
    assert_eq!(sink.answered(2).len(), 2);

    // The lane's next occupant hears it at its start, after the inbox.
    let later = Sink::default();
    let successor = make_driver(later.clone(), &scratch, Some(scratch.terminal()));
    successor.catch_up();
    let frames = later.answered(2);
    assert_eq!(frames[0], steer(1, NEWS));
    let delivery = frames[1]["message"]
        .as_str()
        .expect("the ending is delivered");
    assert!(
        delivery.starts_with(&format!(
            "<system-notice>\nBackground job {job} has completed. Resume your work using the \
             result below.\n(no output)\n\nWall time: "
        )),
        "{delivery}"
    );
    heard(&scratch, &job);

    // Once heard, it is owed to nobody.
    assert!(scratch.terminal().undelivered().is_empty());
}

/// A command held on the scratch's fifo, kept past a short foreground, and
/// the terminal that kept it. No follow thread runs: the test tells it.
fn kept_job(scratch: &Scratch, fifo: &str) -> (Terminal, Job) {
    let path = scratch.0.join(fifo);
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo")
            .success()
    );
    let terminal = scratch
        .terminal()
        .foreground(std::time::Duration::from_millis(50));
    let call = vendor::omp::ShellCall {
        command: format!("read held < {fifo}"),
        timeout: None,
        notices: Vec::new(),
    };
    let Run::Backgrounded(_, job) = terminal.run(&call, &Control::new()) else {
        panic!("a held command outruns the foreground");
    };
    release(&path);
    (terminal, job)
}

#[test]
fn an_ending_the_job_tool_reported_is_not_delivered_again() {
    let scratch = Scratch::new("read-ending");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
    let (terminal, job) = kept_job(&scratch, "hold");
    let wait = vendor::omp::JobCall {
        op: vendor::omp::JobOp::Wait,
        id: Some(job.id().to_owned()),
        timeout: ANSWER_BOUND,
    };
    let answer = terminal.job(&wait, &Control::new());
    assert!(answer.output.contains("(no output)"), "{}", answer.output);

    driver.deliver(&job, terminal.follow(&job, &Control::new()).as_ref());
    assert!(sink.frames().is_empty(), "the answer already told the lane");
    assert!(
        terminal.undelivered().is_empty(),
        "the next seat's catch-up owes it nothing"
    );
    settled(&scratch);
}

#[test]
fn an_unread_ending_is_delivered_once_and_a_lost_telling_is_owed_again() {
    let scratch = Scratch::new("unread-ending");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
    let (terminal, job) = kept_job(&scratch, "hold");
    let report = terminal.follow(&job, &Control::new());
    driver.deliver(&job, report.as_ref());
    driver.deliver(&job, report.as_ref());
    assert_eq!(
        sink.frames().len(),
        1,
        "of two tellers racing to one ending exactly one speaks"
    );
    assert!(terminal.undelivered().is_empty());

    let (terminal, lost) = kept_job(&scratch, "hold-lost");
    driver.close();
    driver.deliver(&lost, terminal.follow(&lost, &Control::new()).as_ref());
    assert_eq!(
        terminal
            .undelivered()
            .iter()
            .map(Job::id)
            .collect::<Vec<_>>(),
        [lost.id()],
        "a notice that never reached the lane is owed to the next seat"
    );
    settled(&scratch);
}

#[test]
fn steering_one_agent_backgrounds_only_that_agents_running_shell() {
    let recipient = Scratch::new("recipient-steered");
    let recipient_sink = Sink::default();
    let recipient_driver = make_driver(
        recipient_sink.clone(),
        &recipient,
        Some(recipient.terminal()),
    );
    let recipient_fifo = recipient.0.join("hold");

    let other = Scratch::new("other-foreground");
    let other_sink = Sink::default();
    let other_driver = make_driver(other_sink.clone(), &other, Some(other.terminal()));
    let other_fifo = other.0.join("hold");

    for fifo in [&recipient_fifo, &other_fifo] {
        assert!(
            std::process::Command::new("mkfifo")
                .arg(fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
    }
    recipient_driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"recipient","toolCallId":"call_recipient","toolName":"shell","arguments":{"command":"read held < hold"}}"#,
    ));
    other_driver.observe(&decoded(
        br#"{"type":"host_tool_call","id":"other","toolCallId":"call_other","toolName":"shell","arguments":{"command":"read held < hold"}}"#,
    ));

    recipient.publish(NEWS);
    recipient_driver.deliver_inbox();
    let recipient_frames = recipient_sink.answered(2);
    let answer = recipient_frames
        .iter()
        .find(|frame| frame["id"] == "recipient")
        .expect("the recipient call is answered");
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .expect("recipient answer text")
            .contains("Backgrounded early to handle an incoming message"),
        "{answer:?}"
    );

    assert!(
        other_sink.frames().is_empty(),
        "the unrelated agent received a shell answer"
    );
    assert_eq!(
        other_driver
            .calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len(),
        1,
        "the unrelated agent's shell call stopped being foreground work"
    );

    release(&other_fifo);
    let other_frames = other_sink.answered(1);
    assert_eq!(other_frames.len(), 1, "{other_frames:?}");
    assert_eq!(other_frames[0]["id"], "other");
    let other_text = other_frames[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("other answer text");
    assert!(
        !other_text.contains("Backgrounded as job"),
        "the unrelated shell was backgrounded: {other_text}"
    );
    assert_eq!(other_frames[0]["result"]["details"]["exitCode"], 0);

    release(&recipient_fifo);
    recipient_sink.answered(3);
    settled(&recipient);
    settled(&other);
}

#[test]
fn a_message_backgrounds_the_running_command_and_its_ending_is_delivered_as_steering() {
    let scratch = Scratch::new("steered");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, Some(scratch.terminal()));
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
    scratch.publish(NEWS);
    driver.deliver_inbox();

    // The steer and the interrupted call's answer are written by two
    // threads, so only their presence is fixed.
    let frames = sink.answered(2);
    assert!(frames.contains(&steer(1, NEWS)), "{frames:?}");
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
    let driver = make_driver(sink.clone(), &scratch, None);
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
fn task_generation_reminders_are_private_and_deduplicated_by_authority() {
    let scratch = Scratch::new("task-generation");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    let notice = TaskGenerationNotice {
        occupant: super::super::super::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        generation: 4,
        change: "task edited".to_owned(),
        editor: "operator".to_owned(),
        position: 44,
    };

    driver.deliver_task_generation(&notice);
    driver.deliver_task_generation(&notice);
    let frames = sink.answered(1);
    assert_eq!(frames.len(), 1, "one authority identity interjects once");
    let message = frames[0]["message"]
        .as_str()
        .expect("private reminder text");
    assert!(message.starts_with("<system-notice>\n"));
    assert!(message.contains("generation 4 is task edited by operator at position 44"));
    assert!(message.contains("spice task reclaim CLAIM-1kKRRsDP"));
    assert!(message.contains("no acknowledgment key or visible ACK/NAK is required"));

    driver.deliver_task_generation(&TaskGenerationNotice {
        generation: 5,
        ..notice.clone()
    });
    driver.deliver_task_generation(&notice);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "a later generation receives one new reminder and suppresses older replay",
    );
}

#[test]
fn a_kick_carries_inbox_consent_or_private_task_authority() {
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
    let task = Kick::TaskGeneration(TaskGenerationNotice {
        occupant: super::super::super::new_uuid(),
        task: "CLAIM-1kKRRsDP".to_owned(),
        claim: 33,
        generation: 4,
        change: "task edited".to_owned(),
        editor: "operator".to_owned(),
        position: 44,
    });
    let bytes = serde_json::to_vec(&task).expect("a private task kick serializes");
    assert!(bytes.len() <= KICK_BYTES, "{}", bytes.len());
    assert_eq!(
        serde_json::from_slice::<Kick>(&bytes).expect("a private task kick reads back"),
        task,
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
    let driver = make_driver(sink.clone(), &scratch, None);
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
fn a_settled_prompt_closes_the_stream_and_later_deliveries_are_dropped() {
    let scratch = Scratch::new("closed");
    let sink = Sink::default();
    let driver = make_driver(sink.clone(), &scratch, None);
    driver.observe(&decoded(
        br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"completed","sessionSettled":true}"#,
    ));

    scratch.publish(b"<key>\ndirective=d1\nstop\n</key>\n");
    driver.deliver_inbox();
    assert_eq!(driver.interjections(), 1, "the attempt is counted");
    assert!(sink.frames().is_empty(), "nothing reaches a closed stream");
    assert_eq!(
        driver.open(b"late\n").expect_err("closed").kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn an_unsettled_prompt_keeps_input_until_the_session_settles() {
    let scratch = Scratch::new("settling");
    let sink = Sink::default();
    let driver = make_driver(sink, &scratch, None);
    driver.observe(&decoded(br#"{"type":"agent_end","messages":[]}"#));
    driver.observe(&decoded(
        br#"{"type":"prompt_result","id":"prompt-7","agentInvoked":true,"status":"completed","sessionSettled":false}"#,
    ));
    driver
        .open(b"{\"type\":\"still_open\"}\n")
        .expect("unsettled background work keeps the command stream open");

    driver.observe(&decoded(
        br#"{"type":"session_settled","reason":"prompt_complete"}"#,
    ));
    assert_eq!(
        driver.open(b"late\n").expect_err("settled").kind(),
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
