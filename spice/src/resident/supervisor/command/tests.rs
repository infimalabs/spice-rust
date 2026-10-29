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
    let landed_path = root.join("landed.json");
    fs::create_dir_all(&root).expect("fixture directory");
    driver.configure_room_catch_up(path.clone(), landed_path.clone(), occupant);

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
    fs::remove_file(&path).expect("changed attention removes the current plan");
    driver.deliver_room_catch_up(false);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "changed attention cannot discard a landed page before cursor progress"
    );
    write(occupant, "expanded retry landed");
    driver.deliver_room_catch_up(false);
    assert_eq!(
        sink.answered(2).len(),
        2,
        "recomputed content cannot replace a landed page before cursor progress"
    );

    fs::remove_file(&landed_path).expect("durable cursor releases landed evidence");
    fs::remove_file(&path).expect("durable cursor settles the notice");
    driver.deliver_room_catch_up(false);
    let frames = sink.answered(3);
    assert_eq!(frames[2]["type"], "aside");
    assert_eq!(frames[2]["purpose"], ROOM_CATCH_UP_PURPOSE);
    assert_eq!(frames[2]["message"], "");
    crate::resident::room_catch_up::publish_notice(
        &landed_path,
        &crate::resident::room_catch_up::Notice {
            occupant,
            through: wire::Seq::new(7).expect("fixture position"),
            text: "former occupant delivery".to_owned(),
        },
    )
    .expect("former occupant landed state writes");
    driver.configure_room_catch_up(path.clone(), landed_path, other);
    write(other, "successor delivery");
    driver.deliver_room_catch_up(false);
    assert_eq!(
        sink.answered(4)[3]["message"],
        "successor delivery",
        "a successor occupant never inherits former occupant landed state"
    );
    fs::remove_dir_all(root).expect("fixture cleanup");
}

#[test]
fn failed_landed_evidence_write_preserves_and_retries_the_exact_page() {
    let sink = Sink::default();
    let driver = driver(&sink);
    let occupant = crate::resident::new_uuid();
    let root = std::env::temp_dir().join(format!("spice-room-landing-retry-{occupant}"));
    let path = root.join("notice.json");
    let blocked_parent = root.join("blocked");
    let landed_path = blocked_parent.join("landed.json");
    fs::create_dir_all(&root).expect("fixture directory");
    fs::write(&blocked_parent, b"not a directory").expect("landing path is blocked");
    driver.configure_room_catch_up(path.clone(), landed_path.clone(), occupant);
    let landed = crate::resident::room_catch_up::Notice {
        occupant,
        through: wire::Seq::new(7).expect("fixture position"),
        text: "observed page".to_owned(),
    };
    crate::resident::room_catch_up::publish_notice(&path, &landed).expect("current notice writes");
    driver.deliver_room_catch_up(false);
    let observed = br#"{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"observed page"}]}}
"#;
    assert!(driver.observe(&decoded(observed)).is_none());

    let replacement = crate::resident::room_catch_up::Notice {
        occupant,
        through: wire::Seq::new(8).expect("fixture position"),
        text: "recomputed page".to_owned(),
    };
    crate::resident::room_catch_up::publish_notice(&path, &replacement)
        .expect("recomputed notice writes");
    driver.deliver_room_catch_up(true);
    assert_eq!(
        sink.answered(1).len(),
        1,
        "failed landing persistence suppresses replacement and resend"
    );

    fs::remove_file(&blocked_parent).expect("landing path unblocks");
    fs::create_dir(&blocked_parent).expect("landing directory exists");
    driver.deliver_room_catch_up(false);
    assert_eq!(
        crate::resident::room_catch_up::read_notice(&landed_path),
        Some(landed),
        "the next resident event persists the exact observed page"
    );
    assert_eq!(sink.answered(1).len(), 1);

    fs::remove_file(&landed_path).expect("cursor releases landed evidence");
    driver.deliver_room_catch_up(false);
    assert_eq!(sink.answered(2)[1]["message"], replacement.text);
    fs::remove_dir_all(root).expect("fixture cleanup");
}

/// A short directory for test doors: Darwin bounds socket paths to 104 bytes.
fn door_directory() -> PathBuf {
    if cfg!(unix) {
        PathBuf::from("/tmp")
    } else {
        std::env::temp_dir()
    }
}

fn confirmed_background_round_trip<W: Write + Send + 'static>(
    driver: Arc<Driver<W>>,
    occupant: CanonicalUuid,
    notice: crate::resident::design_input::BackgroundNotice,
) -> io::Result<bool> {
    let identity = crate::resident::new_uuid();
    let path = door_directory().join(format!("spice-background-test-{identity}.sock"));
    let door = door::Door::bind(&path, KICK_BYTES)?;
    let responder = std::thread::spawn(move || {
        let kick = door.recv(None).expect("background kick arrives");
        let Kick::BackgroundConfirmed {
            notice,
            request,
            owner,
            reply,
        } = serde_json::from_slice(&kick).expect("background kick decodes")
        else {
            panic!("confirmed delivery sends a confirmed background kick");
        };
        assert_eq!(owner, None, "ordinary notices carry no retained owner");
        let accepted = background_accepted(&driver, occupant, &notice);
        confirmed_background::answer(&reply, request, accepted);
    });
    let result = confirmed_background::send_to_door(&path, &door_directory(), notice);
    responder.join().expect("background responder joins");
    result
}

#[derive(Clone, Copy)]
struct RefusingSink;

impl Write for RefusingSink {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "fixture refuses the aside",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn confirmed_background_delivery_requires_the_exact_occupant_and_a_successful_write() {
    let occupant = crate::resident::new_uuid();
    let notice = crate::resident::design_input::BackgroundNotice {
        occupant,
        purpose: "design-return:DESIGN-1".to_owned(),
        text: "returned input".to_owned(),
    };
    let sink = Sink::default();
    assert!(
        confirmed_background_round_trip(driver(&sink), occupant, notice.clone())
            .expect("matching delivery answers"),
        "the exact occupant confirms a successful enqueue"
    );
    assert_eq!(sink.answered(1)[0]["message"], notice.text);

    assert!(
        !confirmed_background_round_trip(
            driver(&Sink::default()),
            crate::resident::new_uuid(),
            notice.clone(),
        )
        .expect("mismatched delivery answers"),
        "another occupant refuses the notice"
    );

    let refusing = Driver::with_readers(
        RefusingSink,
        || None,
        || None,
        |_| Err("no reads here".to_owned()),
        steer::Key::for_identity(crate::resident::new_uuid()),
        None,
    );
    assert!(
        !confirmed_background_round_trip(refusing, occupant, notice.clone())
            .expect("failed write answers"),
        "a failed driver write refuses consumption"
    );

    let missing = door_directory().join(format!(
        "spice-background-missing-{}.sock",
        crate::resident::new_uuid()
    ));
    assert!(
        confirmed_background::send_to_door(&missing, &door_directory(), notice).is_err(),
        "a missing supervisor door is a transport failure"
    );
}
