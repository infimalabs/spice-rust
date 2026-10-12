use super::*;

mod retune;
mod task_notices;

use listen::{Dialect, Locus};

/// How long a test waits for the driver to answer before it fails.
const ANSWER_BOUND: std::time::Duration = std::time::Duration::from_secs(2);

/// The driver's command stream: one request per line, with a wake per line.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>, Arc<std::sync::Condvar>);

impl Sink {
    /// Frames the driver has written, once it has written `frames` of them.
    /// A background notice reads as an `aside` frame with its purpose and
    /// message.
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
            .map(|line| {
                let mut frame: serde_json::Value =
                    serde_json::from_slice(line).expect("one JSON request per line");
                if let Some(args) = frame["message"]
                    .as_str()
                    .and_then(|message| message.strip_prefix("/spice-aside "))
                {
                    let notice: serde_json::Value =
                        serde_json::from_str(args).expect("one SDK bridge notice");
                    frame["type"] = serde_json::json!("aside");
                    frame["purpose"] = notice["purpose"].clone();
                    frame["message"] = notice["message"].clone();
                }
                frame
            })
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

/// A driver with nothing to read and no terminal, writing to `sink`.
fn driver(sink: &Sink) -> Arc<Driver<Sink>> {
    Driver::with_readers(
        sink.clone(),
        || None,
        || None,
        |_| Err("no reads here".to_owned()),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    )
}

fn decoded(bytes: &[u8]) -> Decoded {
    listen::decode(Dialect::Omp, bytes, Locus::new(1, 0, bytes.len() as u64))
        .expect("matching locus")
}

#[test]
fn room_catch_up_retries_only_until_its_exact_delivery_lands() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let occupant = crate::resident::new_uuid();
    let other = crate::resident::new_uuid();
    let root = std::env::temp_dir().join(format!("spice-room-catch-up-{}", occupant));
    let path = root.join("notice.json");
    fs::create_dir_all(&root).expect("fixture directory");
    driver.configure_room_catch_up(path.clone(), occupant);

    driver.deliver_room_catch_up(false);
    assert!(
        sink.answered(0).is_empty(),
        "a missing delivery stays pending"
    );
    let write = |occupant, text: &str| {
        fs::write(
            &path,
            serde_json::to_vec(&crate::resident::room_catch_up::Notice {
                occupant,
                through: wire::Seq::new(7).expect("fixture position"),
                text: text.to_owned(),
            })
            .expect("notice serializes"),
        )
        .expect("notice writes");
    };
    write(other, "wrong occupant");
    driver.deliver_room_catch_up(false);
    assert!(
        sink.answered(0).is_empty(),
        "renewal never inherits another occupant's delivery"
    );

    write(occupant, "retry landed");
    driver.deliver_room_catch_up(false);
    driver.deliver_room_catch_up(false);
    assert_eq!(
        sink.answered(1).len(),
        1,
        "ordinary kicks coalesce an aside still queued in the provider"
    );
    driver.deliver_room_catch_up(true);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "compaction retries an aside with no transcript landing"
    );

    let observed = br#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"retry landed"}]}}
"#;
    assert!(driver.observe(&decoded(observed)).is_none());
    driver.deliver_room_catch_up(false);
    driver.deliver_room_catch_up(true);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "a landed aside remains coalesced until durable recognition"
    );

    fs::remove_file(&path).expect("durable cursor settles the notice");
    driver.deliver_room_catch_up(false);
    let frames = sink.answered(3);
    assert_eq!(frames[2]["type"], "aside");
    assert_eq!(frames[2]["purpose"], ROOM_CATCH_UP_PURPOSE);
    assert_eq!(frames[2]["message"], "");
    fs::remove_dir_all(root).expect("fixture cleanup");
}
