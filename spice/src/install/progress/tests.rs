use std::collections::BTreeSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::super::{RECORD_PATH, SKILL_PATH};
use super::*;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository(label: &str) -> (Scratch, Repository) {
    let root = std::env::temp_dir().join(format!(
        "spice-install-progress-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&root).expect("scratch root creates");
    crate::test_repository::initialize(&root);
    let repository = Repository::discover(&root).expect("scratch repository discovers");
    (Scratch(root), repository)
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().expect("fixture file has a parent"))
        .expect("fixture parent creates");
    fs::write(path, bytes).expect("fixture bytes write");
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("fixture mode writes");
    #[cfg(windows)]
    sill::authority::set_owner_mode(path, mode).expect("fixture mode writes");
}

fn mode_of(path: &Path) -> u32 {
    let file = fs::File::open(path).expect("fixture reopens");
    sill::authority::file_mode(&file).expect("fixture mode reads") & 0o777
}

fn parent_reversals(plan: &InstallPlan) -> Vec<(Location, String)> {
    plan.operations
        .iter()
        .flat_map(|operation| {
            operation
                .introduced_parents
                .iter()
                .cloned()
                .map(|parent| (operation.location, parent))
        })
        .chain(plan.retirements.iter().flat_map(|retirement| {
            retirement
                .introduced_parents
                .iter()
                .cloned()
                .map(|parent| (Location::CommonGit, parent))
        }))
        .collect()
}

fn install_without_binary(repository: &Repository) -> Vec<(Location, String)> {
    let prepared = prepare(repository, None).expect("install prepares");
    let reversals = parent_reversals(prepared.plan());
    apply_prepared_record(repository, None, prepared).expect("install applies");
    reversals
}

fn refresh_without_binary(repository: &Repository) -> Vec<(Location, String)> {
    let LoadedRecord { record, generation } = read_record(repository)
        .expect("receipt reads")
        .expect("receipt exists");
    let plan = refresh_plan(repository, None, &record).expect("refresh plan carries ownership");
    let reversals = parent_reversals(&plan);
    let prepared = fresh(plan, Some(generation)).expect("refresh prepares");
    apply_prepared_record(repository, None, prepared).expect("refresh applies");
    reversals
}

#[test]
fn refresh_keeps_each_initial_parent_reversal_until_unapply() {
    let (_scratch, repository) = repository("refresh-parent-ownership");
    let initial = install_without_binary(&repository);
    assert!(!initial.is_empty());
    assert_eq!(
        initial.len(),
        initial.iter().collect::<BTreeSet<_>>().len(),
        "initial install assigns every introduced directory once"
    );

    assert_eq!(refresh_without_binary(&repository), initial);
    assert_eq!(refresh_without_binary(&repository), initial);
    unapply_record(&repository).expect("refreshed install unapplies");

    for (location, relative) in initial {
        assert!(
            !location.root(&repository).join(&relative).exists(),
            "empty introduced directory survived unapply: {relative}"
        );
    }
}

/// `.spice` is the only state root name: a directory by any other name is
/// foreign state an install neither reads, moves, nor deletes.
#[test]
fn install_creates_the_state_root_and_leaves_a_differently_named_root_untouched() {
    let (_scratch, repository) = repository("foreign-root");
    let foreign = repository.common_dir().join(".spice-rs");
    let journal = foreign.join("data/spicejournal.sqlite3");
    let note = foreign.join("notes/note.md");
    write_mode(&journal, b"foreign journal", 0o600);
    write_mode(&note, b"foreign note", 0o600);

    install_without_binary(&repository);

    assert_eq!(
        repository.state_root(),
        repository.common_dir().join(".spice")
    );
    assert!(repository.state_root().join(RECORD_PATH).is_file());
    assert_eq!(
        fs::read(&journal).expect("foreign journal remains"),
        b"foreign journal"
    );
    assert_eq!(
        fs::read(&note).expect("foreign note remains"),
        b"foreign note"
    );
    let mut children = fs::read_dir(&foreign)
        .expect("foreign root remains")
        .map(|entry| entry.expect("foreign child reads").file_name())
        .collect::<Vec<_>>();
    children.sort();
    assert_eq!(children, ["data", "notes"]);
}

#[test]
fn unapply_retains_preexisting_directories_and_operator_content_after_refresh() {
    let (_scratch, repository) = repository("refresh-parent-boundaries");
    let preexisting = repository.common_dir().join(".spice-install/hooks");
    fs::create_dir_all(&preexisting).expect("preexisting install directory creates");
    install_without_binary(&repository);
    refresh_without_binary(&repository);

    let operator_file = repository
        .common_dir()
        .join(".spice-install/agents/skills/operator.txt");
    fs::write(&operator_file, b"operator content").expect("operator content writes");
    unapply_record(&repository).expect("refreshed install unapplies");

    assert!(preexisting.is_dir());
    assert_eq!(
        fs::read(&operator_file).expect("operator content remains"),
        b"operator content"
    );
    assert!(
        !repository
            .common_dir()
            .join(".spice-install/git-hooks")
            .exists()
    );
}

#[test]
fn preserved_payloads_stay_out_of_checkpoints_and_survive_both_resume_frontiers() {
    let (_scratch, repository) = repository("reversal-payload");
    let target = repository.common_dir().join(SKILL_PATH);
    let prior = vec![0xA5; 2 * 1024 * 1024];
    // Windows restores only owner-only modes, so that is all it admits.
    let prior_mode = if cfg!(unix) { 0o640 } else { 0o600 };
    write_mode(&target, &prior, prior_mode);

    // Start a real apply and stop after the preserved target moved but
    // before its completion reached the receipt: the one admitted
    // ahead-of-record crash frontier.
    let prepared = prepare(&repository, None).expect("install prepares");
    let PreparedInstall {
        mut record,
        receipt,
        fresh,
    } = prepared;
    assert!(fresh);
    let generated = materialize_operations(&record, None, None).expect("operations materialize");
    record
        .plan
        .validate_pristine(&repository)
        .expect("pristine plan validates");
    require_receipt(&repository, &record.plan, receipt.as_ref()).expect("receipt remains absent");
    let mut generation =
        write_initial_record(&repository, &record).expect("initial receipt publishes");
    reversal::ensure_payloads(&repository, &record.plan).expect("reversal payloads publish");
    let skill = record
        .plan
        .operations
        .iter()
        .position(|operation| operation.asset == Asset::Skill)
        .expect("skill operation exists");
    let mut durable_completed = record.completed;
    while record.completed < skill {
        let index = record.completed;
        let (next, changed) = apply_operation(
            &repository,
            &mut record,
            &generated[index],
            index,
            generation,
            durable_completed,
        )
        .expect("operation applies");
        generation = next;
        record.completed += 1;
        if changed {
            generation =
                write_record(&repository, &record, generation).expect("progress publishes");
            durable_completed = record.completed;
        }
    }
    let (_, changed) = apply_operation(
        &repository,
        &mut record,
        &generated[skill],
        skill,
        generation,
        durable_completed,
    )
    .expect("frontier operation applies");
    assert!(changed);
    drop(record);

    let resumed = prepare(&repository, None).expect("interrupted apply reloads");
    apply_prepared_record(&repository, None, resumed).expect("interrupted apply completes");
    let record_length = fs::metadata(repository.state_root().join(RECORD_PATH))
        .expect("receipt metadata reads")
        .len();
    assert!(
        record_length < u64::try_from(prior.len()).unwrap(),
        "the receipt embedded the preserved payload"
    );

    // Enter unapply and stop after one real file-reversal checkpoint. The
    // next invocation must accept that durable prefix and finish from the
    // external payload store.
    let LoadedRecord {
        mut record,
        generation,
    } = read_record(&repository)
        .expect("receipt reads")
        .expect("receipt exists");
    verify_progress(&repository, &record).expect("completed install verifies");
    reversal::ensure_payloads(&repository, &record.plan).expect("payload store verifies");
    record.status = InstallStatus::Unapplying;
    record.completed = 0;
    record.restored.fill(None);
    record.restored_retirements.fill(None);
    let mut generation = write_record(&repository, &record, generation).expect("unapply starts");
    while let Some(next) = reverse_retirement(&repository, &mut record, generation.clone())
        .expect("retirement reverses")
    {
        generation = next;
    }
    reverse_file_operation(&repository, &mut record, generation)
        .expect("one file reversal publishes");
    drop(record);

    let receipt = unapply_record(&repository).expect("interrupted unapply completes");
    assert_eq!(receipt.status(), InstallStatus::Uninstalled);
    assert_eq!(fs::read(&target).expect("prior bytes read"), prior);
    assert_eq!(mode_of(&target), prior_mode);
}
