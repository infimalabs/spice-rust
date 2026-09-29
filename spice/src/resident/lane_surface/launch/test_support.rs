use std::cell::OnceCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use lane::capture::{CaptureFailure, CaptureRecord, CaptureReportError, CaptureReporter};

use super::super::super::read_materializations::CurrentReads;
use super::*;

pub(in crate::resident) const SESSION: &str = "018f7777-7777-7444-8888-999999999999";
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub(in crate::resident) struct AdvisoryRtkFixture {
    pub(in crate::resident) root: PathBuf,
    pub(in crate::resident) paths: RuntimePaths,
    pub(in crate::resident) epoch: CanonicalUuid,
    pub(in crate::resident) lane: CanonicalUuid,
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) capability: CanonicalUuid,
    pub(in crate::resident) vendor: wire::Name,
    reads: OnceCell<CurrentReads>,
}

impl AdvisoryRtkFixture {
    pub(in crate::resident) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "spice-resident-advisory-rtk-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory creates");
        let root = fs::canonicalize(root).expect("fixture root canonicalizes");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"resident-advisory-rtk\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest writes");
        fs::write(
            root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"resident-advisory-rtk\"\nversion = \"0.0.0\"\n",
        )
        .expect("fixture lockfile writes");
        fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").expect("fixture source writes");
        fs::create_dir(root.join(".git")).expect("fixture Git directory creates");

        let rtk = root.join("advisory-rtk");
        fs::write(
            &rtk,
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "if [ \"$#\" -eq 1 ] && [ \"$1\" = --version ]; then\n",
                "  printf '\\377'\n",
                "  exit 0\n",
                "fi\n",
                "exit 91\n",
            ),
        )
        .expect("fixture RTK writes");
        executable(&rtk);

        let vendor = root.join("fake-omp");
        write_vendor(&vendor);
        fs::write(
            root.join("spice-rust.toml"),
            format!(
                concat!(
                    "[lane]\nlease = \"20s\"\n\n",
                    "[vendor.omp]\nbinary = {:?}\n\n",
                    "[rtk]\nexecutable = {:?}\n",
                ),
                vendor.display().to_string(),
                rtk.display().to_string(),
            ),
        )
        .expect("fixture launch configuration writes");
        let repository = Repository::discover(&root).expect("fixture repository discovers");
        let skill = spice::install::installed_skill(&repository);
        fs::create_dir_all(skill.parent().expect("installed skill parent"))
            .expect("installed skill parent creates");
        fs::write(&skill, ::std::include_bytes!("../../../../agent/SKILL.md"))
            .expect("current installed skill writes");
        let paths = RuntimePaths::from_repository(&repository).expect("runtime paths resolve");
        Self {
            root,
            paths,
            epoch: uuid(1),
            lane: uuid(2),
            occupant: uuid(3),
            capability: uuid(4),
            vendor: "omp".parse().expect("fixture vendor name"),
            reads: OnceCell::new(),
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
            fast_mode: false,
            model: None,
            effort: None,
        }
    }

    pub(in crate::resident) fn reporter(&self) -> Arc<dyn CaptureReporter> {
        Arc::new(SilentReporter)
    }

    /// The lane runtime over `bus`, reading consent under the queue posture.
    ///
    /// The reads start once and live as long as the fixture.
    pub(in crate::resident) fn lane_runtime<'a>(
        &'a self,
        bus: &Bus,
        capabilities: &'a canvas::axum::Capabilities,
        discovery: &'a canvas::Discovery,
    ) -> super::super::Runtime<'a> {
        let reads = self
            .reads
            .get_or_init(|| CurrentReads::start_queueing(bus, &self.paths));
        super::super::Runtime::new(capabilities, discovery, self.reporter(), reads)
    }
}

impl Drop for AdvisoryRtkFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("advisory RTK fixture removes");
    }
}

fn executable(path: &Path) {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("fixture executable mode installs");
    #[cfg(not(unix))]
    let _ = path;
}

fn write_vendor(path: &Path) {
    fs::write(
        path,
        format!(
            concat!(
                "#!/bin/sh\n",
                "set -eu\n",
                "session={}\n",
                "while [ \"$#\" -gt 1 ]; do\n",
                "  if [ \"$1\" = --resume ]; then session=$2; fi\n",
                "  shift\n",
                "done\n",
                "printf '%s\\n' '{{\"type\":\"ready\",\"protocolVersion\":1}}'\n",
                "printf '{{\"id\":\"spice-state\",\"type\":\"response\",\"command\":\"get_state\",",
                "\"success\":true,\"data\":{{\"sessionId\":\"%s\"}}}}\\n' \"$session\"\n",
            ),
            SESSION,
        ),
    )
    .expect("fixture vendor writes");
    executable(path);
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
