use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::test_support::AdvisoryRtkFixture;
use super::*;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn occupant_deadline_uses_runtime_instead_of_the_claim_lease() {
    let fixture = AdvisoryRtkFixture::new();
    let vendor = fixture.root.join("fake-claude");
    fs::write(
        fixture.root.join("spice-rust.toml"),
        format!(
            concat!(
                "[lane]\nlease = \"1s\"\nruntime = \"5s\"\n\n",
                "[vendor.claude]\nbinary = {:?}\n\n",
                "[rtk]\nexecutable = {:?}\n",
            ),
            vendor.display().to_string(),
            fixture.root.join("advisory-rtk").display().to_string(),
        ),
    )
    .expect("independent lease and runtime policy writes");

    let launch = occupant_launch(
        &fixture.paths,
        fixture.request(vendor::Resume::Fresh),
        fixture.reporter(),
    )
    .expect("occupant launch composes");
    assert_eq!(launch.spec.deadlines.runtime(), Duration::from_secs(5));
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
                && fault.detail.contains("not a regular file")
    ));
    fs::remove_dir(&common).expect("invalid common skill directory removes");
    fs::write(&common, b"stale common skill\n").expect("stale common skill writes");
    assert!(matches!(
        required_skill(&repository),
        Err(Outcome::Fault(ref fault))
            if fault.reason == "lane::skill-unavailable"
                && fault.detail.contains("does not match this appliance")
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
                    && fault.detail.contains("not a regular file")
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
                    && fault.detail.contains("outside Git-common authority")
        ));
    }
    fs::remove_dir_all(&root).expect("fixture removes");
}
