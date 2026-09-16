//! Installed-wrapper claim continuity across real resident replacement.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use bus::{Bus, CommitOutcome};
use journal::{FactCause, JournalWrite};
use wire::{Actor, CanonicalUuid, Instant, Producer};

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

const SESSION_ID: &str = "018f7777-7777-7444-8888-999999999999";
const SESSION_RECORD: &str =
    r#"{"type":"system","subtype":"init","session_id":"018f7777-7777-7444-8888-999999999999"}"#;

struct Fixture {
    root: PathBuf,
    runtime: PathBuf,
    cargo_home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "claim-refresh-process");
        let runtime = root.join(".git/.spice-rs");
        let cargo_home = root.join("fixture-cargo-home");
        fs::create_dir(root.join(".git")).expect("fixture git directory");
        fs::create_dir(&cargo_home).expect("fixture bootstrap Cargo home");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"claim-refresh-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture Cargo manifest");
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"claim-refresh-fixture\"\nversion = \"0.0.0\"\n",
        )
        .expect("fixture Cargo lockfile");
        let skill = root.join(".git/.spice-install/agents/skills/spice/SKILL.md");
        fs::create_dir_all(skill.parent().expect("fixture skill parent"))
            .expect("fixture skill parent creates");
        fs::write(&skill, ::std::include_bytes!("../agent/SKILL.md"))
            .expect("fixture installs appliance skill");
        staged_app::seed(&root);
        let vendor = root.join("fake-claude");
        fs::write(
            &vendor,
            concat!(
                "#!/bin/sh\n",
                "trap 'exit 0' TERM INT\n",
                "printf '%s\\n' '",
                "{\"type\":\"system\",\"subtype\":\"init\",",
                "\"session_id\":\"018f7777-7777-7444-8888-999999999999\"}'\n",
                "while [ -d .git ]; do sleep 1; done\n",
            ),
        )
        .expect("fixture vendor writes");
        #[cfg(unix)]
        fs::set_permissions(&vendor, fs::Permissions::from_mode(0o700))
            .expect("fixture vendor becomes executable");
        fs::write(
            root.join("spice-rust.toml"),
            format!(
                concat!(
                    "[lane]\nlease = \"20s\"\n\n",
                    "[tasks]\nstems = [\"taste\"]\n\n",
                    "[vendor.claude]\nbinary = {:?}\n",
                ),
                vendor.display().to_string()
            ),
        )
        .expect("fixture vendor configuration writes");
        Self {
            root,
            runtime,
            cargo_home,
        }
    }

    fn journal(&self) -> PathBuf {
        self.runtime.join("data/spicejournal.sqlite3")
    }

    fn tape(&self, lane: CanonicalUuid) -> PathBuf {
        self.runtime
            .join("tape")
            .join(lane.to_string())
            .join("claude")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        make_fixture_writable(&self.root);
        fs::remove_dir_all(&self.root).expect("fixture cleanup");
    }
}

fn make_fixture_writable(path: &std::path::Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .expect("fixture directory becomes writable");
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_fixture_writable(&entry.path());
            }
        }
    } else {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("fixture file becomes writable");
    }
}

struct RunningResident {
    child: Option<Child>,
}

impl RunningResident {
    fn spawn(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.root)
            .arg(spice::install::TEST_CANVAS_STAGE_ARGUMENT)
            .env(
                spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
                &fixture.cargo_home,
            )
            .env(
                spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
                fixture_native_tool("cargo"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
                fixture_native_tool("rustc"),
            )
            .env(
                spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
                fixture_native_tool("rustdoc"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("foreground resident launches");
        Self { child: Some(child) }
    }

    fn assert_ready(&mut self, fixture: &Fixture) {
        fs::create_dir_all(&fixture.runtime).expect("runtime directory");
        match readiness::wait_until_ready(
            self.child.as_mut().expect("resident child"),
            &fixture.runtime,
            readiness::ARRIVAL_HORIZON,
            "resident discovery",
            || fixture.runtime.join("canvas").is_file(),
        ) {
            Readiness::Ready => {}
            Readiness::Exited => self.assert_running(),
        }
    }

    fn assert_running(&mut self) {
        if self
            .child
            .as_mut()
            .expect("resident child")
            .try_wait()
            .expect("resident status")
            .is_none()
        {
            return;
        }
        let output = self
            .child
            .take()
            .expect("exited resident child")
            .wait_with_output()
            .expect("exited resident output");
        panic!(
            "foreground resident exited: stdout={:?} stderr={:?}",
            output.stdout, output.stderr
        );
    }

    fn wait(mut self) -> Output {
        self.child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("foreground resident exits")
    }
}

impl Drop for RunningResident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn fixture_native_tool(name: &str) -> PathBuf {
    let tool = PathBuf::from(env!("CARGO"))
        .parent()
        .expect("Cargo test executable has one toolchain directory")
        .join(name);
    assert!(tool.is_file(), "native fixture tool exists: {tool:?}");
    tool
}

fn invoke(fixture: &Fixture, arguments: &[&str], json: bool) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_spice"));
    process.env_remove(steer::hook::LANE);
    if json {
        process.arg("--json");
    }
    process
        .args(arguments)
        .current_dir(&fixture.root)
        .output()
        .expect("spice launches")
}

fn stop(fixture: &Fixture, resident: RunningResident) {
    let down = invoke(fixture, &["down"], true);
    assert!(down.status.success(), "down failed: {:?}", down.stderr);
    assert!(resident.wait().status.success(), "resident failed");
}

fn seed_occupied_lane(
    fixture: &Fixture,
    lane: CanonicalUuid,
    capability: CanonicalUuid,
    occupant: CanonicalUuid,
) {
    let journal = journal::Journal::open(fixture.journal()).expect("lane fixture journal opens");
    let bus = Bus::start(journal).expect("lane fixture bus starts");
    let current = bus
        .residency_head()
        .expect("lane fixture head reads")
        .expect("lane fixture head exists");
    let epoch = current.body().epoch;
    let worktree = fs::canonicalize(&fixture.root)
        .expect("lane fixture worktree canonicalizes")
        .display()
        .to_string();
    let mut declaration = SESSION_RECORD.as_bytes().to_vec();
    declaration.push(b'\n');
    let locus = tape::Tape::open(fixture.tape(lane))
        .expect("lane fixture tape opens")
        .tee(&declaration)
        .expect("session declaration reaches tape");
    let session = wire::LaneSession {
        lane,
        occupant,
        worktree: worktree.clone(),
        vendor: "claude".parse().expect("fixture vendor"),
        launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", "high")),
        epoch,
        stream: "stdout".to_owned(),
        identity: SESSION_ID.to_owned(),
        locus: wire::TapeLocus {
            member: locus.member,
            offset: locus.offset,
            length: locus.length,
        },
    };
    let mut next = current.body().clone();
    next.lanes.push(wire::LaneResidency {
        lane,
        lineage_id: lane,
        name: None,
        worktree: worktree.clone(),
        life: "drive".to_owned(),
        vendor: "claude".parse().expect("fixture vendor"),
        model: None,
        effort: None,
        occupant: Some(occupant),
        lineage: vec![occupant],
        capabilities: vec![capability],
        session: Some(session.clone()),
        disposition: "occupied".to_owned(),
    });
    next.lanes.sort_by_key(|row| row.lane);
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
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
                worktree,
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                model: None,
                effort: None,
            },
            FactCause::Existing(current.position()),
        )
        .expect("lane fixture fact");
    let declared = write
        .fact(
            wire::body::LaneSessionDeclared {
                lane: session.lane,
                occupant: session.occupant,
                worktree: session.worktree,
                vendor: session.vendor,
                launch: session.launch,
                epoch: session.epoch,
                stream: session.stream,
                identity: session.identity,
                member: session.locus.member,
                offset: session.locus.offset,
                length: session.locus.length,
            },
            FactCause::Pending(seated),
        )
        .expect("session fixture fact");
    let started = write
        .fact(
            wire::body::LaneStarted {
                lane,
                occupant,
                epoch,
                group: 123,
            },
            FactCause::Pending(declared),
        )
        .expect("occupant fixture fact");
    write.residency_head(started, move |_| next);
    assert!(matches!(
        bus.commit(write).expect("fixture commit reaches writer"),
        CommitOutcome::Committed(_)
    ));
    bus.shutdown().expect("fixture bus stops");
}

fn instant(value: &str) -> Instant {
    value.parse().expect("fixture instant is canonical")
}

fn uuid(value: &str) -> CanonicalUuid {
    value.parse().expect("fixture UUID is canonical")
}

#[path = "steering/claim_refresh.rs"]
mod claim_refresh;
