use super::*;

pub(super) fn materialize_and_update(
    root: &Path,
    target: &str,
    expected: &str,
    action: &str,
) -> Result<(), Error> {
    let marker = transaction_path(root)?;
    write_pending(
        &marker,
        &Pending::Move {
            expected: expected.to_owned(),
            target: target.to_owned(),
        },
    )?;
    let materialized = run(root, &["read-tree", "--reset", "-u", target])
        .map_err(|error| restore_move_failure(root, error))?;
    if !materialized.success() {
        let detail = materialized.detail(action);
        recover_pending(root)?;
        return Err(Error::fault("git::materialize-failed", detail));
    }
    let reference = required_read(
        root,
        &["symbolic-ref", "--quiet", "HEAD"],
        "resolve HEAD ref",
    )
    .map_err(|error| restore_move_failure(root, error))?;
    let updated = run(root, &["update-ref", &reference, target, expected])
        .map_err(|error| restore_move_failure(root, error))?;
    let current = required_read(root, &["rev-parse", "HEAD"], "read updated HEAD")
        .map_err(|error| restore_move_failure(root, error))?;
    if updated.success() || current == target {
        remove_marker(&marker)?;
        return Ok(());
    }
    let detail = updated.detail(action);
    recover_pending(root)?;
    let combined = format!("{}\n{}", updated.stdout, updated.stderr).to_ascii_lowercase();
    let reason = if combined.contains("cannot lock ref")
        && combined.contains(" is at ")
        && combined.contains(" but expected ")
    {
        "git::ref-race"
    } else {
        "git::ref-update-refused"
    };
    Err(Error::refusal(
        reason,
        detail,
        concat!(
            "inspect the restored clean checkout and retry the ",
            "same task boundary",
        ),
    ))
}

fn restore_move_failure(root: &Path, error: Error) -> Error {
    match recover_pending(root) {
        Ok(()) => error,
        Err(recovery) => recovery,
    }
}

pub(super) fn materialize_conflict(
    root: &Path,
    merged_tree: &str,
    records: &[String],
    agent: &str,
    upstream: &str,
    message: &str,
) -> Result<(), Error> {
    let previous = snapshot_merge_state(root)?;
    let marker = transaction_path(root)?;
    write_pending(
        &marker,
        &Pending::Conflict {
            agent: agent.to_owned(),
            upstream: upstream.to_owned(),
            previous,
        },
    )?;
    let result = install_conflict(root, merged_tree, records, agent, upstream, message);
    if let Err(error) = result {
        recover_pending(root)?;
        return Err(error);
    }
    remove_marker(&marker)
}

fn install_conflict(
    root: &Path,
    merged_tree: &str,
    records: &[String],
    agent: &str,
    upstream: &str,
    message: &str,
) -> Result<(), Error> {
    let parsed = records
        .iter()
        .map(|record| parse_conflict_record(record))
        .collect::<Result<Vec<_>, _>>()?;
    let materialized = run(root, &["read-tree", "--reset", "-u", merged_tree])?;
    if !materialized.success() {
        return Err(Error::fault(
            "git::conflict-materialize-failed",
            materialized.detail("materialize conflicted merge tree"),
        ));
    }
    let paths = parsed
        .iter()
        .map(|(_, path)| path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for path in paths {
        let removed = run(root, &["update-index", "--force-remove", "--", path])?;
        if !removed.success() {
            return Err(Error::fault(
                "git::conflict-index-failed",
                removed.detail(&format!("prepare conflict index for {path}")),
            ));
        }
    }
    let index = parsed
        .iter()
        .map(|(metadata, path)| format!("{metadata}\t{path}\0"))
        .collect::<String>();
    let staged = run_with_input(
        root,
        &["update-index", "-z", "--index-info"],
        index.as_bytes(),
    )?;
    if !staged.success() {
        return Err(Error::fault(
            "git::conflict-index-failed",
            staged.detail("install conflict index stages"),
        ));
    }
    write_git_state(root, "ORIG_HEAD", &format!("{agent}\n"))?;
    write_git_state(root, "MERGE_MODE", "no-ff\n")?;
    write_git_state(root, "MERGE_MSG", &format!("{message}\n"))?;
    write_git_state(root, "MERGE_HEAD", &format!("{upstream}\n"))
}

fn parse_conflict_record(record: &str) -> Result<(String, String), Error> {
    let (metadata, path) = record.split_once('\t').ok_or_else(|| {
        Error::fault(
            "git::conflict-record-invalid",
            format!("malformed record {record:?}"),
        )
    })?;
    if metadata.split_whitespace().count() != 3 || path.is_empty() {
        return Err(Error::fault(
            "git::conflict-record-invalid",
            format!("malformed record {record:?}"),
        ));
    }
    Ok((metadata.to_owned(), path.to_owned()))
}

pub(super) fn conflict_error(label: &str, upstream: &str) -> Error {
    Error::refusal(
        "git::recoverable-conflict",
        format!(
            concat!(
                "task {label} overlaps baseline {upstream}; Git is ",
                "paused with MERGE_HEAD and complete index stages",
            ),
            label = label,
            upstream = upstream
        ),
        format!(
            concat!(
                "resolve every conflicted path, git add the ",
                "resolution, commit while MERGE_HEAD exists, then ",
                "retry task completion for {label}",
            ),
            label = label
        ),
    )
}

pub(super) fn require_no_open_merge(root: &Path, label: &str) -> Result<(), Error> {
    if let Some(upstream) = try_read(root, &["rev-parse", "--verify", "MERGE_HEAD"])? {
        return Err(conflict_error(label, &upstream));
    }
    Ok(())
}

pub(super) fn require_clean(root: &Path, action: &str) -> Result<(), Error> {
    let status = required_read(root, &["status", "--porcelain"], "inspect worktree")?;
    if status.is_empty() {
        Ok(())
    } else {
        Err(Error::refusal(
            "git::worktree-dirty",
            format!("{action} requires a clean worktree:\n{status}"),
            "commit or clear every reported path before retrying",
        ))
    }
}

pub(super) fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool, Error> {
    let output = run(root, &["merge-base", "--is-ancestor", ancestor, descendant])?;
    match output.status {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::fault(
            "git::ancestry-failed",
            output.detail("inspect commit ancestry"),
        )),
    }
}

pub(super) fn merge_first_parent(root: &Path, commit: &str) -> Result<Option<String>, Error> {
    let line = required_read(
        root,
        &["rev-list", "--parents", "-n", "1", commit],
        "read merge parents",
    )?;
    let parents = line.split_whitespace().skip(1).collect::<Vec<_>>();
    Ok((parents.len() >= 2).then(|| parents[0].to_owned()))
}

pub(super) fn recovered_lane_head(
    root: &Path,
    head: &str,
    meta: Metadata<'_>,
) -> Result<Option<String>, Error> {
    let Some(message) = try_read(root, &["show", "-s", "--format=%B", head])? else {
        return Ok(None);
    };
    let fields = message
        .lines()
        .filter_map(|line| line.split_once(": "))
        .collect::<BTreeMap<_, _>>();
    let Some(candidate) = fields.get("Task-Lane-Head") else {
        return Ok(None);
    };
    let candidate = candidate.to_string();
    let matches = fields.get("Spice-Task-Boundary") == Some(&"1")
        && fields.get("Task-Handle") == Some(&meta.label)
        && fields.get("Task-Phase") == Some(&meta.phase);
    if !matches {
        return Ok(None);
    }
    Ok(is_ancestor(root, &candidate, head)?.then_some(candidate))
}

pub(super) fn compose_message(meta: Metadata<'_>, agent_head: &str) -> String {
    let stem = meta.project.split('.').next().unwrap_or(meta.project);
    let prefix = if stem.is_empty() {
        String::new()
    } else {
        format!("{stem}: ")
    };
    let phase = if meta.phase == "todo" || meta.phase.is_empty() {
        String::new()
    } else {
        format!(" ({})", meta.phase)
    };
    let title = if meta.title.trim().is_empty() {
        meta.label
    } else {
        meta.title.trim()
    };
    let mut trailers = vec![
        ("Spice-Task-Boundary", "1"),
        ("Task-Handle", meta.label),
        ("Task-Lane-Head", agent_head),
        ("Task-Phase", meta.phase),
        ("Task-Project", meta.project),
        ("Task-Session", meta.actor),
    ];
    trailers.retain(|(_, value)| !value.is_empty());
    trailers.sort_unstable_by_key(|(name, _)| *name);
    let trailers = trailers
        .into_iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prefix}{title} {}{phase}\n\n{trailers}", meta.label)
}

pub(super) fn recover_pending(root: &Path) -> Result<(), Error> {
    let marker = transaction_path(root)?;
    let bytes = match fs::read(&marker) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => {
            return Err(filesystem_fault(
                "read task Git transaction",
                &marker,
                error,
            ));
        }
    };
    let pending: Pending = serde_json::from_slice(&bytes).map_err(|error| {
        Error::fault(
            "git::transaction-invalid",
            format!("{}: {error}", marker.display()),
        )
    })?;
    match pending {
        Pending::Move { expected, target } => recover_move(root, &expected, &target)?,
        Pending::Conflict {
            agent,
            upstream,
            previous,
        } => recover_conflict(root, &agent, &upstream, &previous)?,
    }
    remove_marker(&marker)
}

fn recover_move(root: &Path, expected: &str, target: &str) -> Result<(), Error> {
    let current = required_read(root, &["rev-parse", "HEAD"], "recover task Git transaction")?;
    let restore = if current == target { target } else { &current };
    let restored = run(root, &["read-tree", "--reset", "-u", restore])?;
    if !restored.success() {
        return Err(Error::fault(
            "git::transaction-recovery-failed",
            restored.detail("restore task boundary tree"),
        ));
    }
    if current != expected && current != target {
        return Err(Error::refusal(
            "git::transaction-head-moved",
            format!(
                concat!(
                    "task boundary expected {expected} or ",
                    "{target}, but HEAD is {current}",
                ),
                expected = expected,
                target = target,
                current = current,
            ),
            concat!(
                "inspect the restored current HEAD before ",
                "retrying the task boundary",
            ),
        ));
    }
    Ok(())
}

fn recover_conflict(
    root: &Path,
    agent: &str,
    upstream: &str,
    previous: &BTreeMap<String, Option<String>>,
) -> Result<(), Error> {
    if try_read(root, &["rev-parse", "--verify", "MERGE_HEAD"])?.as_deref() == Some(upstream) {
        return Ok(());
    }
    let current = required_read(root, &["rev-parse", "HEAD"], "recover conflict transaction")?;
    if current != agent {
        if is_ancestor(root, agent, &current)? && is_ancestor(root, upstream, &current)? {
            return Ok(());
        }
        return Err(Error::refusal(
            "git::transaction-head-moved",
            format!(
                concat!(
                    "conflict recovery expected {agent}, but HEAD ",
                    "is {current}",
                ),
                agent = agent,
                current = current
            ),
            concat!(
                "inspect the current HEAD before retrying the ",
                "task boundary",
            ),
        ));
    }
    let restored = run(root, &["read-tree", "--reset", "-u", agent])?;
    if !restored.success() {
        return Err(Error::fault(
            "git::transaction-recovery-failed",
            restored.detail("restore pre-conflict tree"),
        ));
    }
    restore_merge_state(root, previous)
}

fn snapshot_merge_state(root: &Path) -> Result<BTreeMap<String, Option<String>>, Error> {
    MERGE_STATE_FILES
        .iter()
        .map(|name| {
            let path = git_state_path(root, name)?;
            let content = match fs::read_to_string(&path) {
                Ok(content) => Some(content),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(filesystem_fault("snapshot Git state", &path, error));
                }
            };
            Ok(((*name).to_owned(), content))
        })
        .collect()
}

fn restore_merge_state(
    root: &Path,
    previous: &BTreeMap<String, Option<String>>,
) -> Result<(), Error> {
    for name in MERGE_STATE_FILES {
        let path = git_state_path(root, name)?;
        match previous.get(*name).and_then(Option::as_ref) {
            Some(content) => fs::write(&path, content)
                .map_err(|error| filesystem_fault("restore Git state", &path, error))?,
            None => match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(filesystem_fault("clear partial Git state", &path, error));
                }
            },
        }
    }
    Ok(())
}

fn write_git_state(root: &Path, name: &str, content: &str) -> Result<(), Error> {
    let path = git_state_path(root, name)?;
    fs::write(&path, content).map_err(|error| filesystem_fault("write Git state", &path, error))
}

pub(super) fn git_state_path(root: &Path, name: &str) -> Result<PathBuf, Error> {
    let written = required_read(
        root,
        &["rev-parse", "--git-path", name],
        "resolve Git state path",
    )?;
    let path = PathBuf::from(written);
    Ok(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

pub(super) fn transaction_path(root: &Path) -> Result<PathBuf, Error> {
    git_state_path(root, TRANSACTION_FILE)
}

pub(super) fn write_pending(path: &Path, pending: &Pending) -> Result<(), Error> {
    let bytes = serde_json::to_vec(pending).map_err(|error| {
        Error::fault(
            "git::transaction-invalid",
            format!("encode transaction: {error}"),
        )
    })?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| filesystem_fault("stage task Git transaction", &temporary, error))?;
    fs::rename(&temporary, path)
        .map_err(|error| filesystem_fault("publish task Git transaction", path, error))
}

fn remove_marker(path: &Path) -> Result<(), Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(filesystem_fault("retire task Git transaction", path, error)),
    }
}

fn filesystem_fault(action: &str, path: &Path, error: std::io::Error) -> Error {
    Error::fault(
        "git::transaction-filesystem",
        format!("could not {action} at {}: {error}", path.display()),
    )
}
