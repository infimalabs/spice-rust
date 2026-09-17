//! Driving a vendor over its stdin: the opening request, interjected
//! steering, and the close that ends its one turn.
//!
//! A vendor launched over a command stream reads nothing from its arguments
//! after start. The supervisor owns the pipe: it writes the opening request
//! the vendor contract composed, interjects this lane's rendered steering
//! inbox whenever the resident republishes it, answers the dialogs the agent
//! opens, and drops the pipe once the agent's turn completes so the child
//! drains and exits on its own. A drain interrupts the turn over the same
//! pipe before any signal is sent. A confirm dialog becomes one consent request
//! the resident decides; every other dialog is declined, since nobody at the
//! lane can compose a choice or text for it. The resident reaches the driver
//! through one datagram kick per inbox publication or consent answer; a lane
//! with no command-driven occupant has nobody listening, and the kick is lost
//! by design.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use listen::{Decoded, Dialog, Event, TurnMoment};
use serde::{Deserialize, Serialize};
use wire::CanonicalUuid;

use super::super::RuntimePaths;

/// Bytes one kick datagram may carry: a consent answer names one request.
const KICK_BYTES: usize = 256;

/// One child's command stream and what has already been said on it.
pub(in crate::resident) struct Driver<W: Write + Send = std::process::ChildStdin> {
    stdin: Mutex<Option<W>>,
    inbox: PathBuf,
    sequence: AtomicU64,
    delivered: Mutex<Option<Vec<u8>>>,
    /// The confirm dialog each open consent request will answer.
    asks: Mutex<HashMap<CanonicalUuid, String>>,
}

/// A confirm dialog stated as the consent request the resident decides.
pub(in crate::resident) struct Ask {
    pub(in crate::resident) request: CanonicalUuid,
    pub(in crate::resident) operation: String,
}

/// What one datagram at the lane's kick door says.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::resident) enum Kick {
    /// The lane's steering inbox was republished.
    Inbox,
    /// Consent ruled on a request the occupant raised.
    Answer {
        request: CanonicalUuid,
        confirmed: bool,
    },
    /// The lane is being drained: interrupt the turn and end the input.
    Abort,
}

impl<W: Write + Send> Driver<W> {
    /// Own a child's stdin and the inbox path this lane's steering renders to.
    pub(in crate::resident) fn new(stdin: W, inbox: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            inbox: inbox.into(),
            sequence: AtomicU64::new(0),
            delivered: Mutex::new(None),
            asks: Mutex::new(HashMap::new()),
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

    /// Let one decoded record shape delivery, and state the consent request a
    /// confirm dialog raises.
    ///
    /// A confirm holds the turn until consent answers it through
    /// [`Self::answer`]; any other dialog is declined at once, since no
    /// operator sees a lane's prompts and the turn would otherwise hold. A
    /// compaction discards what the agent was told, so the standing inbox is
    /// interjected again; the end of the agent's turn ends its input, so the
    /// child drains and exits rather than idling for a prompt nobody sends.
    pub(in crate::resident) fn observe(&self, decoded: &Decoded) -> Option<Ask> {
        let asked = match decoded
            .context()
            .and_then(|context| context.dialog.as_ref())
        {
            Some(Dialog::Confirm { id, title, message }) => Some(self.ask(id, title, message)),
            Some(dialog) => {
                let _declined = self.write(&vendor::omp::dialog_declined(dialog.id()));
                None
            }
            None => None,
        };
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
        asked
    }

    /// Answer the confirm dialog one consent request stands for.
    ///
    /// A request this driver never raised, or one it already answered, is
    /// nobody's dialog here and says nothing.
    pub(in crate::resident) fn answer(&self, request: CanonicalUuid, confirmed: bool) {
        let dialog = self
            .asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request);
        if let Some(dialog) = dialog {
            let _answered = self.write(&vendor::omp::dialog_confirmed(&dialog, confirmed));
        }
    }

    fn ask(&self, dialog: &str, title: &str, message: &str) -> Ask {
        let request = super::super::new_uuid();
        self.asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request, dialog.to_owned());
        Ask {
            request,
            operation: match (title.trim(), message.trim()) {
                (title, "") => title.to_owned(),
                ("", message) => message.to_owned(),
                (title, message) => format!("{title}: {message}"),
            },
        }
    }

    /// Interrupt the agent's turn and end its input: how a drain begins.
    ///
    /// omp stops the turn where it stands and records the assistant message
    /// as aborted, then reads the end of its input and exits on its own. The
    /// resident's signal is only for an occupant that does neither.
    pub(in crate::resident) fn abort(&self) {
        let _aborted = self.write(&vendor::omp::abort_request());
        self.close();
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
    send(paths, lane, &Kick::Inbox);
}

/// Tell whichever driver listens for this lane how consent ruled on one of
/// its requests.
///
/// An occupant that has since left took its dialog with it, so the answer is
/// as fire-and-forget as an inbox kick.
pub(in crate::resident) fn answer(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    request: CanonicalUuid,
    confirmed: bool,
) {
    send(paths, lane, &Kick::Answer { request, confirmed });
}

/// Interrupt the driver listening at `door`, if one does.
///
/// A lane without a command-driven occupant has no door, and a door whose
/// supervisor has gone refuses the datagram; either way the error says
/// nobody is there to interrupt, so a signal is the only word left.
///
/// # Errors
///
/// Returns the send error when no driver takes the kick.
pub(in crate::resident) fn abort(door: &Path) -> io::Result<()> {
    send_to(door, &Kick::Abort)
}

fn send(paths: &RuntimePaths, lane: CanonicalUuid, kick: &Kick) {
    let _sent = send_to(&kick_path(paths, lane), kick);
}

fn send_to(door: &Path, kick: &Kick) -> io::Result<()> {
    #[cfg(unix)]
    {
        let bytes = serde_json::to_vec(kick).expect("a kick is representable as JSON");
        std::os::unix::net::UnixDatagram::unbound()?.send_to(&bytes, door)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (door, kick);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "kick doors require Unix",
        ))
    }
}

/// Bind this lane's kick door and let every kick reach the driver.
///
/// A stale door from an earlier supervisor is replaced. The listening thread
/// lives as long as the supervisor process; the door is removed once the
/// child has exited. A datagram that says nothing this door understands is
/// dropped, since the sender never waits for it.
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
                let mut kick = [0u8; KICK_BYTES];
                while let Ok(length) = socket.recv(&mut kick) {
                    match serde_json::from_slice(&kick[..length]) {
                        Ok(Kick::Inbox) => driver.deliver_inbox(),
                        Ok(Kick::Answer { request, confirmed }) => {
                            driver.answer(request, confirmed);
                        }
                        Ok(Kick::Abort) => driver.abort(),
                        Err(_) => {}
                    }
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
    fn a_prompt_nobody_can_compose_is_declined_while_notices_pass() {
        let scratch = Scratch::new("dialog");
        let sink = Sink::default();
        let driver = Driver::new(sink.clone(), scratch.inbox());
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
    fn a_confirm_is_stated_as_one_request_and_answered_once_consent_rules() {
        let scratch = Scratch::new("confirm");
        let sink = Sink::default();
        let driver = Driver::new(sink.clone(), scratch.inbox());
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
        let driver = Driver::new(sink.clone(), scratch.inbox());
        driver.abort();
        assert_eq!(
            sink.frames(),
            [serde_json::json!({"id": "spice-abort", "type": "abort"})]
        );
        assert_eq!(
            driver.open(b"late\n").expect_err("closed").kind(),
            io::ErrorKind::BrokenPipe
        );

        let door_path =
            kick_path_beneath(Path::new("/tmp"), &format!("abort-{}", std::process::id()));
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
