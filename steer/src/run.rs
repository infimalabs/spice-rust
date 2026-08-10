//! The wrapper that lets a lane hear before its command runs.
//!
//! This module names nothing but the standard library, and the bound is
//! measured rather than asserted: `steer/tests/bound.rs` reads this file and
//! refuses any import that leaves std. The reason is cost, not purity. This
//! code sits in front of every command an agent runs, so whatever it touches is
//! paid for on every invocation. It reads one rendered file, writes those bytes
//! to the stream, and hands the command through with one spawn — no repository
//! discovery, no database, no second process.
//!
//! It also refuses nothing. A wrapper that declined to run commands would break
//! the agent it wraps, so a missing rendering or an unreadable one is ordinary
//! and the command still runs. Regression is the enforcement here.

use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::{self, Write};
#[cfg(unix)]
use std::net::Shutdown;
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Command, ExitStatus};
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
#[cfg(unix)]
use std::thread::{self, JoinHandle};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const RELAY_SETTLE: Duration = Duration::from_millis(50);

/// Writes a lane's rendered direction to the stream, reporting whether anything
/// was waiting.
///
/// An absent rendering writes nothing and reports `false`. Absence is not a
/// failure: the file is a rendering of the journal, disposition lives in the
/// journal itself, and a lane with nothing pending has nothing to render.
///
/// # Errors
///
/// Returns the error when a rendering that exists cannot be read, or when the
/// stream cannot take it.
pub fn emit(rendering: &Path, stream: &mut impl Write) -> io::Result<bool> {
    let bytes = match fs::read(rendering) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if bytes.is_empty() {
        return Ok(false);
    }
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(true)
}

/// Runs the wrapped command and waits for that immediate child.
///
/// The command's output is relayed through private sockets, so descendants
/// cannot retain the wrapper's own external streams after its immediate child
/// exits. Waiting on the spawned process identity survives that process
/// replacing itself with `exec`; it ends when that identity exits, regardless
/// of any separately living descendants it created.
///
/// # Errors
///
/// Returns the error when the command cannot be spawned.
pub fn hand<I, S>(program: &OsStr, arguments: I) -> io::Result<ExitStatus>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    #[cfg(unix)]
    return hand_relayed(program, arguments);

    #[cfg(not(unix))]
    {
        let mut child = Command::new(program).args(arguments).spawn()?;
        child.wait()
    }
}

#[cfg(unix)]
fn hand_relayed<I, S>(program: &OsStr, arguments: I) -> io::Result<ExitStatus>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let (stdout_read, stdout_write) = UnixStream::pair()?;
    let (stderr_read, stderr_write) = UnixStream::pair()?;
    let stops = [stdout_read.try_clone()?, stderr_read.try_clone()?];
    let mut child = Command::new(program)
        .args(arguments)
        .stdout(Stdio::from(OwnedFd::from(stdout_write)))
        .stderr(Stdio::from(OwnedFd::from(stderr_write)))
        .spawn()?;
    let (sender, receiver) = mpsc::channel();
    let relays = [
        spawn_relay(stdout_read, RelayTarget { stderr: false }, sender.clone()),
        spawn_relay(stderr_read, RelayTarget { stderr: true }, sender),
    ];
    let status = child.wait();
    let relayed = finish_relays(&receiver, &stops, relays);
    match (status, relayed) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(status), Ok(())) => Ok(status),
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct RelayTarget {
    stderr: bool,
}

#[cfg(unix)]
fn spawn_relay(
    source: UnixStream,
    target: RelayTarget,
    sender: Sender<io::Result<()>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let _ = sender.send(relay(source, target));
    })
}

#[cfg(unix)]
fn relay(mut source: UnixStream, target: RelayTarget) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        if target.stderr {
            io::stderr().lock().write_all(&buffer[..read])?;
        } else {
            io::stdout().lock().write_all(&buffer[..read])?;
        }
    }
}

#[cfg(unix)]
fn finish_relays(
    receiver: &Receiver<io::Result<()>>,
    stops: &[UnixStream; 2],
    relays: [JoinHandle<()>; 2],
) -> io::Result<()> {
    let deadline = Instant::now() + RELAY_SETTLE;
    let mut results = std::vec::Vec::with_capacity(2);
    while results.len() < 2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(result) => results.push(result),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    if results.len() < 2 {
        for stop in stops {
            let _ = stop.shutdown(Shutdown::Read);
        }
    }
    while results.len() < 2 {
        results.push(receiver.recv().map_err(|_| relay_panic())?);
    }
    for relay in relays {
        relay.join().map_err(|_| relay_panic())?;
    }
    results.into_iter().collect()
}

#[cfg(unix)]
fn relay_panic() -> io::Error {
    io::Error::other("command output relay ended without reporting its result")
}
