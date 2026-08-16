use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant as Monotonic};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use rusqlite::{Connection, params};
use sill::{Acquisition, LockFile, LockName};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::ORIGIN;
use tungstenite::{Message, WebSocket};
use wire::{
    Actor, ClaimDisposition, ClaimHead, FactKind, Instant, LeaseHead, Outcome, Producer,
    ReplyPosition, TaskHead, WorkflowHead,
};

#[path = "support/staged_app.rs"]
mod staged_app;

const FAULT_EXIT: i32 = 1;
const REFUSAL_EXIT: i32 = 2;
/// The longest a fixture client command may take, and the longest a fixture
/// socket waits while one is in flight. This is not itself the client answer
/// bound: `down` can spend one answer bound on the resident response and then a
/// second on release confirmation, while these tests only need a process-level
/// guard that still catches an unbounded wait.
const CLIENT_DEADLINE_BOUND: Duration = Duration::from_secs(45);
const LANE_DEADLINE_BOUND: Duration = Duration::from_secs(20);
/// The hang guard around a supervised vendor effect reaching its tape.
///
/// The wait itself is event-driven; this covers the full vendor, capture,
/// decode, and persistence pipeline rather than resident discovery.
const LANE_EFFECT_HORIZON: Duration = Duration::from_secs(45);
/// Keep a fake peer alive beyond the process-level guard so scheduler delay
/// cannot turn the client's elapsed bound into a peer-close transport fault.
/// Every normal path still stops the peer immediately through its shared flag.
const ADVERSARIAL_PEER_LIFE: Duration = Duration::from_secs(60);

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn seed_fixture_app(root: &Path) {
    staged_app::seed(root);
}

#[path = "resident/process_support.rs"]
mod process_support;
use process_support::*;

#[path = "resident/effect_support.rs"]
mod effect_support;
use effect_support::*;

/// Process cases share the transport and lifecycle fixtures defined above.
#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

#[path = "resident/cases.rs"]
mod cases;

#[path = "resident/daemon_cases.rs"]
mod daemon_cases;

#[path = "resident/event_wait.rs"]
mod event_wait;

#[path = "resident/lane_process.rs"]
mod lane_process;

#[path = "resident/lane_continuity.rs"]
mod lane_continuity;

#[path = "resident/lane_resume.rs"]
mod lane_resume;

#[path = "resident/automatic_lifecycle.rs"]
mod automatic_lifecycle;

#[path = "resident/steering_authority.rs"]
mod steering_authority;

#[path = "resident/vendor_process.rs"]
mod vendor_process;
