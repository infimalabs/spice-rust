use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};

use super::*;

pub(in crate::resident) const SESSION: &str = "018f7777-7777-7444-8888-999999999999";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub(in crate::resident) struct InactiveRtkFixture {
    pub(in crate::resident) root: PathBuf,
    pub(in crate::resident) paths: RuntimePaths,
    pub(in crate::resident) epoch: CanonicalUuid,
    pub(in crate::resident) lane: CanonicalUuid,
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) capability: CanonicalUuid,
    pub(in crate::resident) vendor: wire::Name,
    probe_log: PathBuf,
    occupant_environment: PathBuf,
}

impl InactiveRtkFixture {
    pub(in crate::resident) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-resident-inactive-rtk-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory creates");
        let root = fs::canonicalize(root).expect("fixture root canonicalizes");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"resident-inactive-rtk\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest writes");
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"resident-inactive-rtk\"\nversion = \"0.0.0\"\n",
        )
        .expect("fixture lockfile writes");
        fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").expect("fixture source writes");
        git(&root, &["init", "--quiet", "--initial-branch", "fixture"]);
        git(&root, &["config", "user.name", "Spice Test"]);
        git(&root, &["config", "user.email", "spice@example.test"]);

        let probe_log = root.join("rtk-probes");
        let rtk = root.join("obsolete-rtk");
        fs::write(
            &rtk,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "set -eu\n",
                    "printf '%s|%s\\n' \"$PWD\" \"$*\" >> '{}'\n",
                    "if [ \"$#\" -eq 1 ] && [ \"$1\" = --version ]; then\n",
                    "  printf '%s\\n' 'rtk 0.44.9'\n",
                    "  exit 0\n",
                    "fi\n",
                    "exit 91\n",
                ),
                probe_log.display(),
            ),
        )
        .expect("fixture RTK writes");
        executable(&rtk);

        let occupant_environment = root.join("occupant-environment");
        let vendor = root.join("fake-claude");
        fs::write(
            &vendor,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "set -eu\n",
                    "if [ \"${{SPICE_RS_RTK_EXECUTABLE+x}}\" = x ] || ",
                    "[ \"${{RTK_DB_PATH+x}}\" = x ]; then\n",
                    "  printf '%s\\n' present > '{}'\n",
                    "  exit 92\n",
                    "fi\n",
                    "printf '%s\\n' absent > '{}'\n",
                    "printf '%s\\n' '",
                    "{{\"type\":\"system\",\"subtype\":\"init\",",
                    "\"session_id\":\"{}\"}}'\n",
                ),
                occupant_environment.display(),
                occupant_environment.display(),
                SESSION,
            ),
        )
        .expect("fixture vendor writes");
        executable(&vendor);
        fs::write(
            root.join("spice-rust.toml"),
            format!(
                concat!(
                    "[lane]\nlease = \"20s\"\n\n",
                    "[vendor.claude]\nbinary = {:?}\n\n",
                    "[rtk]\nexecutable = {:?}\n",
                ),
                vendor.display().to_string(),
                rtk.display().to_string(),
            ),
        )
        .expect("fixture launch configuration writes");
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "fixture base"]);

        let repository = Repository::discover(&root).expect("fixture repository discovers");
        let skill = spice::install::installed_skill(&repository);
        fs::create_dir_all(skill.parent().expect("installed skill parent"))
            .expect("installed skill parent creates");
        fs::write(&skill, include_bytes!("../../../../agent/SKILL.md"))
            .expect("current installed skill writes");
        let paths = RuntimePaths::from_repository(&repository).expect("runtime paths resolve");
        Self {
            root,
            paths,
            epoch: uuid(1),
            lane: uuid(2),
            occupant: uuid(3),
            capability: uuid(4),
            vendor: "claude".parse().expect("fixture vendor name"),
            probe_log,
            occupant_environment,
        }
    }

    pub(in crate::resident) fn request(&self, resume: vendor::Resume) -> OccupantLaunchRequest<'_> {
        OccupantLaunchRequest {
            worktree: self.root.to_str().expect("fixture worktree is UTF-8"),
            vendor: &self.vendor,
            epoch: self.epoch,
            lane: self.lane,
            occupant: self.occupant,
            resume,
        }
    }

    pub(in crate::resident) fn reporter(&self) -> Arc<dyn CaptureReporter> {
        Arc::new(SilentReporter)
    }

    pub(in crate::resident) fn assert_one_configured_probe_from_launch_worktree(&self) {
        assert_eq!(
            fs::read_to_string(&self.probe_log).expect("configured RTK was probed"),
            format!("{}|--version\n", self.root.display())
        );
    }

    pub(in crate::resident) fn assert_occupant_started_without_rtk(&self) {
        assert_eq!(
            fs::read_to_string(&self.occupant_environment)
                .expect("occupant published its environment verdict"),
            "absent\n"
        );
    }
}

impl Drop for InactiveRtkFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("inactive RTK fixture removes");
    }
}

pub(in crate::resident) fn assert_obsolete_health(health: &wire::body::RtkHealth) {
    assert_eq!(health.state(), wire::body::RtkHealthState::Obsolete);
    assert_eq!(health.version(), Some("0.44.9"));
    assert_eq!(health.detail(), "RTK 0.44.9 is older than required 0.45.0");
    assert!(health.detail().chars().count() <= 1024);
}

fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .status()
        .expect("fixture Git command starts");
    assert!(
        status.success(),
        "fixture Git command failed: {arguments:?}"
    );
}

fn executable(path: &Path) {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("fixture executable mode installs");
    #[cfg(not(unix))]
    let _ = path;
}

fn uuid(value: u64) -> CanonicalUuid {
    format!("018f82d4-6f59-7d06-92e3-{value:012x}")
        .parse()
        .expect("fixture UUID")
}

struct SilentReporter;

impl CaptureReporter for SilentReporter {
    fn record(&self, _record: CaptureRecord) -> Result<(), CaptureReportError> {
        Ok(())
    }

    fn failed(&self, _failure: CaptureFailure) {}
}
