use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use listen::{Dialect, Locus};
use vendor::omp::{JobCall, JobOp};

use super::super::terminal::job::Ending;
use super::super::terminal::{Control, Event as JobEvent, Job, Jobs, Terminal};
use super::{Driver, Work};

/// The occupant's input, as the driver writes it.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink {
    /// Every background notice written, as its purpose and message.
    fn notices(&self) -> Vec<(String, String)> {
        let bytes = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let frame: serde_json::Value = serde_json::from_slice(line).expect("a frame");
                let notice = frame["message"].as_str()?.strip_prefix("/spice-aside ")?;
                let notice: serde_json::Value = serde_json::from_str(notice).expect("a notice");
                Some((
                    notice["purpose"].as_str()?.to_owned(),
                    notice["message"].as_str()?.to_owned(),
                ))
            })
            .collect()
    }

    /// Every frame written, in order.
    fn frames(&self) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).expect("a frame"))
            .collect()
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "spice-job-event-delivery-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ));
        std::fs::create_dir_all(&path).expect("scratch");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn terminal(scratch: &Scratch) -> (Jobs, Terminal) {
    let jobs = Jobs::beneath(&scratch.0, crate::resident::new_uuid());
    let terminal = Terminal::new(
        "/bin/sh",
        &scratch.0,
        sill::Environment::empty(),
        jobs.clone(),
    );
    (jobs, terminal)
}

fn seat(sink: &Sink, terminal: &Terminal) -> Arc<Driver<Sink>> {
    Driver::with_readers(
        sink.clone(),
        || None,
        || None,
        |_| Err("no quotes here".to_owned()),
        steer::Key::for_identity(crate::resident::new_uuid()),
        Some(terminal.clone()),
    )
}

fn lines(texts: &[&str]) -> JobEvent {
    JobEvent::Lines {
        lines: texts.iter().map(|text| (*text).to_owned()).collect(),
    }
}

/// The notice each event is delivered as, by its own purpose.
fn told(job: &Job, number: u64, event: &JobEvent) -> (String, String) {
    let message = match event {
        JobEvent::Lines { lines } => vendor::omp::job_event(job.id(), lines),
        JobEvent::Flooded => vendor::omp::job_flooded(job.id()),
    };
    (format!("job:{}:event:{number}", job.id()), message)
}

fn ending_purpose(job: &Job) -> String {
    format!("job:{}", job.id())
}

#[test]
fn a_successor_hears_what_its_vacated_predecessor_did_not_then_the_ending() {
    let scratch = Scratch::new("catch-up");
    let (jobs, terminal) = terminal(&scratch);
    let job = jobs.record_for_tests("tail -f build.log", true, Some(60_000));
    let first = lines(&["compiling"]);
    let second = lines(&["warning: unused", "finished"]);

    let predecessor = Sink::default();
    let seated = seat(&predecessor, &terminal);
    job.print_for_tests(1, &first);
    seated.deliver_events(&job);
    seated.close();
    job.print_for_tests(2, &second);
    job.print_for_tests(3, &JobEvent::Flooded);
    seated.deliver_events(&job);
    job.end_for_tests(Ending::Exited { code: 0 });
    assert_eq!(
        predecessor.notices(),
        vec![told(&job, 1, &first)],
        "a closed seat hears nothing more"
    );

    let successor = Sink::default();
    let next = seat(&successor, &terminal);
    for owed in terminal.undelivered() {
        next.hear(&terminal, &owed, &Control::new());
    }
    let notices = successor.notices();
    assert_eq!(
        notices[..2],
        [told(&job, 2, &second), told(&job, 3, &JobEvent::Flooded)],
        "the unheard events, in order, and none already heard"
    );
    assert_eq!(notices.len(), 3);
    assert_eq!(
        notices[2].0,
        ending_purpose(&job),
        "the ending follows its events"
    );

    next.hear(&terminal, &job, &Control::new());
    assert!(terminal.undelivered().is_empty());
    assert_eq!(successor.notices().len(), 3, "each is told once");
}

#[test]
fn a_seated_lane_hears_each_event_once_in_order_while_the_job_or_service_runs() {
    for timeout_millis in [Some(60_000), None] {
        let scratch = Scratch::new("seated");
        let (jobs, terminal) = terminal(&scratch);
        let job = jobs.record_for_tests("watch the build", true, timeout_millis);
        let printed = [lines(&["one"]), lines(&["two", "three"]), lines(&["four"])];

        let sink = Sink::default();
        let seated = seat(&sink, &terminal);
        let follower = {
            let (seated, terminal, job) = (Arc::clone(&seated), terminal.clone(), job.clone());
            std::thread::spawn(move || seated.hear(&terminal, &job, &Control::new()))
        };
        for (number, event) in (1..).zip(&printed) {
            job.print_for_tests(number, event);
        }
        job.end_for_tests(Ending::Exited { code: 0 });
        follower.join().expect("follower ends with the job");

        let notices = sink.notices();
        let expected: Vec<_> = (1..)
            .zip(&printed)
            .map(|(number, event)| told(&job, number, event))
            .collect();
        assert_eq!(notices[..printed.len()], expected[..], "{timeout_millis:?}");
        assert_eq!(notices.len(), printed.len() + 1);
        assert_eq!(notices[printed.len()].0, ending_purpose(&job));
    }
}

#[test]
fn a_blocked_wait_is_restated_after_another_call_settles_until_its_own_answer() {
    let scratch = Scratch::new("restated-wait");
    let (jobs, terminal) = terminal(&scratch);
    let job = jobs.record_for_tests("serve", false, Some(600_000));
    let sink = Sink::default();
    let seated = seat(&sink, &terminal);
    let settled = |id: &str| {
        let record = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "toolResult",
                "toolCallId": id,
                "toolName": "edit",
                "content": [{"type": "text", "text": "patched"}],
                "isError": false,
            },
        })
        .to_string();
        listen::decode(
            Dialect::Omp,
            record.as_bytes(),
            Locus::new(1, 0, record.len() as u64),
        )
        .expect("matching locus")
    };
    let waiter = {
        let (seated, terminal) = (Arc::clone(&seated), terminal.clone());
        let call = Work::Job(JobCall {
            op: JobOp::Wait,
            id: Some(job.id().to_owned()),
            timeout: Duration::from_secs(30),
            until: None,
        });
        std::thread::spawn(move || seated.resolve(&terminal, "wait-1", &call, &Control::new()))
    };
    let armed = std::time::Instant::now();
    while sink.frames().is_empty() {
        assert!(
            armed.elapsed() < Duration::from_secs(5),
            "the wait reports when it arms"
        );
        std::thread::yield_now();
    }

    seated.observe(&settled("edit-1"));
    job.end_for_tests(Ending::Exited { code: 0 });
    waiter.join().expect("the wait answers when its job ends");
    seated.observe(&settled("edit-2"));

    let frames = sink.frames();
    let calls: Vec<_> = frames
        .iter()
        .map(|frame| (frame["type"].as_str(), frame["id"].as_str()))
        .collect();
    assert_eq!(
        calls,
        [
            (Some("host_tool_update"), Some("wait-1")),
            (Some("host_tool_update"), Some("wait-1")),
            (Some("host_tool_result"), Some("wait-1")),
        ]
    );
    assert_eq!(frames[1]["partialResult"], frames[0]["partialResult"]);
}
