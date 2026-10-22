use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::test_support::{InertProbes, LaunchFixture};
use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn occupant_deadline_uses_runtime_instead_of_the_claim_lease() {
    let fixture = LaunchFixture::new();
    fs::write(
        fixture.root.join("spice.toml"),
        "[lane]\nlease = \"1s\"\nruntime = \"5s\"\n",
    )
    .expect("independent lease and runtime policy writes");

    let launch = occupant_launch(
        &fixture.paths,
        fixture.request(vendor::Resume::Fresh),
        fixture.reporter(),
        &InertProbes,
    )
    .expect("occupant launch composes");
    assert_eq!(launch.spec.deadlines.runtime(), Duration::from_secs(5));
}

#[test]
fn absent_cargo_lock_launches_without_rust_environment_or_tool_probe() {
    let fixture = LaunchFixture::new();
    fs::remove_file(fixture.root.join("Cargo.lock")).expect("remove fixture Cargo lock");
    let repository = Repository::discover(&fixture.root).expect("fixture repository discovers");

    let (environment, _) = launch_environment(
        &repository,
        &fixture.root.join("omp"),
        None,
        "rtk",
        fixture.lane,
        &InertProbes,
    )
    .expect("non-Rust launch environment composes");
    for name in [
        spice::cargo_isolation::CARGO_HOME_VARIABLE,
        spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
        spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
        spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
        spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
        spice::cargo_isolation::RUSTC_VARIABLE,
        spice::cargo_isolation::RUSTDOC_VARIABLE,
    ] {
        assert!(
            environment.entries().iter().all(|(key, _)| key != name),
            "non-Rust launch retained {name}"
        );
    }
    for name in [
        "SHELL",
        steer::hook::BASH_ENV,
        steer::hook::ZDOTDIR,
        steer::hook::HOOKS,
        steer::hook::NATIVE_SHELL,
        steer::hook::EXECUTABLE,
        spice::rtk::EXECUTABLE_VARIABLE,
        spice::rtk::HISTORY_VARIABLE,
    ] {
        assert!(
            environment.entries().iter().any(|(key, _)| key == name),
            "non-Rust launch omitted {name}"
        );
    }
    for name in ADMITTED_AMBIENT {
        let carried = environment
            .entries()
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone());
        assert_eq!(
            carried,
            crate::bootstrap_environment(name),
            "launch must carry {name} exactly when the resident has it"
        );
    }
}

#[test]
fn occupant_inherits_only_admitted_ambient_including_the_ssh_agent_socket() {
    let ambient = |key: &str| Some(OsString::from(format!("ambient-{key}")));
    let admitted = admitted_ambient(ambient);
    let names = admitted
        .iter()
        .map(|(key, value)| {
            assert_eq!(
                Some(value.clone()),
                ambient(key.to_str().expect("UTF-8 name"))
            );
            key.to_str().expect("UTF-8 name")
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["HOME", "TMPDIR", "USER", "SSH_AUTH_SOCK"]);

    let without_agent =
        admitted_ambient(|key| (key != "SSH_AUTH_SOCK").then(|| ambient(key)).flatten());
    assert!(without_agent.iter().all(|(key, _)| key != "SSH_AUTH_SOCK"));
    assert_eq!(without_agent.len(), 3);
}

#[test]
fn zero_config_repository_without_a_lock_composes_default_omp_and_refuses_other_vendors() {
    let fixture = LaunchFixture::new();
    for path in ["Cargo.lock", "Cargo.toml", "spice.toml"] {
        fs::remove_file(fixture.root.join(path)).expect("remove Rust fixture input");
    }
    let worktree = fixture.root.to_str().expect("fixture worktree is UTF-8");
    let omp: wire::Name = "omp".parse().expect("default vendor name");
    let other: wire::Name = "other".parse().expect("unsupported vendor name");
    let request = |vendor| OccupantLaunchRequest {
        worktree,
        vendor,
        epoch: fixture.epoch,
        lane: fixture.lane,
        occupant: fixture.occupant,
        resume: vendor::Resume::Fresh,
        fast_mode: false,
        model: None,
        effort: None,
    };

    occupant_launch(
        &fixture.paths,
        request(&omp),
        fixture.reporter(),
        &InertProbes,
    )
    .expect("zero-config non-Rust repository composes default omp");

    let Err(Outcome::Refusal(refusal)) = occupant_launch(
        &fixture.paths,
        request(&other),
        fixture.reporter(),
        &InertProbes,
    ) else {
        panic!("a lane recorded under an unsupported vendor must refuse to wake");
    };
    let diagnostic = refusal.diagnostic().expect("the refusal is diagnosed");
    assert_eq!(diagnostic.reason(), "lane::vendor-unsupported");
    assert_eq!(diagnostic.expected(), Some("omp"));
    assert_eq!(diagnostic.observed(), Some("other"));
}

#[test]
fn a_worktree_in_another_repository_refuses_the_lane_instead_of_faulting_the_resident() {
    let resident = LaunchFixture::new();
    let foreign = LaunchFixture::new();

    let Err(Outcome::Refusal(refusal)) = occupant_launch(
        &resident.paths,
        foreign.request(vendor::Resume::Fresh),
        resident.reporter(),
        &InertProbes,
    ) else {
        panic!("a copied journal naming another repository's worktree must park only that lane");
    };
    let diagnostic = refusal.diagnostic().expect("the refusal is diagnosed");
    assert_eq!(diagnostic.reason(), "lane::foreign-worktree");
    assert_eq!(
        diagnostic.observed(),
        Some(
            foreign
                .paths
                .common
                .to_str()
                .expect("fixture path is UTF-8")
        )
    );
}

#[test]
fn regular_cargo_lock_retains_the_complete_isolated_rust_environment() {
    let fixture = LaunchFixture::new();
    let repository = Repository::discover(&fixture.root).expect("fixture repository discovers");
    let environment = cargo_environment(&repository, fixture.lane)
        .expect("regular Cargo lock prepares isolation");
    let names = environment
        .iter()
        .map(|(name, _)| name.to_str().expect("Rust variable name is UTF-8"))
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        [
            spice::cargo_isolation::CARGO_HOME_VARIABLE,
            spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE,
            spice::cargo_isolation::NATIVE_CARGO_VARIABLE,
            spice::cargo_isolation::NATIVE_RUSTC_VARIABLE,
            spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE,
            spice::cargo_isolation::RUSTC_VARIABLE,
            spice::cargo_isolation::RUSTDOC_VARIABLE,
        ]
    );
}

#[test]
fn non_regular_cargo_lock_refuses_before_rust_tool_probe() {
    let fixture = LaunchFixture::new();
    let lock = fixture.root.join("Cargo.lock");
    fs::remove_file(&lock).expect("remove fixture Cargo lock");
    fs::create_dir(&lock).expect("replace Cargo lock with directory");
    let repository = Repository::discover(&fixture.root).expect("fixture repository discovers");

    let error =
        cargo_environment(&repository, fixture.lane).expect_err("non-regular Cargo lock refuses");
    assert!(matches!(
        error,
        Outcome::Fault(ref fault)
            if fault.reason == "lane::cargo-isolation-unavailable"
    ));
}

#[test]
fn launch_skill_comes_only_from_initialized_git_common_state() {
    let root = std::env::temp_dir().join(format!(
        "spice-launch-skill-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir_all(root.join(".git")).expect("fixture repository creates");
    let repository = Repository::discover(&root).expect("fixture repository discovers");
    let accidental = root.join(".agents/skills/spice/SKILL.md");
    fs::create_dir_all(accidental.parent().expect("accidental skill parent"))
        .expect("accidental worktree skill parent creates");
    fs::write(&accidental, b"wrong worktree skill\n").expect("accidental worktree skill writes");

    let missing = required_skill(&repository).expect_err("worktree copy cannot satisfy launch");
    let Outcome::Fault(missing) = missing else {
        panic!("missing common skill must fault")
    };
    assert_eq!(missing.reason, "lane::skill-unavailable");
    let common = spice::install::installed_skill(&repository);
    assert!(missing.detail.contains(&common.display().to_string()));

    fs::create_dir_all(&common).expect("invalid common skill directory creates");
    let invalid = required_skill(&repository).expect_err("skill directory cannot satisfy launch");
    assert!(matches!(
        invalid,
        Outcome::Fault(ref fault)
            if fault.reason == "lane::skill-unavailable"
    ));
    fs::remove_dir(&common).expect("invalid common skill directory removes");
    fs::write(&common, b"stale common skill\n").expect("stale common skill writes");
    assert!(matches!(
        required_skill(&repository),
        Err(Outcome::Fault(ref fault))
            if fault.reason == "lane::skill-unavailable"
    ));
    fs::write(&common, ::std::include_bytes!("../../../../agent/SKILL.md"))
        .expect("current common skill writes");

    assert_eq!(
        required_skill(&repository).expect("common regular skill admits"),
        common
    );
    assert_eq!(
        fs::read_to_string(&accidental).expect("accidental skill remains"),
        "wrong worktree skill\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        fs::remove_file(&common).expect("common skill removes");
        symlink(&accidental, &common).expect("redirected common skill creates");
        assert!(matches!(
            required_skill(&repository),
            Err(Outcome::Fault(ref fault))
                if fault.reason == "lane::skill-unavailable"
        ));
        fs::remove_file(&common).expect("redirected common skill removes");
        fs::remove_dir(common.parent().expect("common skill parent"))
            .expect("common skill parent removes");
        fs::write(
            &accidental,
            ::std::include_bytes!("../../../../agent/SKILL.md"),
        )
        .expect("redirect target carries current skill");
        symlink(
            accidental.parent().expect("accidental skill parent"),
            common.parent().expect("common skill parent"),
        )
        .expect("redirected common skill parent creates");
        assert!(matches!(
            required_skill(&repository),
            Err(Outcome::Fault(ref fault))
                if fault.reason == "lane::skill-unavailable"
        ));
    }
    fs::remove_dir_all(&root).expect("fixture removes");
}

#[test]
fn the_session_stamp_is_omp_s_millisecond_iso_spelling() {
    let now: jiff::Timestamp = "2026-09-16T06:43:21.805123Z".parse().expect("an instant");
    assert_eq!(session_stamp(now), "2026-09-16T06:43:21.805Z");
    let whole: jiff::Timestamp = "2026-09-16T06:43:21Z".parse().expect("an instant");
    assert_eq!(session_stamp(whole), "2026-09-16T06:43:21.000Z");
}

/// Probes for a launch that must not read the lane's session authority.
struct UnreadAuthority;

impl LaunchProbes for UnreadAuthority {
    fn advance(&self, repository: &Repository) -> String {
        InertProbes.advance(repository)
    }

    fn shadow(&self, repository: &Repository, base: Environment) -> CommandResult<Environment> {
        InertProbes.shadow(repository, base)
    }

    fn rtk(
        &self,
        executable: &std::ffi::OsStr,
        working: &Path,
        environment: &Environment,
    ) -> wire::body::RtkHealth {
        InertProbes.rtk(executable, working, environment)
    }

    fn session(&self, _worktree: PathBuf) -> crate::session_command::Environment {
        panic!("a resumed launch plan read the lane's authority")
    }
}

#[test]
fn a_resumed_launch_defers_its_authority_to_delivery() {
    let fixture = LaunchFixture::new();
    let resume = vendor::Resume::Session(
        vendor::SessionId::new(super::test_support::SESSION.to_owned()).expect("fixture identity"),
    );
    let launch = occupant_launch(
        &fixture.paths,
        fixture.request(resume),
        fixture.reporter(),
        &UnreadAuthority,
    )
    .expect("resumed launch composes without reading authority");
    let Some(crate::resident::supervisor::command::FirstTurn::Resumed(_)) = launch.first_prompt
    else {
        panic!("a resumed launch's first turn is not marked resumed");
    };

    let launch = occupant_launch(
        &fixture.paths,
        fixture.request(vendor::Resume::Fresh),
        fixture.reporter(),
        &InertProbes,
    )
    .expect("fresh launch composes");
    let Some(crate::resident::supervisor::command::FirstTurn::Fresh(_)) = launch.first_prompt
    else {
        panic!("a fresh launch's first turn is not marked fresh");
    };
}
