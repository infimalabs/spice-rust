use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use super::super::files::{prepare_retirement, retire_path, withdraw_parent};
use super::super::handle::retirement_temporary_name;
use super::super::progress::{apply_prepared_record, fresh};
use super::super::record::read_record;
use super::super::{
    BINARY_PATH, EXECUTABLE_MODE, InstallReceipt, RECORD_PATH, RETIRED_XTASK_PATH, SKILL_PATH,
    build_plan, verify_progress,
};
use super::*;

/// One scratch repository, removed again when the test that made it ends.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository(label: &str) -> (Scratch, Repository) {
    let root = std::env::temp_dir().join(format!(
        "spice-install-crossing-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&root).expect("scratch root creates");
    crate::test_repository::initialize(&root);
    let repository = Repository::discover(&root).expect("scratch repository discovers");
    (Scratch(root), repository)
}

fn generation(label: &str) -> Digest {
    Digest::compute(Domain::DistArtifact, label.as_bytes())
}

fn image(bytes: &[u8]) -> GeneratedImage {
    GeneratedImage::new(DATA_MODE, bytes)
}

fn generation_file(generation: &Digest, file: &str) -> String {
    format!("{GENERATIONS}/{generation}/{file}")
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture file has a parent"))
        .expect("fixture parent creates");
    fs::write(path, bytes).expect("fixture bytes write");
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(DATA_MODE)).expect("fixture mode writes");
}

/// The sidecar files schema 18 published for `current`, retaining `older`.
fn sidecar_files() -> Vec<(SidecarAsset, String, Vec<u8>)> {
    let current = generation("current");
    let older = generation("older");
    vec![
        (
            SidecarAsset::AppHtml,
            generation_file(&older, "app.html"),
            b"older html".to_vec(),
        ),
        (
            SidecarAsset::AppHtml,
            generation_file(&current, "app.html"),
            b"current html".to_vec(),
        ),
        (
            SidecarAsset::AppModule,
            generation_file(&current, "app.module"),
            b"current module".to_vec(),
        ),
        (
            SidecarAsset::AppCurrent,
            POINTER.to_owned(),
            format!("{current}\n").into_bytes(),
        ),
    ]
}

fn seed(repository: &Repository) {
    for (_, path, bytes) in sidecar_files() {
        write(&repository.common_dir().join(path), &bytes);
    }
}

fn retained_operation(
    repository: &Repository,
    asset: RetainedAsset,
    path: &str,
    generated: GeneratedImage,
) -> RetainedOperation {
    let bound = bind_destination(repository.common_dir(), path).expect("fixture path binds");
    RetainedOperation {
        asset,
        location: Location::CommonGit,
        path: path.to_owned(),
        destination: bound.destination,
        parent: bound.parent,
        previous: None,
        generated,
        superseded: None,
        introduced_parents: Vec::new(),
    }
}

fn reseal(record: &mut RetainedRecord) {
    let plan = wire::encode(&record.plan).expect("fixture plan encodes");
    record.plan_digest = Digest::compute(Domain::InstallPlan, &plan);
}

/// A complete schema-18 receipt: one fixed effect with an operator reversal
/// image, the app sidecars, its executable, one fixed retirement, and one
/// generation discard schema 18 already completed.
fn retained(repository: &Repository) -> RetainedRecord {
    let mut skill = retained_operation(
        repository,
        RetainedAsset::Current(Asset::Skill),
        SKILL_PATH,
        image(b"skill"),
    );
    skill.previous = Some(image(b"operator skill"));
    let mut operations = vec![skill];
    operations.extend(sidecar_files().into_iter().map(|(asset, path, bytes)| {
        retained_operation(
            repository,
            RetainedAsset::Sidecar(asset),
            &path,
            image(&bytes),
        )
    }));
    operations.push(retained_operation(
        repository,
        RetainedAsset::Current(Asset::Binary),
        BINARY_PATH,
        GeneratedImage::new(EXECUTABLE_MODE, b"executable"),
    ));
    let retirement = |path: &str, discard: bool| {
        let bound =
            bind_destination(repository.common_dir(), path).expect("fixture retirement binds");
        Retirement {
            path: path.to_owned(),
            destination: bound.destination,
            parent: bound.parent,
            previous: None,
            observed: None,
            superseded: discard.then(|| image(b"retired html")),
            discard,
        }
    };
    let retirements = vec![
        retirement(RETIRED_XTASK_PATH, false),
        retirement(&generation_file(&generation("retired"), "app.html"), true),
    ];
    let identity = RepositoryIdentity::capture(repository);
    let plan = RetainedPlan {
        version: SCHEMA,
        repository: identity.clone(),
        record: bind_destination(repository.common_dir(), RECORD_PATH).expect("record binds"),
        operations,
        app: Some(generation("current")),
        app_predecessors: vec![generation("older")],
        retirements,
        hook_activation: None,
        symbols: None,
    };
    let installed = Generation::observe(
        &fs::metadata(repository.common_dir()).expect("common directory metadata reads"),
    );
    let (operations, retirements) = (plan.operations.len(), plan.retirements.len());
    let mut record = RetainedRecord {
        version: SCHEMA,
        repository: identity,
        plan_digest: generation("unsealed"),
        destinations: plan
            .operations
            .iter()
            .map(|operation| (operation.location, operation.destination.clone()))
            .collect(),
        status: InstallStatus::Complete,
        completed: operations + retirements,
        installed: vec![Some(installed); operations],
        installed_parents: vec![Some(installed); operations],
        restored: vec![None; operations],
        retired_parents: vec![None; retirements],
        restored_retirements: vec![None; retirements],
        plan,
    };
    reseal(&mut record);
    record
}

fn cross_bytes(bytes: &[u8]) -> Result<Crossed, InstallError> {
    cross(Path::new(RECORD_PATH), bytes)
}

fn cross_record(record: &RetainedRecord) -> Result<Crossed, InstallError> {
    cross_bytes(&wire::encode(record).expect("fixture record encodes"))
}

#[test]
fn a_complete_schema_18_receipt_crosses_to_its_kept_effects_and_ordered_sidecars() {
    let (_scratch, repository) = repository("cross");
    let retained = retained(&repository);
    let crossed = cross_record(&retained).expect("an exact complete receipt crosses");

    let record = &crossed.record;
    assert_eq!(record.plan.version, PLAN_VERSION);
    assert_eq!(record.status, InstallStatus::Complete);
    assert_eq!(record.completed, record.plan.operation_count());
    assert_eq!(
        record
            .plan
            .operations
            .iter()
            .map(|operation| operation.asset)
            .collect::<Vec<_>>(),
        [Asset::Skill, Asset::Binary]
    );
    assert_eq!(
        record.plan.operations[0].previous,
        Some(image(b"operator skill"))
    );
    assert_eq!(
        record
            .plan
            .retirements
            .iter()
            .map(|retirement| retirement.path.as_str())
            .collect::<Vec<_>>(),
        [RETIRED_XTASK_PATH]
    );
    assert_eq!(record.plan.app, Some(generation("current")));
    assert_eq!(crossed.original_digest, retained.plan_digest);

    let mut files = sidecar_files()
        .into_iter()
        .map(|(_, path, _)| path)
        .filter(|path| path != POINTER)
        .collect::<Vec<_>>();
    files.sort();
    let expected = std::iter::once(POINTER.to_owned())
        .chain(files)
        .collect::<Vec<_>>();
    assert_eq!(
        crossed
            .sidecars
            .iter()
            .map(|sidecar| sidecar.path.clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn only_an_exact_complete_schema_18_receipt_crosses() {
    let (_scratch, repository) = repository("refuse");
    let cases: [(&str, fn(&mut RetainedRecord)); 4] = [
        ("interrupted", |record| {
            record.status = InstallStatus::Applying;
        }),
        ("digest", |record| record.plan.app_predecessors.clear()),
        ("ambiguous pointer", |record| {
            record.plan.app = Some(generation("older"));
            record.plan.app_predecessors = vec![generation("current")];
            reseal(record);
        }),
        ("unpublished generation", |record| {
            record.plan.app_predecessors.clear();
            reseal(record);
        }),
    ];
    for (case, mutate) in cases {
        let mut record = retained(&repository);
        mutate(&mut record);
        assert!(
            matches!(
                cross_record(&record),
                Err(InstallError::RetainedRecord { owner: OWNER, .. })
            ),
            "{case}"
        );
    }
    let mut bytes = wire::encode(&retained(&repository)).expect("fixture record encodes");
    bytes.push(b'\n');
    assert!(matches!(
        cross_bytes(&bytes),
        Err(InstallError::RetainedRecord { owner: OWNER, .. })
    ));
}

/// Apply a binary-less current plan ending in the sidecar discards, exactly
/// as the refresh of a crossed receipt orders them after its live assets.
fn apply_discards(repository: &Repository) -> Result<InstallReceipt, InstallError> {
    let sidecars = sidecars(&retained(repository).plan).expect("fixture evidence admits");
    let mut plan = build_plan(repository, None)?;
    plan.retirements.extend(retirements(repository, &sidecars)?);
    apply_prepared_record(repository, None, fresh(plan, None)?)
}

#[test]
fn sidecar_discards_follow_the_receipt_and_withdraw_only_owned_empty_directories() {
    let (_clean_scratch, clean) = repository("withdraw");
    let (_scratch, repository) = repository("apply");
    let common = repository.common_dir();
    seed(&repository);

    let drifted = common.join(generation_file(&generation("current"), "app.html"));
    write(&drifted, b"operator html");
    assert!(matches!(
        apply_discards(&repository),
        Err(InstallError::Drift { .. })
    ));
    assert!(!common.join(RECORD_PATH).exists());
    assert!(common.join(POINTER).exists());
    write(&drifted, b"current html");

    let older = common.join(GENERATIONS).join(generation("older").as_str());
    let foreign = older.join("operator.txt");
    write(&foreign, b"operator");
    let receipt = apply_discards(&repository).expect("sidecar discards apply");
    assert_eq!(receipt.status(), InstallStatus::Complete);
    for (_, path, _) in sidecar_files() {
        assert!(!common.join(&path).exists(), "{path}");
    }
    assert!(
        !common
            .join(GENERATIONS)
            .join(generation("current").as_str())
            .exists()
    );
    assert_eq!(fs::read(&foreign).expect("foreign file reads"), b"operator");
    let loaded = read_record(&repository)
        .expect("receipt reads")
        .expect("receipt exists");
    verify_progress(&repository, &loaded.record).expect("completed discards verify");

    seed(&clean);
    apply_discards(&clean).expect("sidecar discards apply");
    assert!(!clean.common_dir().join(SIDECAR_ROOT).exists());
    assert!(clean.common_dir().join(ANCHOR).exists());
}

fn discards(repository: &Repository, files: &[(&str, &[u8])]) -> Vec<Retirement> {
    let generation = generation("current");
    let sidecars = files
        .iter()
        .map(|(file, bytes)| {
            let path = generation_file(&generation, file);
            write(&repository.common_dir().join(&path), bytes);
            Sidecar {
                path,
                image: image(bytes),
            }
        })
        .collect::<Vec<_>>();
    retirements(repository, &sidecars).expect("sidecar discards bind")
}

fn temporary(repository: &Repository, retirement: &Retirement, index: usize) -> PathBuf {
    retirement
        .destination(repository)
        .with_file_name(retirement_temporary_name(index))
}

#[test]
fn an_interrupted_sidecar_discard_resumes_and_withdraws_its_whole_directory_chain() {
    let (_scratch, repository) = repository("resume");
    let [first, second] = <[Retirement; 2]>::try_from(discards(
        &repository,
        &[("app.html", b"html"), ("app.module", b"module")],
    ))
    .expect("two discards");
    for (index, retirement) in [&first, &second].into_iter().enumerate() {
        prepare_retirement(&repository, retirement, index).expect("discard prepares");
    }

    // Interrupted after the file moved to its fenced temporary name.
    fs::rename(
        first.destination(&repository),
        temporary(&repository, &first, 0),
    )
    .expect("fixture stages the discard");
    retire_path(&repository, &first, 0).expect("resume finishes the staged discard");
    assert!(!temporary(&repository, &first, 0).exists());
    assert!(!first.destination(&repository).exists());
    retire_path(&repository, &first, 0).expect("a finished discard is a no-op");
    retire_path(&repository, &second, 1).expect("the next discard retires in place");
    assert!(second.withdraws_parent(None));

    // Interrupted after withdrawing the generation but before its parents.
    fs::remove_dir(
        repository
            .common_dir()
            .join(GENERATIONS)
            .join(generation("current").as_str()),
    )
    .expect("fixture withdraws the generation");
    withdraw_parent(&repository, &second).expect("withdrawal resumes at the surviving prefix");
    assert!(!repository.common_dir().join(SIDECAR_ROOT).exists());
    assert!(repository.common_dir().join(ANCHOR).exists());
    withdraw_parent(&repository, &second).expect("a withdrawn chain stays withdrawn");
}

#[test]
fn a_drifted_or_colliding_sidecar_discard_refuses_without_moving_any_file() {
    let (_scratch, repository) = repository("drift");
    let [retirement] =
        <[Retirement; 1]>::try_from(discards(&repository, &[("app.html", b"app.html")]))
            .expect("one discard");
    let installed = retirement.destination(&repository);
    let staged = temporary(&repository, &retirement, 0);

    write(&installed, b"operator bytes");
    assert!(matches!(
        prepare_retirement(&repository, &retirement, 0),
        Err(InstallError::Drift { .. })
    ));
    assert!(matches!(
        retire_path(&repository, &retirement, 0),
        Err(InstallError::Drift { .. })
    ));
    assert_eq!(fs::read(&installed).unwrap(), b"operator bytes");
    assert!(!staged.exists());

    write(&installed, b"app.html");
    write(&staged, b"app.html");
    assert!(matches!(
        prepare_retirement(&repository, &retirement, 0),
        Err(InstallError::ParentChanged(_))
    ));
    assert!(matches!(
        retire_path(&repository, &retirement, 0),
        Err(InstallError::ParentChanged(_))
    ));
    assert!(installed.exists() && staged.exists());

    fs::remove_file(&installed).unwrap();
    write(&staged, b"operator bytes");
    assert!(matches!(
        retire_path(&repository, &retirement, 0),
        Err(InstallError::Drift { .. })
    ));
    assert_eq!(fs::read(&staged).unwrap(), b"operator bytes");
}
