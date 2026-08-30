//! Durable handoff for allocator-owned resolution evidence.

use super::*;

const VERSION: u64 = 1;
const ROOT: &str = "task-completion";
const MANIFEST: &str = "operation.json";
const PROGRESS: &str = "progress.jsonl";
const TERMINAL: &str = "terminal.bin";
const TERMINAL_STAGED: &str = "terminal.staged";
const LOCK: &str = "worker.lock";

#[derive(Clone, Debug)]
pub(crate) struct Authority<'a> {
    pub(crate) operation: wire::CanonicalUuid,
    pub(crate) task: &'a str,
    pub(crate) actor: &'a wire::Actor,
    pub(crate) epoch: wire::CanonicalUuid,
    pub(crate) claim: wire::Seq,
    pub(crate) validation: &'a str,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u64,
    operation: wire::CanonicalUuid,
    task: String,
    actor: wire::Actor,
    epoch: wire::CanonicalUuid,
    claim: wire::Seq,
    validation: String,
    source: Digest,
}

impl Manifest {
    fn new(authority: &Authority<'_>, source: Digest) -> Self {
        Self {
            version: VERSION,
            operation: authority.operation,
            task: authority.task.to_owned(),
            actor: authority.actor.clone(),
            epoch: authority.epoch,
            claim: authority.claim,
            validation: authority.validation.to_owned(),
            source,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!(
                "unsupported completion operation version {}; expected {VERSION}",
                self.version
            ));
        }
        if self.task.is_empty() || self.validation.is_empty() {
            return Err("completion operation omits task or validation authority".to_owned());
        }
        Ok(())
    }

    fn admits(&self, authority: &Authority<'_>) -> bool {
        self.task == authority.task
            && self.actor == *authority.actor
            && self.epoch == authority.epoch
            && self.claim == authority.claim
            && self.validation == authority.validation
    }
}

pub(super) fn run(repository: &Repository, authority: Authority<'_>) -> Outcome<()> {
    let source = match capture_source(repository) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    run_with(repository, authority, source, launch)
}

fn run_with(
    repository: &Repository,
    authority: Authority<'_>,
    source: Digest,
    launcher: impl FnOnce(&Repository, &Path) -> io::Result<()>,
) -> Outcome<()> {
    let root = match operation_root(repository, authority.claim) {
        Ok(root) => root,
        Err(error) => return fault("gate::resolution-operation-unavailable", error),
    };
    let manifest = match load_manifest(&root) {
        Ok(Some(manifest)) => manifest,
        Ok(None) => {
            let manifest = Manifest::new(&authority, source.clone());
            if let Err(error) = initialize(&root, &manifest) {
                return fault("gate::resolution-operation-unavailable", error);
            }
            manifest
        }
        Err(error) => return fault("gate::resolution-operation-invalid", error),
    };
    if !manifest.admits(&authority) {
        return refusal(
            "gate::resolution-operation-mismatch",
            format!(
                "operation={} task={} actor={} epoch={} claim={}",
                manifest.operation, manifest.task, manifest.actor, manifest.epoch, manifest.claim,
            ),
            "retry the exact validation under the claim that started this completion operation",
        );
    }
    if manifest.source != source {
        return refusal(
            "gate::resolution-operation-source-moved",
            format!("sealed={}; observed={source}", manifest.source),
            "restore the exact resolved index sealed by the completion operation",
        );
    }
    let terminal = root.join(TERMINAL);
    match read_private_file(&terminal) {
        Ok(bytes) if !bytes.is_empty() => {
            return replay_terminal(&root, &manifest, &bytes);
        }
        Ok(_) => {}
        Err(error) => return fault("gate::resolution-operation-unavailable", error),
    }
    if let Err(error) = launcher(repository, &root) {
        return fault("gate::resolution-operation-worker-unavailable", error);
    }
    pending(manifest.operation)
}

pub(super) fn exists(
    repository: &Repository,
    authority: Authority<'_>,
) -> Result<bool, Outcome<()>> {
    let root = operation_root(repository, authority.claim)
        .map_err(|error| fault("gate::resolution-operation-unavailable", error))?;
    let Some(manifest) = load_manifest(&root).map_err(|error| {
        fault(
            "gate::resolution-operation-invalid",
            io::Error::other(error),
        )
    })?
    else {
        return Ok(false);
    };
    if !manifest.admits(&authority) {
        return Ok(false);
    }
    let source = capture_source(repository)?;
    Ok(source == manifest.source)
}

fn capture_source(repository: &Repository) -> Result<Digest, Outcome<()>> {
    configured::Dispatch::capture_at(repository.clone())
        .map(|dispatch| dispatch.source().clone())
        .map_err(|failure| {
            if failure.is_fault() {
                fault(
                    failure.reason(),
                    io::Error::other(failure.detail().to_owned()),
                )
            } else {
                refusal(
                    failure.reason(),
                    failure.detail().to_owned(),
                    "restore the exact resolved source before retrying task completion",
                )
            }
        })
}

fn operation_root(repository: &Repository, claim: wire::Seq) -> io::Result<PathBuf> {
    let state = repository.admit_state().map_err(io::Error::other)?;
    Ok(state
        .worktree_runtime()
        .join(ROOT)
        .join(claim.get().to_string()))
}

pub(super) fn terminal_path(repository: &Repository, claim: wire::Seq) -> io::Result<PathBuf> {
    Ok(operation_root(repository, claim)?.join(TERMINAL))
}

fn initialize(root: &Path, manifest: &Manifest) -> io::Result<()> {
    let parent = root
        .parent()
        .ok_or_else(|| io::Error::other("operation root has no completion parent"))?;
    let state = parent
        .parent()
        .ok_or_else(|| io::Error::other("completion parent has no state root"))?;
    ensure_private_directory(state)?;
    ensure_private_directory(parent)?;
    ensure_private_directory(root)?;
    ensure_private_file(&root.join(PROGRESS))?;
    ensure_private_file(&root.join(TERMINAL))?;
    publish_json(&root.join(MANIFEST), manifest)?;
    File::open(parent)?.sync_all()?;
    File::open(state)?.sync_all()?;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::other(
                    "completion operation root is not a real directory",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> io::Result<()> {
    match create_private_file(path) {
        Ok(file) => file.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
                Ok(())
            } else {
                Err(io::Error::other("completion artifact is not a real file"))
            }
        }
        Err(error) => Err(error),
    }
}

fn read_private_file(path: &Path) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other("completion artifact is not a real file"));
    }
    fs::read(path)
}

fn publish_json(path: &Path, manifest: &Manifest) -> io::Result<()> {
    let temporary = path.with_extension("staged");
    let opened = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary);
    let mut file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let staged: Manifest = serde_json::from_slice(&read_private_file(&temporary)?)
                .map_err(io::Error::other)?;
            if staged != *manifest {
                return Err(io::Error::other(
                    "staged completion manifest names different authority",
                ));
            }
            fs::rename(&temporary, path)?;
            return File::open(path.parent().expect("manifest has a parent"))?.sync_all();
        }
        Err(error) => return Err(error),
    };
    serde_json::to_writer(&mut file, manifest).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(path.parent().expect("manifest has a parent"))?.sync_all()
}

fn load_manifest(root: &Path) -> Result<Option<Manifest>, String> {
    let path = root.join(MANIFEST);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            return Err(format!("{} is not a real regular file", path.display()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    manifest.validate()?;
    Ok(Some(manifest))
}

fn launch(repository: &Repository, root: &Path) -> io::Result<()> {
    let name = LockName::new("task-completion").expect("fixed completion lock name is valid");
    if matches!(
        LockFile::new(name, root.join(LOCK)).availability()?,
        Availability::Held
    ) {
        return Ok(());
    }
    let executable = env::current_exe()?;
    let spec = worker_spec_at(
        executable,
        [
            OsString::from(RESOLUTION_OPERATION_WORKER_ARGUMENT),
            root.as_os_str().to_owned(),
        ],
        repository.worktree_root(),
        ProcessBounds::production(),
    )
    .map_err(io::Error::other)?;
    let running = sill::spawn(spec)?;
    let root = root.to_path_buf();
    let _supervisor = std::thread::Builder::new()
        .name("spice-resolution-operation".to_owned())
        .spawn(move || {
            let detail = match running.wait() {
                Ok(waited) => format!(
                    "completion worker exited before publishing a terminal result: {:?}; stderr={}",
                    waited.completion(),
                    captured(waited.stderr()),
                ),
                Err(error) => format!("completion worker wait or reap failed: {error}"),
            };
            let _ = publish_supervisor_terminal(&root, detail);
        })?;
    Ok(())
}

fn publish_supervisor_terminal(root: &Path, detail: String) -> io::Result<()> {
    if !read_private_file(&root.join(TERMINAL))?.is_empty() {
        return Ok(());
    }
    let manifest = load_manifest(root)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("completion operation manifest is absent"))?;
    if recover_staged_terminal(root, &manifest)? {
        return Ok(());
    }
    publish_failure(
        root,
        WorkerFailure {
            reason: "gate::resolution-operation-worker-lost".to_owned(),
            detail,
            fault: true,
        },
    )
}

pub(super) fn worker(root: &Path) -> io::Result<()> {
    let repository = Repository::discover(env::current_dir()?).map_err(io::Error::other)?;
    let state = repository.admit_state().map_err(io::Error::other)?;
    if root.parent() != Some(&state.worktree_runtime().join(ROOT)) {
        return Err(io::Error::other(
            "completion operation root crosses its worktree state authority",
        ));
    }
    let root_metadata = fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err(io::Error::other(
            "completion operation root is not a real directory",
        ));
    }
    let manifest = load_manifest(root)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("completion operation manifest is absent"))?;
    if operation_root(&repository, manifest.claim)? != root {
        return Err(io::Error::other(
            "completion operation root does not match its claim authority",
        ));
    }
    run_once(root, &manifest, || run_worker(root, &repository, &manifest))
}

fn run_once(
    root: &Path,
    manifest: &Manifest,
    execute: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let name = LockName::new("task-completion").expect("fixed completion lock name is valid");
    let lock = LockFile::new(name, root.join(LOCK));
    let _possession = lock.acquire()?;
    let terminal_metadata = fs::symlink_metadata(root.join(TERMINAL))?;
    if terminal_metadata.file_type().is_symlink() || !terminal_metadata.file_type().is_file() {
        return Err(io::Error::other(
            "completion terminal is not a real regular file",
        ));
    }
    if terminal_metadata.len() != 0 {
        return Ok(());
    }
    if recover_staged_terminal(root, manifest)? {
        return Ok(());
    }
    execute()
}

fn run_worker(root: &Path, repository: &Repository, manifest: &Manifest) -> io::Result<()> {
    let _dispatch = match DispatchGuard::wait_in(repository) {
        Ok(dispatch) => dispatch,
        Err(error) => {
            return publish_failure(
                root,
                WorkerFailure {
                    reason: "gate::pre-commit-lock-unavailable".to_owned(),
                    detail: error.to_string(),
                    fault: true,
                },
            );
        }
    };
    let source = match configured::Dispatch::capture_at(repository.clone()) {
        Ok(dispatch) => dispatch.source().clone(),
        Err(failure) => {
            return publish_failure(
                root,
                WorkerFailure {
                    reason: failure.reason().to_owned(),
                    detail: failure.detail().to_owned(),
                    fault: failure.is_fault(),
                },
            );
        }
    };
    if source != manifest.source {
        return publish_failure(
            root,
            WorkerFailure {
                reason: "gate::resolution-operation-source-moved".to_owned(),
                detail: format!("sealed={}; observed={source}", manifest.source),
                fault: false,
            },
        );
    }
    let staged = root.join(TERMINAL_STAGED);
    reset_private_file(&staged)?;
    if let Err(error) = resolution_worker(&root.join(PROGRESS), &staged) {
        return publish_failure(
            root,
            WorkerFailure {
                reason: "gate::resolution-operation-worker-lost".to_owned(),
                detail: error.to_string(),
                fault: true,
            },
        );
    }
    File::open(&staged)?.sync_all()?;
    fs::rename(staged, root.join(TERMINAL))?;
    File::open(root)?.sync_all()
}

fn publish_failure(root: &Path, failure: WorkerFailure) -> io::Result<()> {
    let staged = root.join(TERMINAL_STAGED);
    reset_private_file(&staged)?;
    write_failure(&staged, &failure)?;
    File::open(&staged)?.sync_all()?;
    fs::rename(staged, root.join(TERMINAL))?;
    File::open(root)?.sync_all()
}

fn recover_staged_terminal(root: &Path, manifest: &Manifest) -> io::Result<bool> {
    let staged = root.join(TERMINAL_STAGED);
    let bytes = match read_private_file(&staged) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some((kind, document)) = bytes.split_first() else {
        return Ok(false);
    };
    match *kind {
        REPORT_RECORD => {
            let report = ResolutionReport::decode(document).map_err(io::Error::other)?;
            if report.source != manifest.source {
                return Err(io::Error::other(
                    "staged resolution terminal names a different source",
                ));
            }
        }
        FAILURE_RECORD => {
            let _: WorkerFailure = serde_json::from_slice(document).map_err(io::Error::other)?;
        }
        other => {
            return Err(io::Error::other(format!(
                "staged resolution terminal has unknown record {other}",
            )));
        }
    }
    File::open(&staged)?.sync_all()?;
    fs::rename(staged, root.join(TERMINAL))?;
    File::open(root)?.sync_all()?;
    Ok(true)
}

fn replay_terminal(root: &Path, manifest: &Manifest, bytes: &[u8]) -> Outcome<()> {
    if let Some((kind, document)) = bytes.split_first()
        && *kind == REPORT_RECORD
        && let Ok(report) = ResolutionReport::decode(document)
        && report.source != manifest.source
    {
        return fault(
            "gate::resolution-operation-terminal-mismatch",
            io::Error::other(format!(
                "operation={} sealed={} terminal={}",
                manifest.operation, manifest.source, report.source,
            )),
        );
    }
    resolution_terminal(&root.join(TERMINAL), &root.join(PROGRESS))
}

fn reset_private_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(io::Error::other("staged terminal is not a real file"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .sync_all()
}

fn pending(operation: wire::CanonicalUuid) -> Outcome<()> {
    Outcome::Refusal(Refusal::evidence(
        super::RESOLUTION_PENDING_REASON,
        "",
        operation.to_string(),
    ))
}

fn refusal(reason: &str, evidence: String, summary: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal::evidence(reason, evidence, summary))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = env::temp_dir().join(format!(
                "spice-completion-{name}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4().simple(),
            ));
            fs::create_dir(&root).expect("create completion scratch");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove completion scratch");
        }
    }

    fn repository(name: &str) -> (Scratch, Repository) {
        let temporary = Scratch::new(name);
        let initialized = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(temporary.path())
            .status()
            .expect("initialize Git repository");
        assert!(initialized.success());
        let repository = Repository::discover(temporary.path()).expect("discover repository");
        (temporary, repository)
    }

    fn authority<'a>(task: &'a str, actor: &'a wire::Actor, validation: &'a str) -> Authority<'a> {
        Authority {
            operation: "00000000-0000-4000-8000-000000000041"
                .parse()
                .expect("operation UUID"),
            task,
            actor,
            epoch: "00000000-0000-4000-8000-000000000042"
                .parse()
                .expect("epoch UUID"),
            claim: wire::Seq::new(43).expect("claim sequence"),
            validation,
        }
    }

    #[test]
    fn manifest_binds_claim_source_and_validation_but_not_retry_command_id() {
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000044"
            .parse()
            .expect("lane actor");
        let first = authority("TASK-1", &actor, "focused tests passed");
        let source = Digest::compute(wire::Domain::GateTree, b"resolved source");
        let manifest = Manifest::new(&first, source);
        assert!(manifest.admits(&first));

        let mut retry = first.clone();
        retry.operation = "00000000-0000-4000-8000-000000000045"
            .parse()
            .expect("retry UUID");
        assert!(manifest.admits(&retry));

        let moved = authority("TASK-1", &actor, "different validation");
        assert!(!manifest.admits(&moved));
    }

    #[test]
    fn initialization_publishes_one_restart_readable_private_generation() {
        let temporary = Scratch::new("initialize");
        let root = temporary.path().join("operation");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000046"
            .parse()
            .expect("lane actor");
        let manifest = Manifest::new(
            &authority("TASK-2", &actor, "exact validation"),
            Digest::compute(wire::Domain::GateTree, b"source"),
        );
        initialize(&root, &manifest).expect("initialize operation");

        assert_eq!(
            load_manifest(&root).expect("reload operation"),
            Some(manifest)
        );
        for path in [
            root.join(MANIFEST),
            root.join(PROGRESS),
            root.join(TERMINAL),
        ] {
            let metadata = fs::symlink_metadata(&path).expect("operation artifact metadata");
            assert!(metadata.file_type().is_file());
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        assert!(!root.join(MANIFEST).with_extension("staged").exists());
    }

    #[test]
    fn restart_promotes_an_exact_synced_manifest_generation() {
        let temporary = Scratch::new("staged-manifest");
        let root = temporary.path().join("task-completion").join("43");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000053"
            .parse()
            .expect("lane actor");
        let manifest = Manifest::new(
            &authority("TASK-6", &actor, "exact validation"),
            Digest::compute(wire::Domain::GateTree, b"source"),
        );
        ensure_private_directory(root.parent().expect("completion parent"))
            .expect("create completion parent");
        ensure_private_directory(&root).expect("create operation root");
        let staged = root.join(MANIFEST).with_extension("staged");
        fs::write(
            &staged,
            serde_json::to_vec(&manifest).expect("encode manifest"),
        )
        .expect("stage manifest");

        initialize(&root, &manifest).expect("resume initialization");
        assert_eq!(load_manifest(&root).expect("load manifest"), Some(manifest));
        assert!(!staged.exists());
    }

    #[test]
    fn terminal_replay_accepts_only_the_manifest_source() {
        let temporary = Scratch::new("terminal");
        let root = temporary.path();
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000047"
            .parse()
            .expect("lane actor");
        let source = Digest::compute(wire::Domain::GateTree, b"source");
        let manifest = Manifest::new(
            &authority("TASK-3", &actor, "exact validation"),
            source.clone(),
        );
        let report = serde_json::json!({
            "version": RESOLUTION_REPORT_VERSION,
            "source": source,
            "configured": {
                "source": source,
                "steps": [{
                    "label": "fixture",
                    "argv": ["/bin/true"],
                    "selected_paths": ["src/lib.rs"],
                    "enabled": true,
                    "findings": [],
                }],
            },
        });
        let mut terminal = vec![REPORT_RECORD];
        terminal.extend(serde_json::to_vec(&report).expect("encode report"));
        fs::write(root.join(TERMINAL), &terminal).expect("write terminal");
        fs::write(root.join(PROGRESS), b"").expect("write progress");
        assert!(matches!(
            replay_terminal(root, &manifest, &terminal),
            Outcome::Reply(_)
        ));

        let other = Digest::compute(wire::Domain::GateTree, b"other");
        let mismatched = Manifest {
            source: other,
            ..manifest
        };
        assert!(matches!(
            replay_terminal(root, &mismatched, &terminal),
            Outcome::Fault(ref fault)
                if fault.reason == "gate::resolution-operation-terminal-mismatch"
        ));
    }

    #[test]
    fn restart_promotes_a_synced_terminal_without_rerunning_evidence() {
        let temporary = Scratch::new("staged-terminal");
        let root = temporary.path().join("operation");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000052"
            .parse()
            .expect("lane actor");
        let source = Digest::compute(wire::Domain::GateTree, b"source");
        let manifest = Manifest::new(
            &authority("TASK-5", &actor, "exact validation"),
            source.clone(),
        );
        initialize(&root, &manifest).expect("initialize operation");
        let report = serde_json::json!({
            "version": RESOLUTION_REPORT_VERSION,
            "source": source,
            "configured": {
                "source": source,
                "steps": [{
                    "label": "fixture",
                    "argv": ["/bin/true"],
                    "selected_paths": ["src/lib.rs"],
                    "enabled": true,
                    "findings": [],
                }],
            },
        });
        let mut staged = vec![REPORT_RECORD];
        staged.extend(serde_json::to_vec(&report).expect("encode terminal"));
        fs::write(root.join(TERMINAL_STAGED), &staged).expect("stage terminal");

        assert!(recover_staged_terminal(&root, &manifest).expect("recover terminal"));
        assert_eq!(
            fs::read(root.join(TERMINAL)).expect("read terminal"),
            staged
        );
        assert!(!root.join(TERMINAL_STAGED).exists());
    }

    #[test]
    fn supervisor_publishes_a_missing_terminal_without_replacing_one() {
        let temporary = Scratch::new("supervisor-terminal");
        let root = temporary.path().join("operation");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000054"
            .parse()
            .expect("lane actor");
        let manifest = Manifest::new(
            &authority("TASK-7", &actor, "exact validation"),
            Digest::compute(wire::Domain::GateTree, b"source"),
        );
        initialize(&root, &manifest).expect("initialize operation");

        publish_supervisor_terminal(&root, "worker exited unexpectedly".to_owned())
            .expect("publish missing terminal");
        let first = fs::read(root.join(TERMINAL)).expect("read worker failure");
        let Some((kind, document)) = first.split_first() else {
            panic!("worker failure terminal is present");
        };
        assert_eq!(*kind, FAILURE_RECORD);
        let failure: WorkerFailure = serde_json::from_slice(document).expect("decode failure");
        assert_eq!(failure.reason, "gate::resolution-operation-worker-lost");

        publish_supervisor_terminal(&root, "different failure".to_owned())
            .expect("retain published terminal");
        assert_eq!(
            fs::read(root.join(TERMINAL)).expect("reread worker failure"),
            first
        );
    }

    #[test]
    fn concurrent_workers_execute_evidence_once_and_share_the_terminal() {
        let temporary = Scratch::new("execute-once");
        let root = temporary.path().join("operation");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000055"
            .parse()
            .expect("lane actor");
        let manifest = Manifest::new(
            &authority("TASK-8", &actor, "exact validation"),
            Digest::compute(wire::Domain::GateTree, b"source"),
        );
        initialize(&root, &manifest).expect("initialize operation");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut workers = Vec::new();
        for ordinal in 0..2 {
            let root = root.clone();
            let manifest = manifest.clone();
            let barrier = barrier.clone();
            let executions = executions.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                run_once(&root, &manifest, || {
                    executions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    publish_failure(
                        &root,
                        WorkerFailure {
                            reason: "fixture::terminal".to_owned(),
                            detail: format!("worker={ordinal}"),
                            fault: false,
                        },
                    )
                })
                .expect("worker completes");
            }));
        }
        for worker in workers {
            worker.join().expect("worker joins");
        }

        assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            !fs::read(root.join(TERMINAL))
                .expect("read shared terminal")
                .is_empty()
        );
    }

    #[test]
    fn delayed_handoff_survives_disconnect_running_retry_and_restart() {
        let (_temporary, repository) = repository("replay");
        let actor: wire::Actor = "lane:00000000-0000-4000-8000-000000000048"
            .parse()
            .expect("lane actor");
        let first = authority("TASK-4", &actor, "exact validation");
        let source = Digest::compute(wire::Domain::GateTree, b"source");
        let launches = std::cell::Cell::new(0);
        let possession = std::cell::RefCell::new(None);
        let started = std::time::Instant::now();
        let pending = run_with(&repository, first.clone(), source.clone(), |_, root| {
            launches.set(launches.get() + 1);
            let name = LockName::new("task-completion").expect("completion lock name");
            let lock = LockFile::new(name, root.join(LOCK));
            possession.replace(Some(lock.acquire().expect("hold delayed worker lock")));
            Ok(())
        });
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        let Outcome::Refusal(ref pending) = pending else {
            panic!("first completion handoff is pending");
        };
        assert_eq!(pending.reason(), RESOLUTION_PENDING_REASON);
        assert_eq!(pending.complete_explanation(), first.operation.to_string());

        let mut retry = first.clone();
        retry.operation = "00000000-0000-4000-8000-000000000049"
            .parse()
            .expect("retry UUID");
        let Outcome::Refusal(retrying) = run_with(&repository, retry, source.clone(), |_, root| {
            let name = LockName::new("task-completion").expect("completion lock name");
            assert!(matches!(
                LockFile::new(name, root.join(LOCK))
                    .availability()
                    .expect("inspect delayed worker"),
                Availability::Held
            ));
            Ok(())
        }) else {
            panic!("running completion remains pending");
        };
        assert_eq!(retrying.complete_explanation(), first.operation.to_string());

        let root = operation_root(&repository, first.claim).expect("operation root");
        drop(possession.take());
        let report = serde_json::json!({
            "version": RESOLUTION_REPORT_VERSION,
            "source": source,
            "configured": {
                "source": source,
                "steps": [{
                    "label": "fixture",
                    "argv": ["/bin/true"],
                    "selected_paths": ["src/lib.rs"],
                    "enabled": true,
                    "findings": [],
                }],
            },
        });
        let mut terminal = vec![REPORT_RECORD];
        terminal.extend(serde_json::to_vec(&report).expect("encode terminal"));
        fs::write(root.join(TERMINAL), terminal).expect("publish terminal");

        let restarted = Repository::discover(repository.worktree_root())
            .expect("rediscover repository after simulated restart");
        assert!(matches!(
            run_with(&restarted, first, source, |_, _| {
                panic!("terminal replay must not launch another worker")
            }),
            Outcome::Reply(_)
        ));
        assert_eq!(launches.get(), 1);
    }
}
