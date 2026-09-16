//! Driving a vendor over its stdin: the opening request, interjected
//! steering, and the close that ends its one turn.
//!
//! A vendor launched over a command stream reads nothing from its arguments
//! after start. The supervisor owns the pipe: it writes the opening request
//! the vendor contract composed, interjects this lane's rendered steering
//! inbox whenever the resident republishes it, and drops the pipe once the
//! agent's turn completes so the child drains and exits on its own. The
//! resident reaches the driver through one datagram kick per publication;
//! a lane with no command-driven occupant has nobody listening, and the kick
//! is lost by design.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use listen::{Decoded, Event, TurnMoment};
use wire::CanonicalUuid;

use super::super::RuntimePaths;

/// One child's command stream and what has already been said on it.
pub(in crate::resident) struct Driver<W: Write + Send = std::process::ChildStdin> {
    stdin: Mutex<Option<W>>,
    inbox: PathBuf,
    sequence: AtomicU64,
    delivered: Mutex<Option<Vec<u8>>>,
}

impl<W: Write + Send> Driver<W> {
    /// Own a child's stdin and the inbox path this lane's steering renders to.
    pub(in crate::resident) fn new(stdin: W, inbox: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            inbox: inbox.into(),
            sequence: AtomicU64::new(0),
            delivered: Mutex::new(None),
        })
    }

    /// Write the opening request the vendor contract composed.
    ///
    /// # Errors
    ///
    /// Returns the pipe's error, or `BrokenPipe` once the stream is closed.
    pub(in crate::resident) fn open(&self, opening: &[u8]) -> io::Result<()> {
        self.write(opening)
    }

    /// Interject the rendered inbox unless it is empty, missing, or exactly
    /// what the agent already received.
    pub(in crate::resident) fn deliver_inbox(&self) {
        let rendered = match fs::read(&self.inbox) {
            Ok(rendered) => rendered,
            Err(_) => return,
        };
        if rendered.is_empty() {
            return;
        }
        let mut delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if delivered.as_deref() == Some(rendered.as_slice()) {
            return;
        }
        let Ok(message) = std::str::from_utf8(&rendered) else {
            return;
        };
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        if self
            .write(&vendor::omp::steer_request(sequence, message))
            .is_ok()
        {
            *delivered = Some(rendered);
        }
    }

    /// Let one decoded record shape delivery.
    ///
    /// A compaction discards what the agent was told, so the standing inbox
    /// is interjected again; the end of the agent's turn ends its input, so
    /// the child drains and exits rather than idling for a prompt nobody
    /// sends.
    pub(in crate::resident) fn observe(&self, decoded: &Decoded) {
        let compacted = decoded
            .events()
            .iter()
            .any(|event| matches!(event, Event::Compaction(_)));
        if compacted {
            self.delivered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            self.deliver_inbox();
        }
        let completed = decoded
            .context()
            .and_then(|context| context.turn)
            .is_some_and(|turn| turn == TurnMoment::Completed);
        if completed {
            self.close();
        }
    }

    /// End the child's input; further deliveries are dropped.
    pub(in crate::resident) fn close(&self) {
        self.stdin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    /// Interjections the driver has written so far.
    #[cfg(test)]
    fn interjections(&self) -> u64 {
        self.sequence.load(Ordering::Relaxed)
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(stream) = stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the child's command stream is closed",
            ));
        };
        stream.write_all(bytes)?;
        stream.flush()
    }
}

/// Where this lane's driver listens for republished-inbox kicks.
///
/// `sockaddr_un.sun_path` is only 104 bytes on Darwin, so the door is one
/// content-addressed leaf beneath the short system temporary path, exactly
/// as the capture socket is.
pub(in crate::resident) fn kick_path(paths: &RuntimePaths, lane: CanonicalUuid) -> PathBuf {
    let mut identity = blake3::Hasher::new();
    identity.update(paths.common.as_os_str().as_encoded_bytes());
    identity.update(lane.to_string().as_bytes());
    kick_path_beneath(Path::new("/tmp"), &identity.finalize().to_string())
}

fn kick_path_beneath(root: &Path, identity: &str) -> PathBuf {
    root.join(format!("spice-k-{identity}.sock"))
}

/// Tell whichever driver listens for this lane that its inbox changed.
///
/// Nothing listens for a lane without a command-driven occupant, and a
/// driver that misses a kick reads the current inbox on the next one, so
/// the send is fire-and-forget.
pub(in crate::resident) fn kick(paths: &RuntimePaths, lane: CanonicalUuid) {
    #[cfg(unix)]
    {
        let path = kick_path(paths, lane);
        if let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() {
            let _sent = socket.send_to(b"inbox", path);
        }
    }
    #[cfg(not(unix))]
    let _ = (paths, lane);
}

/// Bind this lane's kick door and interject the inbox on every kick.
///
/// A stale door from an earlier supervisor is replaced. The listening thread
/// lives as long as the supervisor process; the door is removed once the
/// child has exited.
///
/// # Errors
///
/// Returns the bind error when the door cannot be opened.
pub(in crate::resident) fn listen_for_kicks(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    driver: Arc<Driver>,
) -> io::Result<KickDoor> {
    let path = kick_path(paths, lane);
    #[cfg(unix)]
    {
        let _ = fs::remove_file(&path);
        let socket = std::os::unix::net::UnixDatagram::bind(&path)?;
        std::thread::Builder::new()
            .name("spice-kick".to_owned())
            .spawn(move || {
                let mut kick = [0u8; 64];
                while socket.recv(&mut kick).is_ok() {
                    driver.deliver_inbox();
                }
            })?;
    }
    #[cfg(not(unix))]
    let _ = driver;
    Ok(KickDoor { path })
}

/// The bound kick door, removed when the supervisor is done with it.
pub(in crate::resident) struct KickDoor {
    path: PathBuf,
}

impl Drop for KickDoor {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sockaddr_un.sun_path` on Darwin, the tightest platform bound.
    const SUN_PATH_BYTES: usize = 104;

    use listen::{Dialect, Locus};

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Sink {
        fn frames(&self) -> Vec<serde_json::Value> {
            let written = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            written
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).expect("one JSON request per line"))
                .collect()
        }
    }

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
        let driver = Driver::new(sink.clone(), &inbox);
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
    fn the_end_of_the_turn_closes_the_stream_and_later_deliveries_are_dropped() {
        let scratch = Scratch::new("closed");
        let inbox = scratch.inbox();
        let sink = Sink::default();
        let driver = Driver::new(sink.clone(), &inbox);
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
            name.to_string_lossy().starts_with("spice-k-")
                && name.to_string_lossy().ends_with(".sock")
        }));
    }
}
