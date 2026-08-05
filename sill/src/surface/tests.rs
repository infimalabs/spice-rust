use std::cell::Cell;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use super::owner::{
    GuardianState, LockOwnerEvidence, guardian_state, owner_record, read_owner, replacement_record,
    resolve_refusal, write_owner, write_owner_with,
};
use super::process_state::{
    process_group_absent, process_group_terminal, process_terminal, process_zombie,
};
use super::{
    Contention, LockAttempt, LockAvailability, LockSurfaceError, LockTargetKind, LockTopology,
    ProtectedGroup,
};
use crate::LockConfig;

const BUSY_EXIT: u64 = 71;
const TAKEN_EXIT: u64 = 72;
const FULL_EXIT: u64 = 73;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sill-surface-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("exclusive scratch root is created");
        Self(fs::canonicalize(path).expect("scratch root canonicalizes"))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn topology(root: &Path) -> LockTopology {
    let config: LockConfig = toml::from_str(&format!(
        concat!(
            "busy = {}\n",
            "taken = {}\n",
            "full = {}\n",
            "[single]\npath = \"single.lock\"\n",
            "[pool]\ndirectory = \"pool\"\nshards = 2\n",
        ),
        BUSY_EXIT, TAKEN_EXIT, FULL_EXIT,
    ))
    .expect("fixture configuration is admitted");
    LockTopology::new(root, &config)
}

#[test]
fn report_is_stable_and_does_not_materialize_the_topology() {
    let scratch = Scratch::new("report");
    let root = scratch.path().join("state/locks");
    let report = topology(&root).report().expect("topology is observable");

    assert_eq!(report.root, root.display().to_string());
    assert_eq!(report.exits.busy, BUSY_EXIT as i32);
    assert_eq!(report.exits.taken, TAKEN_EXIT as i32);
    assert_eq!(report.exits.full, FULL_EXIT as i32);
    assert_eq!(
        report
            .pools
            .iter()
            .map(|pool| (pool.name.as_str(), pool.kind, pool.shards))
            .collect::<Vec<_>>(),
        [
            ("pool", LockTargetKind::Pool, 2),
            ("single", LockTargetKind::File, 1),
        ]
    );
    assert_eq!(
        report.pools[0]
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        [
            root.join("pool/0.lock").display().to_string(),
            root.join("pool/1.lock").display().to_string(),
        ]
    );
    assert!(
        report
            .pools
            .iter()
            .flat_map(|pool| &pool.entries)
            .all(|entry| entry.availability == LockAvailability::Available)
    );
    assert!(!root.exists(), "reporting created lock state");
}

#[test]
fn report_observes_held_and_released_shards_without_creating_missing_ones() {
    let scratch = Scratch::new("availability");
    let root = scratch.path().join("locks");
    let topology = topology(&root);
    let held = match topology
        .try_acquire("pool", Some(0))
        .expect("selected shard is observable")
    {
        LockAttempt::Acquired(held) => held,
        LockAttempt::Contended { .. } => panic!("fresh shard was contended"),
    };

    let report = topology.report().expect("held topology is observable");
    let pool = report
        .pools
        .iter()
        .find(|pool| pool.name == "pool")
        .expect("pool is reported");
    assert_eq!(pool.entries[0].availability, LockAvailability::Held);
    assert_eq!(pool.entries[1].availability, LockAvailability::Available);
    assert!(root.join("pool/0.lock").is_file());
    assert!(
        !root.join("pool/1.lock").exists(),
        "observing an absent shard created it"
    );
    assert_eq!(
        pool.entries[0].owner.as_ref().map(|owner| owner.pid),
        Some(std::process::id())
    );
    assert!(!pool.entries[0].owner_stale);

    drop(held);
    let released = topology.report().expect("released topology is observable");
    let pool = released
        .pools
        .iter()
        .find(|pool| pool.name == "pool")
        .expect("pool is reported");
    assert_eq!(pool.entries[0].availability, LockAvailability::Available);
    assert_eq!(pool.entries[0].owner, None);
    assert!(!pool.entries[0].owner_stale);
}

/// One pid that is proved departed rather than assumed to be.
///
/// A literal pid is departed only by luck: this machine runs eleven lanes
/// and a full run spawns hundreds of children, so any number small enough
/// to write down is one something else can be holding. A child this test
/// spawned and reaped is a pid the kernel will not hand out again until it
/// has wrapped the whole space, and the assertion keeps even that from
/// being a premise -- a reused pid fails here, where the cause is named.
fn departed_pid() -> u32 {
    let mut departed = Command::new("/usr/bin/true");
    departed.process_group(0);
    let mut departed = departed.spawn().expect("departed process launches");
    let pid = departed.id();
    assert!(departed.wait().expect("departed process reaps").success());
    assert!(
        process_terminal(pid),
        "pid {pid} was live again between the reap and the naming"
    );
    pid
}

#[test]
fn stale_owner_evidence_never_becomes_kernel_possession() {
    let scratch = Scratch::new("stale-owner");
    let root = scratch.path().join("locks");
    let path = root.join("pool/0.lock");
    fs::create_dir_all(path.parent().expect("stale owner parent"))
        .expect("stale owner parent creates");
    let departed = departed_pid();
    fs::write(&path, format!(r#"{{"pid":{departed},"cwd":"/departed"}}"#))
        .expect("stale owner evidence writes");
    let topology = topology(&root);

    let report = topology.report().expect("stale evidence is observable");
    let entry = &report
        .pools
        .iter()
        .find(|pool| pool.name == "pool")
        .expect("pool is reported")
        .entries[0];
    assert_eq!(entry.availability, LockAvailability::Available);
    assert_eq!(entry.owner.as_ref().map(|owner| owner.pid), Some(departed));
    assert!(entry.owner_stale);

    let held = match topology
        .try_acquire("pool", Some(0))
        .expect("stale evidence does not contend")
    {
        LockAttempt::Acquired(held) => held,
        LockAttempt::Contended { .. } => panic!("stale evidence became possession"),
    };
    let current = topology.report().expect("replacement owner is observable");
    let entry = &current
        .pools
        .iter()
        .find(|pool| pool.name == "pool")
        .expect("pool is reported")
        .entries[0];
    assert_eq!(entry.availability, LockAvailability::Held);
    assert_eq!(
        entry.owner.as_ref().map(|owner| owner.pid),
        Some(std::process::id())
    );
    assert!(!entry.owner_stale);
    drop(held);
}

/// A departed guardian must send the question back to the kernel, and only
/// a departed guardian may. The difference is otherwise visible only inside
/// the moment between collecting a refusal and reading the record against
/// it, so what each state does is asserted here rather than raced for.
#[test]
fn only_a_departed_guardian_puts_a_collected_refusal_back_to_the_kernel() {
    fn resolved(state: GuardianState) -> (&'static str, u32, u32) {
        let waited = Cell::new(0);
        let reasked = Cell::new(0);
        let answer = resolve_refusal(
            state,
            "collected",
            || {
                waited.set(waited.get() + 1);
                Ok("waited")
            },
            || {
                reasked.set(reasked.get() + 1);
                Ok("reasked")
            },
        );
        (
            answer.expect("every state answers"),
            waited.get(),
            reasked.get(),
        )
    }

    assert_eq!(resolved(GuardianState::Departed), ("reasked", 0, 1));
    assert_eq!(resolved(GuardianState::Pending), ("waited", 1, 0));
    assert_eq!(resolved(GuardianState::Unbounded), ("collected", 0, 0));
}

#[test]
fn guardian_synchronization_separates_a_pending_guardian_from_a_departed_one() {
    let pid = departed_pid();

    let completed = LockOwnerEvidence {
        pid,
        cwd: "/departed".to_owned(),
        owner_released: false,
        protected_group: Some(ProtectedGroup::Published(pid)),
        protected_guardian: Some(std::process::id()),
    };
    assert_eq!(completed.guardian_state(), GuardianState::Pending);

    let live_owner = LockOwnerEvidence {
        pid: std::process::id(),
        cwd: "/current".to_owned(),
        owner_released: false,
        protected_group: Some(ProtectedGroup::Published(pid)),
        protected_guardian: Some(std::process::id()),
    };
    assert_eq!(live_owner.guardian_state(), GuardianState::Unbounded);

    let released_owner = LockOwnerEvidence {
        owner_released: true,
        ..live_owner
    };
    assert!(released_owner.report().is_none());
    assert_eq!(released_owner.guardian_state(), GuardianState::Pending);

    let unguarded = LockOwnerEvidence {
        pid,
        cwd: "/departed".to_owned(),
        owner_released: false,
        protected_group: None,
        protected_guardian: None,
    };
    assert_eq!(unguarded.guardian_state(), GuardianState::Unbounded);

    // A slot reserved before the fork and never filled in is the state a
    // wrapper killed in that window leaves behind. Nothing about it has
    // finished, and reading it as a group that has -- which is what any
    // stand-in group id would do, since an id nobody uses probes as absent
    // and absent reads as terminal -- would hand the lock to a second
    // holder while the first one is still running under a live guardian.
    let reserved = LockOwnerEvidence {
        protected_group: Some(ProtectedGroup::Blank),
        ..released_owner.clone()
    };
    assert_eq!(reserved.guardian_state(), GuardianState::Unbounded);

    let departed_guardian = LockOwnerEvidence {
        pid,
        cwd: "/departed".to_owned(),
        owner_released: false,
        protected_group: Some(ProtectedGroup::Published(pid)),
        protected_guardian: Some(pid),
    };
    // The guardian is gone rather than merely uninteresting, and a guardian
    // releases what it held on its way out. Collapsing this into the same
    // answer as an unbounded record is what made a reader quote a refusal
    // its holder had already withdrawn.
    assert_eq!(departed_guardian.guardian_state(), GuardianState::Departed);
}

/// A reader inside the owner rewrite finds the record being replaced.
///
/// The rewrite hands over the one instant a concurrent reader could land
/// in, because no caller can hold that instant open. The reader here opens
/// the path for itself, exactly as another process would, so what it finds
/// is what that process would have found.
///
/// Emptying the file before rewriting it puts nothing there instead, and
/// nothing is how release is spelled: read_owner answers None,
/// guardian_state answers Unbounded, and a reader that would have waited
/// out a guardian still holding possession refuses an acquisition the
/// evidence would have granted.
///
/// The reading is counted because every other claim here is made inside the
/// callback, and a rewrite that never called back would satisfy them all by
/// never testing them.
#[test]
fn a_reader_inside_the_owner_rewrite_never_finds_an_unheld_lock() {
    let scratch = Scratch::new("rewrite-window");
    let path = scratch.path().join("owner");
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .expect("the owner record opens");

    // A record a reader is entitled to wait out: the protected group has
    // ended and its owner released it, while the guardian it names is still
    // here. That is the acquisition the empty window throws away.
    write_owner(
        &file,
        Some((
            ProtectedGroup::Published(departed_pid()),
            std::process::id(),
        )),
        true,
    )
    .expect("the record being replaced is written");
    let spanned = file.metadata().expect("the seeded record measures").len();
    let seeded = read_owner(&file).expect("the seeded record reads");
    assert_eq!(
        guardian_state(seeded.as_ref()),
        GuardianState::Pending,
        "the seeded record must be one a reader would have waited out"
    );

    let readings = Cell::new(0);
    let inside = Cell::new(None);
    write_owner_with(&file, None, false, || {
        let reader = fs::File::open(&path)?;
        readings.set(readings.get() + 1);
        inside.set(Some(guardian_state(read_owner(&reader)?.as_ref())));
        Ok(())
    })
    .expect("the replacement record is written");

    assert_eq!(
        readings.get(),
        1,
        "the rewrite offered its window exactly once"
    );
    assert_eq!(
        inside.get(),
        Some(GuardianState::Pending),
        "a reader inside the rewrite found the lock released"
    );
    assert!(
        file.metadata().expect("the replacement measures").len() >= spanned,
        "the replacement still covers every byte its predecessor spanned"
    );
}

#[test]
fn a_replacement_record_supersedes_every_byte_the_previous_one_spanned() {
    let record = replacement_record(br#"{"pid":7}"#.to_vec(), 40);
    assert_eq!(record.len(), 40, "the write covers the whole span");
    assert!(record.starts_with(b"{\"pid\":7}\n"));
    assert!(
        record[10..].iter().all(|byte| *byte == b' '),
        "the tail is whitespace a reader can ignore"
    );
    let padded: serde_json::Value = serde_json::from_slice(&record).expect("padded record reads");
    assert_eq!(padded["pid"], 7);

    // A record longer than the span it replaces is written whole, and a
    // fresh file spans nothing at all; neither case may clip the record.
    assert_eq!(
        replacement_record(br#"{"pid":7}"#.to_vec(), 4),
        b"{\"pid\":7}\n"
    );
    assert_eq!(
        replacement_record(br#"{"pid":7}"#.to_vec(), 0),
        b"{\"pid\":7}\n"
    );
}

#[test]
fn a_reserved_slot_is_filled_in_place_and_never_moves_under_the_guardian() {
    use crate::protected::{GROUP_SLOT_BLANK, GROUP_SLOT_BYTES, GROUP_SLOT_PREFIX, group_slot};

    let guardian = std::process::id();
    let reserved = serde_json::to_vec(
        &owner_record(Some((ProtectedGroup::Blank, guardian)), false).expect("owner reads"),
    )
    .expect("a reserved record serializes");
    let start = reserved
        .windows(GROUP_SLOT_PREFIX.len())
        .position(|window| window == GROUP_SLOT_PREFIX)
        .expect("a reserved record carries the slot the guardian seeks")
        + GROUP_SLOT_PREFIX.len();
    assert!(
        reserved[start..start + GROUP_SLOT_BYTES]
            .iter()
            .all(|byte| *byte == GROUP_SLOT_BLANK),
        "the slot is reserved blank, at its full width"
    );

    // Every later record this owner writes puts the slot in the same place,
    // which is the whole of why the guardian may write into an offset it
    // read a moment ago. `owner_released` is the field that would break it
    // by appearing between the two, so it is the one asked here.
    for record in [
        owner_record(Some((ProtectedGroup::Published(4242), guardian)), false),
        owner_record(Some((ProtectedGroup::Published(4242), guardian)), true),
    ] {
        let record = serde_json::to_vec(&record.expect("owner reads")).expect("serializes");
        assert_eq!(
            &record[start - GROUP_SLOT_PREFIX.len()..start],
            GROUP_SLOT_PREFIX,
            "the slot moved: {}",
            String::from_utf8_lossy(&record)
        );
    }

    // What the guardian does is exactly this: the reserved bytes, replaced
    // where they lie, by a record it never re-serializes.
    let mut published = reserved.clone();
    published[start..start + GROUP_SLOT_BYTES].copy_from_slice(&group_slot(4242));
    let published: LockOwnerEvidence =
        serde_json::from_slice(&published).expect("a filled record reads");
    assert_eq!(
        published.protected_group,
        Some(ProtectedGroup::Published(4242))
    );
    let reserved: LockOwnerEvidence =
        serde_json::from_slice(&reserved).expect("a reserved record reads");
    assert_eq!(reserved.protected_group, Some(ProtectedGroup::Blank));
    assert_eq!(published.protected_guardian, reserved.protected_guardian);
}

#[test]
fn unreaped_zombie_process_and_group_are_terminal() {
    let mut command = Command::new("/usr/bin/true");
    command.process_group(0);
    let mut child = command.spawn().expect("zombie fixture launches");
    let pid = child.id();
    let mut information = std::mem::MaybeUninit::<libc::siginfo_t>::uninit();
    // SAFETY: waitid observes this child without reaping it because WNOWAIT is set.
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                information.as_mut_ptr(),
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );

    assert!(process_zombie(pid));
    assert!(process_terminal(pid));
    // The group's only remaining member is that zombie, and signal zero
    // still resolves the group while it goes unreaped, so a terminal answer
    // here cannot have come from the group having gone absent. It has to
    // come from enumerating the member and finding it terminal, which is
    // also what pins the PID reading of the platform enumeration: reading
    // its answer as a byte count would divide the single member away and
    // leave the group identity to answer for itself, which it refuses to do.
    assert!(!process_group_absent(pid));
    assert!(process_group_terminal(pid));
    assert!(child.wait().expect("zombie fixture reaps").success());
    assert!(process_group_absent(pid));
}

#[test]
fn stable_selection_and_each_contention_class_use_configured_codes() {
    let scratch = Scratch::new("contention");
    let topology = topology(&scratch.path().join("locks"));

    let first = match topology.try_acquire("pool", None).expect("first shard") {
        LockAttempt::Acquired(held) => held,
        LockAttempt::Contended { .. } => panic!("fresh pool was contended"),
    };
    assert_eq!(first.shard(), 0);
    let second = match topology.try_acquire("pool", None).expect("second shard") {
        LockAttempt::Acquired(held) => held,
        LockAttempt::Contended { .. } => panic!("second shard was contended"),
    };
    assert_eq!(second.shard(), 1);

    assert!(matches!(
        topology.try_acquire("pool", Some(0)).expect("chosen contention"),
        LockAttempt::Contended {
            kind: Contention::Taken,
            code
        } if code.get() == TAKEN_EXIT as i32
    ));
    assert!(matches!(
        topology.try_acquire("pool", None).expect("pool exhaustion"),
        LockAttempt::Contended {
            kind: Contention::Full,
            code
        } if code.get() == FULL_EXIT as i32
    ));

    let single = match topology.try_acquire("single", None).expect("single lock") {
        LockAttempt::Acquired(held) => held,
        LockAttempt::Contended { .. } => panic!("fresh single lock was contended"),
    };
    assert!(matches!(
        topology.try_acquire("single", None).expect("single contention"),
        LockAttempt::Contended {
            kind: Contention::Busy,
            code
        } if code.get() == BUSY_EXIT as i32
    ));

    drop(first);
    assert!(matches!(
        topology.try_acquire("pool", Some(0)).expect("released shard"),
        LockAttempt::Acquired(held) if held.shard() == 0
    ));
    drop(second);
    drop(single);
}

#[test]
fn invalid_unknown_and_out_of_range_requests_do_not_create_state() {
    let scratch = Scratch::new("selection");
    let root = scratch.path().join("locks");
    let topology = topology(&root);

    assert!(matches!(
        topology.try_acquire("../outside", None),
        Err(LockSurfaceError::InvalidName(_))
    ));
    assert!(matches!(
        topology.try_acquire("missing", None),
        Err(LockSurfaceError::UnknownName(_))
    ));
    assert!(matches!(
        topology.try_acquire("pool", Some(2)),
        Err(LockSurfaceError::ShardOutOfRange { .. })
    ));
    assert!(matches!(
        topology.try_acquire("single", Some(0)),
        Err(LockSurfaceError::ShardOnFile { .. })
    ));
    assert!(!root.exists(), "a refused selection created lock state");
}
