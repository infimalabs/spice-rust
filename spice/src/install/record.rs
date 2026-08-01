use super::*;
use std::collections::HashSet;

pub(super) fn validate_existing_record(prepared: &Prepared) -> Result<(), Error> {
    let record = read_record(&prepared.record.path)?;
    let preflight = preflight_unapply(&prepared.repository, &prepared.home, &record)?;
    if prepared
        .artifacts
        .iter()
        .any(|artifact| artifact.operation.change != Change::Retain)
    {
        return Err(refusal(
            "install::already-applied",
            Some("the recorded installed artifact set".to_owned()),
            Some(prepared.plan.digest.to_string()),
            "unapply the recorded installation before applying changed artifacts",
        ));
    }
    let _ = preflight;
    Ok(())
}

pub(super) struct UnapplyPreflight {
    pub(super) destinations: Vec<Destination>,
    pub(super) hook: Destination,
    pub(super) installed: Vec<Snapshot>,
    pub(super) hook_observed: Snapshot,
}

pub(super) fn preflight_unapply(
    repository: &Repository,
    home: &Path,
    record: &Record,
) -> Result<UnapplyPreflight, Error> {
    validate_record(repository, home, record)?;
    let expected = install_destinations(repository, home)?;
    let destinations = record
        .writes
        .iter()
        .map(|write| {
            expected
                .iter()
                .find(|destination| destination.path == write.path)
                .cloned()
                .ok_or_else(|| record_shape_refusal("write outside the install destination set"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let hook = destination(
        repository.common_dir().join("config"),
        repository.common_dir(),
    )?;
    let installed = record
        .writes
        .iter()
        .map(validate_installed)
        .collect::<Result<Vec<_>, _>>()?;
    let hook_observed = validate_installed_hook(&record.hook)?;
    for destination in &destinations {
        validate_destination(destination)?;
    }
    validate_destination(&hook)?;
    Ok(UnapplyPreflight {
        destinations,
        hook,
        installed,
        hook_observed,
    })
}

fn validate_record(repository: &Repository, home: &Path, record: &Record) -> Result<(), Error> {
    if record.version != RECORD_VERSION {
        return Err(record_version_refusal(record.version));
    }
    if record.repository != repository.worktree_root() || record.home != home {
        return Err(record_shape_refusal(
            "record ownership does not match this repository and home",
        ));
    }
    let identity = PlanIdentity {
        version: PLAN_VERSION,
        repository: &record.repository,
        operations: &record.operations,
        hook: &record.selection,
    };
    let identity_bytes =
        wire::encode(&identity).map_err(|source| encode_fault("recorded plan", source))?;
    let digest = Digest::compute(Domain::InstallPlan, &identity_bytes);
    if record.plan != digest {
        return Err(record_shape_refusal(
            "recorded plan digest does not identify its plan fields",
        ));
    }
    let expected = install_destinations(repository, home)?;
    if record.operations.len() != expected.len() {
        return Err(record_shape_refusal(
            "recorded plan does not name the complete destination set",
        ));
    }
    let mut unique = HashSet::new();
    let mut writes = record.writes.iter();
    for (operation, destination) in record.operations.iter().zip(&expected) {
        if operation.path != destination.path || !unique.insert(&operation.path) {
            return Err(record_shape_refusal(
                "recorded destinations are reordered, duplicated, or out of root",
            ));
        }
        validate_parent_observation(&operation.path, &operation.parent, &destination.root)?;
        validate_operation_shape(operation)?;
        if operation.change == Change::Retain {
            continue;
        }
        let write = writes
            .next()
            .ok_or_else(|| record_shape_refusal("record omitted a changed plan destination"))?;
        if write.path != operation.path
            || write.installed != operation.desired
            || write.mode != operation.mode
            || observation(&write.prior) != operation.observed
        {
            return Err(record_shape_refusal(
                "recorded reversal does not close over its plan operation",
            ));
        }
    }
    if writes.next().is_some() {
        return Err(record_shape_refusal(
            "record contains a reversal outside its changed plan destinations",
        ));
    }
    validate_recorded_hook(repository, record)
}

fn validate_operation_shape(operation: &Operation) -> Result<(), Error> {
    let valid = match (&operation.change, &operation.observed) {
        (Change::Create, Observation::Absent) => true,
        (Change::Replace, Observation::File { digest, mode, .. }) => {
            digest != &operation.desired || *mode != operation.mode
        }
        (Change::Retain, Observation::File { digest, mode, .. }) => {
            digest == &operation.desired && *mode == operation.mode
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(record_shape_refusal(
            "recorded change disagrees with its observed and desired states",
        ))
    }
}

fn validate_recorded_hook(repository: &Repository, record: &Record) -> Result<(), Error> {
    let hook = &record.hook;
    let selection = &record.selection;
    let expected_config = repository.common_dir().join("config");
    let expected_hook = destination(
        repository.common_dir().join(".spice/hooks/pre-commit"),
        repository.common_dir(),
    )?;
    let expected_selected = expected_hook
        .path
        .parent()
        .expect("the installed hook has a parent")
        .display()
        .to_string();
    validate_parent_observation(
        &selection.config,
        &selection.parent,
        repository.common_dir(),
    )?;
    if hook.config != selection.config
        || selection.config != expected_config
        || hook.previous.as_deref() != selection.previous.as_deref()
        || selection.selected.as_deref() != Some(&expected_selected)
        || hook.selected != expected_selected
        || observation(&hook.prior) != selection.observed
        || hook.mode != selection.mode
        || hook.changed != (selection.change != Change::Retain)
    {
        return Err(record_shape_refusal(
            "recorded hook reversal does not close over its plan selection",
        ));
    }
    let prior = snapshot_bytes(&hook.prior)?;
    let previous = parse_hook(&prior)?.value;
    let expected_change = match previous.as_deref() {
        Some(value) if Some(value) == selection.selected.as_deref() => Change::Retain,
        Some(_) => Change::Replace,
        None => Change::Create,
    };
    let expected_mode = snapshot_mode(&hook.prior).unwrap_or(effective_mode(DATA_MODE));
    if previous.as_deref() != selection.previous.as_deref()
        || selection.change != expected_change
        || selection.mode != expected_mode
    {
        return Err(record_shape_refusal(
            "recorded hook selection disagrees with its prior configuration",
        ));
    }
    let installed = set_hook(&prior, Some(&hook.selected))?;
    if Digest::compute(Domain::InstallArtifact, installed.as_bytes()) != hook.installed {
        return Err(record_shape_refusal(
            "recorded hook digest does not identify the selected hook bytes",
        ));
    }
    Ok(())
}

fn validate_installed_hook(hook: &RecordedHook) -> Result<Snapshot, Error> {
    let snapshot = snapshot(&hook.config)?;
    if hook.changed
        && (snapshot_digest(&snapshot).as_ref() != Some(&hook.installed)
            || snapshot_mode(&snapshot) != Some(hook.mode))
    {
        return Err(drift_refusal(&hook.config));
    }
    Ok(snapshot)
}

fn validate_installed(write: &RecordedWrite) -> Result<Snapshot, Error> {
    let snapshot = snapshot(&write.path)?;
    let digest = snapshot_digest(&snapshot);
    let mode = match &snapshot {
        Snapshot::File { mode, .. } => Some(mode),
        Snapshot::Absent => None,
    };
    if digest.as_ref() != Some(&write.installed) || mode.copied() != Some(write.mode) {
        Err(drift_refusal(&write.path))
    } else {
        Ok(snapshot)
    }
}

pub(super) fn validate_installed_state(
    path: &Path,
    digest: &Digest,
    mode: u32,
) -> Result<(), Error> {
    let snapshot = snapshot(path)?;
    if snapshot_digest(&snapshot).as_ref() != Some(digest) || snapshot_mode(&snapshot) != Some(mode)
    {
        return Err(destination_drift_refusal(path));
    }
    Ok(())
}

pub(super) fn rollback_unapply(preflight: &UnapplyPreflight, restored: &[usize]) {
    for index in restored.iter().rev() {
        let _ = restore_snapshot(
            &preflight.destinations[*index],
            &preflight.installed[*index],
        );
    }
}

pub(super) fn restore_snapshot(
    destination: &Destination,
    snapshot: &Snapshot,
) -> Result<(), Error> {
    match snapshot {
        Snapshot::Absent => match fs::remove_file(&destination.path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(io_fault("remove installed file", &destination.path, source)),
        },
        Snapshot::File { bytes, mode, .. } => publish(destination, bytes, *mode),
    }
}

pub(super) fn prepare_unapplied(
    repository: &Repository,
    record: Record,
    installed: &[Snapshot],
    hook_observed: &Snapshot,
) -> Result<Plan, Error> {
    let operations = record
        .writes
        .iter()
        .enumerate()
        .map(|(index, write)| {
            let parent = record
                .operations
                .iter()
                .find(|operation| operation.path == write.path)
                .expect("validated record writes belong to one operation")
                .parent
                .clone();
            let (change, desired, mode) = match &write.prior {
                Snapshot::Absent => (
                    Change::Remove,
                    Digest::compute(Domain::InstallArtifact, &[]),
                    0,
                ),
                Snapshot::File { bytes, mode, .. } => (
                    Change::Replace,
                    Digest::compute(Domain::InstallArtifact, bytes),
                    *mode,
                ),
            };
            Operation {
                path: write.path.clone(),
                parent,
                change,
                observed: Observation::File {
                    digest: write.installed.clone(),
                    mode: write.mode,
                    generation: snapshot_generation(&installed[index])
                        .expect("validated installed artifact is a file"),
                },
                desired,
                mode,
            }
        })
        .collect();
    Ok(Plan {
        version: PLAN_VERSION,
        action: Action::Unapplied,
        repository: repository.worktree_root().to_path_buf(),
        operations,
        hook: HookSelection {
            config: record.hook.config,
            parent: record.selection.parent,
            observed: observation(hook_observed),
            previous: HookValue::Value {
                value: record.hook.selected,
            },
            selected: HookValue::from_option(record.hook.previous),
            mode: snapshot_mode(&record.hook.prior).unwrap_or(0),
            change: match (&record.hook.prior, record.hook.changed) {
                (Snapshot::Absent, true) => Change::Remove,
                (Snapshot::File { .. }, true) => Change::Replace,
                (_, false) => Change::Retain,
            },
        },
        digest: record.plan,
    })
}
