use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::Driver;

const TARGET: &str = "spice://quote/v1/steer/1kQnrzNZ/00";

fn driver(
    read_quote: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
) -> Arc<Driver<Vec<u8>>> {
    Driver::with_readers(
        Vec::new(),
        || None,
        || None,
        read_quote,
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
fn a_quote_read_answers_with_the_resolution_or_its_refusal() {
    let driver = driver(|target| {
        if target == TARGET {
            Ok("Quoted from operator:\n\nship it".to_owned())
        } else {
            Err("[quote unavailable: reason=digest-mismatch]".to_owned())
        }
    });
    driver.serve_uri("u1", "read", TARGET);
    driver.serve_uri("u2", "read", "spice://quote/v1/steer/1kQnrzNZ/11");
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
    assert!(driver.uris.lock().expect("pending locks").is_empty());
}

#[test]
fn anything_but_a_quote_read_is_refused_without_asking_the_resident() {
    let driver = driver(|_| panic!("only a quote read reaches the resident"));
    driver.serve_uri("w", "write", TARGET);
    driver.serve_uri("t", "read", "spice://task/VENDOR-dVlFd9fV8");
    for frame in frames(&driver) {
        assert_eq!(frame["isError"], true);
        assert_eq!(frame["error"], vendor::omp::URI_SERVED);
    }
    assert_eq!(frames(&driver).len(), 2);
    assert!(driver.uris.lock().expect("pending locks").is_empty());
}

#[test]
fn a_withdrawn_quote_read_is_never_answered_and_leaves_nothing_pending() {
    let (entered, reading) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let (entered, released) = (Mutex::new(entered), Mutex::new(released));
    let driver = driver(move |_| {
        entered.lock().expect("entry").send(()).expect("test waits");
        released
            .lock()
            .expect("release")
            .recv()
            .expect("test releases");
        Ok("late".to_owned())
    });
    driver.serve_uri("u1", "read", TARGET);
    reading.recv().expect("the read began");
    driver.cancel("u1");
    release.send(()).expect("the read waits");
    settled(&driver);
    assert!(
        frames(&driver).is_empty(),
        "a withdrawn read gets no answer"
    );
    assert!(driver.uris.lock().expect("pending locks").is_empty());
}
