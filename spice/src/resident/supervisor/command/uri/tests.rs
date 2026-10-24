use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::Driver;
use crate::resource::Access;

const TARGET: &str = "spice://quote/v1/steer/1kQnrzNZ/00";

thread_local! {
    static ANSWERING: RefCell<Option<Ended>> = const { RefCell::new(None) };
}

/// Announces that an answering thread exited, after it answered or stayed
/// silent and let go of the driver.
struct Ended(mpsc::Sender<()>);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

/// A driver whose answering threads each announce their exit on the returned
/// receiver.
fn driver(
    serve: impl Fn(&str, Access<'_>) -> Result<String, String> + Send + Sync + 'static,
) -> (Arc<Driver<Vec<u8>>>, mpsc::Receiver<()>) {
    let (ended, exits) = mpsc::channel();
    let driver = Driver::with_server(
        Vec::new(),
        crate::resident::supervisor::command::Readers::default(),
        Box::new(move |url: &str, access: Access<'_>| {
            ANSWERING.with(|answering| *answering.borrow_mut() = Some(Ended(ended.clone())));
            serve(url, access)
        }),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    );
    (driver, exits)
}

fn frames(driver: &Driver<Vec<u8>>) -> Vec<serde_json::Value> {
    let input = driver.stdin.lock().expect("input locks");
    input
        .as_ref()
        .expect("input remains open")
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("a frame"))
        .collect()
}

/// Receive the exit of each of `answering` threads.
fn settled(exits: &mpsc::Receiver<()>, answering: usize) {
    for _ in 0..answering {
        exits
            .recv_timeout(Duration::from_secs(10))
            .expect("an answering thread exits");
    }
}

#[test]
fn a_read_answers_with_the_resource_or_its_refusal() {
    let (driver, exits) = driver(|target, _| {
        if target == TARGET {
            Ok("Quoted from operator:\n\nship it".to_owned())
        } else {
            Err("[quote unavailable: reason=digest-mismatch]".to_owned())
        }
    });
    driver.serve_uri("u1", "read", TARGET, None);
    driver.serve_uri("u2", "read", "spice://quote/v1/steer/1kQnrzNZ/11", None);
    settled(&exits, 2);
    let mut answers = frames(&driver);
    answers.sort_by_key(|frame| frame["id"].as_str().map(str::to_owned));
    assert_eq!(
        answers,
        [
            serde_json::json!({
                "type": "host_uri_result",
                "id": "u1",
                "content": "Quoted from operator:\n\nship it",
                "contentType": "text/markdown",
            }),
            serde_json::json!({
                "type": "host_uri_result",
                "id": "u2",
                "isError": true,
                "error": "[quote unavailable: reason=digest-mismatch]",
            }),
        ]
    );
    assert!(
        driver
            .uris
            .pending
            .lock()
            .expect("pending locks")
            .is_empty()
    );
}

#[test]
fn a_write_reaches_the_server_with_its_content_and_other_operations_are_refused() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let (driver, exits) = driver({
        let written = Arc::clone(&written);
        move |url, access| {
            let Access::Write(content) = access else {
                panic!("a write never reads");
            };
            written
                .lock()
                .expect("writes lock")
                .push((url.to_owned(), content.to_owned()));
            Ok("edited".to_owned())
        }
    });
    driver.serve_uri("w", "write", "spice://task/X/description", Some("new"));
    driver.serve_uri("x", "write", TARGET, None);
    driver.serve_uri("y", "list", TARGET, None);
    // Only the write reaches a thread; malformed operations refuse inline.
    settled(&exits, 1);
    let mut answers = frames(&driver);
    answers.sort_by_key(|frame| frame["id"].as_str().map(str::to_owned));
    let refused = |id: &str| {
        serde_json::json!({
            "type": "host_uri_result",
            "id": id,
            "isError": true,
            "error": vendor::omp::URI_SERVED,
        })
    };
    assert_eq!(
        answers,
        [
            serde_json::json!({
                "type": "host_uri_result",
                "id": "w",
                "content": "edited",
                "contentType": "text/markdown",
            }),
            refused("x"),
            refused("y"),
        ]
    );
    assert_eq!(
        *written.lock().expect("writes lock"),
        [("spice://task/X/description".to_owned(), "new".to_owned())]
    );
    assert!(
        driver
            .uris
            .pending
            .lock()
            .expect("pending locks")
            .is_empty()
    );
}

#[test]
fn a_withdrawn_quote_read_is_never_answered_and_leaves_nothing_pending() {
    let (entered, reading) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let (entered, released) = (Mutex::new(entered), Mutex::new(released));
    let (driver, exits) = driver(move |_, _| {
        entered.lock().expect("entry").send(()).expect("test waits");
        released
            .lock()
            .expect("release")
            .recv()
            .expect("test releases");
        Ok("late".to_owned())
    });
    driver.serve_uri("u1", "read", TARGET, None);
    reading.recv().expect("the read began");
    driver.cancel("u1");
    release.send(()).expect("the read waits");
    settled(&exits, 1);
    assert!(
        frames(&driver).is_empty(),
        "a withdrawn read gets no answer"
    );
    assert!(
        driver
            .uris
            .pending
            .lock()
            .expect("pending locks")
            .is_empty()
    );
}
