//! Durable-state and event-observation fixtures shared by resident cases.

use super::*;

pub(super) fn hold(path: &Path, identity: &str) -> sill::Possession {
    fs::create_dir_all(path.parent().expect("lock parent")).expect("lock directory");
    let lock = LockFile::new(
        LockName::new("resident").expect("lock name"),
        path.to_path_buf(),
    );
    let Acquisition::Acquired(possession) = lock.try_acquire().expect("lock acquisition") else {
        panic!("fixture lock unexpectedly contended");
    };
    let file = possession.file();
    file.set_len(0).expect("holder truncates");
    let mut writer = file;
    writer
        .write_all(identity.as_bytes())
        .expect("holder writes");
    writer.sync_data().expect("holder persists");
    possession
}

pub(super) fn seed_active_claim(fixture: &Fixture) {
    const PROJECT: &str = "spice.resident";

    let journal = journal::Journal::open(fixture.journal()).expect("journal opens for fixture");
    let bus = Bus::start(journal).expect("fixture bus starts");
    let task: wire::TaskStamp = "1kKRRsDP".parse().expect("task stamp");
    let lane = uuid("00000000-0000-4000-8000-000000000011");
    let occupant = uuid("00000000-0000-4000-8000-000000000012");
    let epoch = uuid("00000000-0000-4000-8000-000000000013");
    let phase: wire::Phase = "todo".parse().expect("phase");
    let origin: wire::Origin = "task:RESIDEN-active".parse().expect("origin");
    let start = instant("2030-04-12T18:44:09.271Z");
    let expiry = instant("2030-04-12T18:45:09.271Z");
    let maximum = instant("2030-04-12T18:46:09.271Z");

    let mut add = JournalWrite::new(
        Actor::Operator,
        origin.clone(),
        start,
        Producer::CommandDoor,
    );
    let added = add
        .fact(
            wire::body::TaskAdded {
                task: task.clone(),
                title: "Preserve an active claim".to_owned(),
                description: None,
                acceptance: Vec::new(),
                project: PROJECT.to_owned(),
                tags: Vec::new(),
                depends: Vec::new(),
                flow: vec![phase.clone()],
                phase: phase.clone(),
                revision: 1,
            },
            FactCause::Root,
        )
        .expect("task fact");
    let task_for_head = task.clone();
    let origin_for_head = origin.clone();
    add.task_head(added, move |seq| TaskHead {
        born: task_for_head,
        queue_origin: seq,
        title: "Preserve an active claim".to_owned(),
        description: None,
        acceptance: Vec::new(),
        project: PROJECT.to_owned(),
        tags: Vec::new(),
        dependencies: Vec::new(),
        provenance: origin_for_head,
        revision: seq,
    });
    let phase_for_head = phase.clone();
    add.workflow_head(task.clone(), added, move |_| WorkflowHead {
        flow: vec![phase_for_head.clone()],
        phase: phase_for_head,
        status: "pending".to_owned(),
        review: "unassigned".to_owned(),
        assignment: None,
        deferral: None,
        completion: None,
    });
    let added = committed(&bus, add);

    let actor = Actor::Lane(lane);
    let mut claim = JournalWrite::new(actor.clone(), origin, start, Producer::CommandDoor);
    let won = claim
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: Some(wire::ClaimCell {
                    project: PROJECT.to_owned(),
                    phase: phase.clone(),
                }),
                until: expiry,
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Existing(
                wire::Seq::new(added.last()).expect("task commit has a non-zero position"),
            ),
        )
        .expect("claim fact");
    claim.claim_head(task.clone(), won, move |seq| ClaimHead {
        actor,
        fact: seq,
        claimed: seq,
        cell: wire::ClaimCell {
            project: PROJECT.to_owned(),
            phase,
        },
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: ClaimDisposition::Active,
    });
    claim.lease_head(task, won, move |_| LeaseHead {
        start,
        renewal: start,
        maximum,
        expiry,
        epoch,
    });
    committed(&bus, claim);
    bus.shutdown().expect("fixture bus stops");
}

pub(super) fn seed_lane(
    fixture: &Fixture,
    lane: wire::CanonicalUuid,
    capability: wire::CanonicalUuid,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("lane fixture journal opens");
    let bus = Bus::start(journal).expect("lane fixture bus starts");
    let current = bus
        .residency_head()
        .expect("lane fixture head reads")
        .expect("lane fixture head exists");
    let epoch = current.body().epoch;
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        lineage_id: lane,
        name: None,
        worktree: fixture.root.display().to_string(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        occupant: None,
        lineage: Vec::new(),
        capabilities: vec![capability],
        session: None,
        disposition: "vacant".to_owned(),
    });
    next.lanes.sort_by_key(|row| row.lane);
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("lane fixture origin");
    let mut write = JournalWrite::new(
        Actor::Spice,
        origin,
        instant("2030-04-12T18:47:09.271Z"),
        Producer::ResidentRecognition,
    );
    let seated = write
        .fact(
            wire::body::LaneSeated {
                lane,
                lineage_id: lane,
                capability,
                name: None,
                worktree: fixture.root.display().to_string(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane fixture fact");
    write.residency_head(seated, move |_| next);
    committed(&bus, write);
    bus.shutdown().expect("lane fixture bus stops");
}

pub(super) fn committed(bus: &Bus, write: JournalWrite) -> bus::JournalCommit {
    match bus.commit(write).expect("fixture commit reaches writer") {
        CommitOutcome::Committed(committed) => committed,
        outcome => panic!("fixture write did not commit: {outcome:?}"),
    }
}

pub(super) fn uuid(value: &str) -> wire::CanonicalUuid {
    value.parse().expect("canonical fixture UUID")
}

pub(super) fn instant(value: &str) -> Instant {
    value.parse().expect("canonical fixture instant")
}

/// Block until `observe` sees the effect of `trigger` under `path`.
///
/// The native observer arms before `trigger` runs and `observe` is read once
/// immediately afterwards, so an effect that lands during setup is still seen.
/// After that read, only a direct kernel notification can cause another
/// observation. A trigger that already ran passes `|| {}` and relies on the
/// post-arm read.
///
/// Panics report the waiting call site rather than this shared helper.
#[track_caller]
pub(super) fn wait_for_event<T>(
    path: &Path,
    trigger: impl FnOnce(),
    mut observe: impl FnMut() -> Option<T>,
) -> T {
    let (sender, receiver) = mpsc::channel::<std::io::Result<()>>();
    let observers = effect_observers(path);
    trigger();
    if let Some(observed) = observe() {
        return observed;
    }
    wait_for_effect_change(observers, sender.clone());
    let deadline = Monotonic::now() + LANE_EFFECT_HORIZON;
    let mut change_signals = 0_u64;
    loop {
        let remaining = deadline.saturating_duration_since(Monotonic::now());
        match receiver.recv_timeout(remaining) {
            Ok(Ok(())) => {
                change_signals += 1;
                let observers = effect_observers(path);
                if let Some(observed) = observe() {
                    return observed;
                }
                wait_for_effect_change(observers, sender.clone());
            }
            Ok(Err(error)) => panic!("effect observer failed: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!(
                    "effect was not observed before {:?}: change_signals={change_signals}",
                    LANE_EFFECT_HORIZON,
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("effect observer disconnected")
            }
        }
    }
}

pub(super) fn wait_for_lane_exit(fixture: &Fixture, lane: wire::CanonicalUuid) {
    wait_for_event(
        &fixture.runtime.join("data"),
        || {},
        || {
            let snapshot = journal::JournalReader::open(fixture.journal())
                .ok()?
                .fact_snapshot()
                .ok()?;
            snapshot
                .facts()
                .iter()
                .any(|fact| {
                    fact.fact.kind() == FactKind::LaneExited
                        && fact.fact.body().get("lane")
                            == Some(&serde_json::Value::String(lane.to_string()))
                })
                .then_some(())
        },
    );
}

#[track_caller]
pub(super) fn wait_for_vendor_exit(watch: &Path, exited: &Path) {
    let marker = wait_for_event(watch, || {}, || recorded_stop(exited));
    assert_eq!(marker, "exited", "vendor exit marker is complete");
}

pub(super) fn effect_observers(path: &Path) -> Vec<readiness::filesystem_event::DirectObserver> {
    effect_observers_before_arm(path, |_| {})
}

pub(super) fn effect_observers_before_arm(
    path: &Path,
    mut before_arm: impl FnMut(&Path),
) -> Vec<readiness::filesystem_event::DirectObserver> {
    let root = if path.is_dir() {
        path
    } else {
        path.parent().expect("effect path has a parent")
    };
    let mut watched = vec![(root.to_path_buf(), false)];
    if path.is_file() {
        watched.push((path.to_path_buf(), false));
    } else {
        watched.extend(
            fs::read_dir(root)
                .expect("effect directory reads")
                .map(|entry| entry.expect("effect directory entry reads").path())
                .filter(|entry| entry.is_file())
                .map(|entry| (entry, true)),
        );
    }
    watched.sort_by(|(left, _), (right, _)| left.cmp(right));
    watched.dedup_by(|(left, _), (right, _)| left == right);
    watched
        .into_iter()
        .filter_map(|(watched, incidental)| {
            before_arm(&watched);
            match readiness::filesystem_event::DirectObserver::try_arm(&watched) {
                Ok(observer) => Some(observer),
                Err(error) if incidental && error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("effect observer arms on {}: {error}", watched.display()),
            }
        })
        .collect()
}

pub(super) fn wait_for_effect_change(
    observers: Vec<readiness::filesystem_event::DirectObserver>,
    sender: mpsc::Sender<std::io::Result<()>>,
) {
    for observer in observers {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(observer.wait());
        });
    }
}

/// Block until the fixture vendor records its stop, or fail as soon as the
/// vendor is gone without recording one.
///
/// The stop file is a proxy for the event these tests want, which is the vendor
/// ending. A vendor whose TERM trap outlives its termination grace is killed
/// before the trap writes, and the file it would have written can never appear.
/// Waiting for it would only delay the failure and then report a timeout, which
/// describes the observation rather than what happened to the vendor.
///
/// A missing stop has three causes that the grace cannot separate on its own,
/// so the fixture witnesses two things the panic reads back: the trap records
/// its own entry first, and the vendor reports the traps it actually installed.
/// The handler lost the race, the machine never ran it, or TERM could never
/// reach it at all -- and the panic names exactly one of them. A fixture built
/// without [`vendor_prologue`] witnesses neither, and is told so rather than
/// being read as the vendor's fault. See [`entered_marker`] and
/// [`trap_report`].
///
/// The operator command returns only after the resident has terminated and
/// reaped the vendor, so in that case the first observation already sees a
/// vendor that is gone and reports it.
///
/// Panics report the waiting call site rather than this shared helper.
#[track_caller]
pub(super) fn wait_for_vendor_stop(watch: &Path, pid_file: &Path, stopped: &Path) -> String {
    let observed = wait_for_event(
        watch,
        || {},
        || {
            if let Some(stop) = recorded_stop(stopped) {
                return Some(Ok(stop));
            }
            let pid = recorded_pid(pid_file)?;
            if process_alive(pid) {
                return None;
            }
            // The vendor is gone. One more read closes the window where it
            // wrote its stop between these two observations.
            Some(recorded_stop(stopped).ok_or(pid))
        },
    );
    match observed {
        Ok(stop) => stop,
        Err(pid) if entered_marker(stopped).exists() => panic!(
            "vendor {pid} entered its TERM trap and ended before recording a \
             stop in {}; the handler ran but did not finish inside the \
             termination grace",
            stopped.display()
        ),
        Err(pid) => match trap_report(stopped) {
            TrapReport::Installed => panic!(
                "vendor {pid} ended without entering its TERM trap, which it \
                 had installed and reported back; the machine never scheduled \
                 the handler inside the termination grace"
            ),
            TrapReport::Declined => panic!(
                "vendor {pid} ended without entering its TERM trap and never \
                 had one: TERM was already ignored when the fixture started, \
                 so its trap builtin could not take effect and the signal \
                 could not move it"
            ),
            TrapReport::Unreported => panic!(
                "vendor {pid} never reported its traps, so this fixture was \
                 not built from vendor_prologue and nothing here can say what \
                 the vendor did with TERM; instrument the fixture rather than \
                 reading this as a vendor fault"
            ),
        },
    }
}

/// The marker a fixture vendor writes as the first action of its TERM trap.
///
/// Derived from the stop path rather than passed alongside it so a fixture
/// cannot record one without the other. The two files split the single failure
/// this helper used to report: a handler that ran and lost the race to the
/// grace writes the marker and not the stop, while a handler that never ran
/// writes neither.
pub(super) fn entered_marker(stopped: &Path) -> PathBuf {
    let mut marker = stopped.as_os_str().to_owned();
    marker.push("-entered");
    PathBuf::from(marker)
}

/// What a fixture vendor reported about its own TERM trap.
pub(super) enum TrapReport {
    /// The vendor reported a TERM trap running its own entry marker.
    Installed,
    /// The vendor reported its traps and TERM was not among them, which a
    /// shell says exactly when TERM was already ignored as it started.
    Declined,
    /// The vendor reported nothing, so it was not built from
    /// [`vendor_prologue`] and its behaviour cannot be read at all.
    Unreported,
}

/// Read back whether the fixture vendor's TERM trap ever took effect.
///
/// A non-interactive shell cannot trap a signal that was already ignored when
/// it started: the trap builtin accepts the request and silently declines it.
/// That is the difference between a vendor TERM could not move and one the
/// machine never scheduled, and it is not observable from timing, so each
/// fixture reports its own installed traps and this reads the answer back.
///
/// Written before the pid file, so a recorded pid always implies a readable
/// readout. A readout that is missing anyway cannot mean the vendor declined
/// the trap; it means the fixture never had the prologue, which is why that is
/// a third answer here rather than folded into [`TrapReport::Declined`]. The
/// line must name TERM as well as the handler, because the same trap command
/// also covers INT, and a shell that declined only TERM would still print the
/// handler on the surviving INT line.
pub(super) fn trap_report(stopped: &Path) -> TrapReport {
    let handler = entered_marker(stopped).display().to_string();
    let Ok(traps) = fs::read_to_string(trap_record(stopped)) else {
        return TrapReport::Unreported;
    };
    if traps
        .lines()
        .any(|trap| trap.contains("TERM") && trap.contains(&handler))
    {
        TrapReport::Installed
    } else {
        TrapReport::Declined
    }
}

/// The readout of its own traps that a fixture vendor writes at startup.
///
/// Derived from the stop path for the same reason [`entered_marker`] is: a
/// fixture cannot report one path while trapping another.
pub(super) fn trap_record(stopped: &Path) -> PathBuf {
    let mut record = stopped.as_os_str().to_owned();
    record.push("-traps");
    PathBuf::from(record)
}

/// The opening lines every fixture vendor shares.
///
/// A TERM trap that records its own entry before recording the stop, the
/// vendor's readout of the traps it actually installed, and the pid the
/// resident is asked to terminate. That order is load-bearing:
/// [`wait_for_vendor_stop`] only diagnoses after reading a pid, so a recorded
/// pid implies a readout already on disk and the diagnosis never has to guess.
///
/// Built here rather than spelled out per fixture because every fixture that
/// hand-copied these lines had to be found and edited again each time the
/// diagnosis learned something new, and the one that gets missed is not a
/// visible gap but a confident wrong answer.
pub(super) fn vendor_prologue(stopped: &Path, vendor_pid: &Path) -> String {
    format!(
        concat!(
            "#!/bin/sh\n",
            "trap 'printf entered > {}; printf stopped > {}; exit 0' TERM INT\n",
            "trap > {}\n",
            "printf '%s\\n' \"$$\" > {}\n",
        ),
        shell_word(&entered_marker(stopped).display().to_string()),
        shell_word(&stopped.display().to_string()),
        shell_word(&trap_record(stopped).display().to_string()),
        shell_word(&vendor_pid.display().to_string()),
    )
}

/// A path or word quoted for the fixture shell scripts written above.
pub(super) fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The stop a fixture vendor finished recording, if it recorded one.
///
/// The trap redirect creates the file before the write lands, so an empty file
/// is a vendor still writing rather than a vendor that stopped.
pub(super) fn recorded_stop(stopped: &Path) -> Option<String> {
    fs::read_to_string(stopped)
        .ok()
        .filter(|text| !text.is_empty())
}

/// The identity a fixture vendor recorded for itself, if it got that far.
pub(super) fn recorded_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

#[cfg(target_os = "linux")]
pub(super) fn process_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(") ")
            .and_then(|(_, fields)| fields.as_bytes().first().copied())
            .is_none_or(|state| !matches!(state, b'Z' | b'X' | b'x')),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => process_identity_exists(pid),
    }
}

#[cfg(not(target_os = "linux"))]
pub(super) fn process_alive(pid: u32) -> bool {
    process_identity_exists(pid)
}

pub(super) fn process_identity_exists(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .expect("process probe launches")
        .status
        .success()
}
