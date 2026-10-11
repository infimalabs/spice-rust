use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::Driver;
use crate::resource::Access;

const TARGET: &str = "spice://quote/v1/steer/1kQnrzNZ/00";

fn driver(
    serve: impl Fn(&str, Access<'_>) -> Result<String, String> + Send + Sync + 'static,
) -> Arc<Driver<Vec<u8>>> {
    Driver::with_readers(
        Vec::new(),
        || None,
        || None,
        Box::new(serve),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    )
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

/// Wait until every answering thread has let go of the driver.
fn settled(driver: &Arc<Driver<Vec<u8>>>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(driver) > 1 {
        assert!(
            Instant::now() < deadline,
            "the answering thread never ended"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_read_answers_with_the_resource_or_its_refusal() {
    let driver = driver(|target, _| {
        if target == TARGET {
            Ok("Quoted from operator:\n\nship it".to_owned())
        } else {
            Err("[quote unavailable: reason=digest-mismatch]".to_owned())
        }
    });
    driver.serve_uri("u1", "read", TARGET, None);
    driver.serve_uri("u2", "read", "spice://quote/v1/steer/1kQnrzNZ/11", None);
    settled(&driver);
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
    let driver = driver({
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
    settled(&driver);
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
    let driver = driver(move |_, _| {
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
    settled(&driver);
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
