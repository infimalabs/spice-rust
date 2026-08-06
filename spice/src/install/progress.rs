use super::record::{InstallRecord, LoadedRecord, write_initial_record, write_record};
use super::*;

pub(super) fn apply_with_interruption(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    interrupt_after_write: Option<usize>,
) -> Result<InstallReceipt, InstallError> {
    let (mut record, mut record_generation, fresh) =
        prepare_apply(repository, spice_binary, expected)?;
    if record.status == InstallStatus::Complete {
        return Ok(record.receipt());
    }

    let generated = record
        .plan
        .operations
        .iter()
        .map(|operation| materialize_generated(operation, spice_binary))
        .collect::<Result<Vec<_>, _>>()?;
    if fresh {
        record.plan.validate_pristine(repository)?;
    } else {
        record.plan.validate_for_repository(repository)?;
    }
    record_generation = Some(match record_generation {
        Some(generation) => write_record(repository, &record, generation)?,
        None => write_initial_record(repository, &record)?,
    });
    while record.completed < record.plan.operations.len() {
        let index = record.completed;
        record_generation = Some(apply_operation(
            repository,
            &mut record,
            &generated[index],
            index,
            record_generation.expect("the install record was published"),
        )?);
        if interrupt_after_write == Some(index) {
            return Err(InstallError::Interrupted { operation: index });
        }
        record.completed += 1;
        record_generation = Some(write_record(
            repository,
            &record,
            record_generation.expect("the install record was published"),
        )?);
    }
    record.status = InstallStatus::Complete;
    write_record(
        repository,
        &record,
        record_generation.expect("the install record was published"),
    )?;
    Ok(record.receipt())
}

fn prepare_apply(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
) -> Result<(InstallRecord, Option<Generation>, bool), InstallError> {
    match read_record(repository)? {
        Some(loaded) => prepare_loaded(repository, spice_binary, expected, loaded),
        None => {
            let plan = build_plan(repository, spice_binary)?;
            admit_digest(&plan, expected)?;
            Ok((InstallRecord::new(plan)?, None, true))
        }
    }
}

fn prepare_loaded(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    loaded: LoadedRecord,
) -> Result<(InstallRecord, Option<Generation>, bool), InstallError> {
    let record = loaded.record;
    if record.status == InstallStatus::Uninstalled {
        verify_progress(repository, &record)?;
        let plan = build_plan(repository, spice_binary)?;
        admit_digest(&plan, expected)?;
        return Ok((InstallRecord::new(plan)?, Some(loaded.generation), true));
    }
    verify_binary_context(&record.plan, spice_binary)?;
    if &record.plan_digest != expected {
        return Err(InstallError::DigestMismatch {
            expected: expected.clone(),
            observed: record.plan_digest,
        });
    }
    match record.status {
        InstallStatus::Complete => {
            verify_progress(repository, &record)?;
            Ok((record, Some(loaded.generation), false))
        }
        InstallStatus::Unapplying => Err(InstallError::UnapplyInProgress),
        InstallStatus::Uninstalled => unreachable!("handled above"),
        InstallStatus::Applying => {
            verify_progress(repository, &record)?;
            Ok((record, Some(loaded.generation), false))
        }
    }
}

fn apply_operation(
    repository: &Repository,
    record: &mut InstallRecord,
    generated: &FileImage,
    index: usize,
    record_generation: Generation,
) -> Result<Generation, InstallError> {
    let operation = &record.plan.operations[index];
    let (current, parent) = observe_bound(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
    )?;
    let (generation, parent) = if matches_generated(current.as_ref(), operation) {
        (
            validate_existing_generation(
                repository,
                record,
                operation,
                current.as_ref(),
                index,
                parent,
            )?,
            parent.ok_or_else(|| InstallError::ParentChanged(operation.destination(repository)))?,
        )
    } else if images_match(current.as_ref(), operation.previous.as_ref()) {
        publish_operation(repository, operation, generated)?
    } else {
        return Err(drift(operation, current));
    };
    record.installed[index] = Some(generation);
    record.installed_parents[index] = Some(parent);
    write_record(repository, record, record_generation)
}

fn validate_existing_generation(
    repository: &Repository,
    record: &InstallRecord,
    operation: &Operation,
    current: Option<&ObservedFile>,
    index: usize,
    parent: Option<Generation>,
) -> Result<Generation, InstallError> {
    let generation = current.expect("a generated match is present").generation;
    if record.installed_parents[index].is_some_and(|expected| Some(expected) != parent) {
        return Err(InstallError::ParentChanged(
            operation.destination(repository),
        ));
    }
    Ok(generation)
}

pub(super) fn unapply_with_interruption(
    repository: &Repository,
    interrupt_after_write: Option<usize>,
) -> Result<InstallReceipt, InstallError> {
    let loaded = read_record(repository)?.ok_or(InstallError::RecordMissing)?;
    let mut record = loaded.record;
    let mut record_generation = loaded.generation;
    match record.status {
        InstallStatus::Applying => return Err(InstallError::ApplyInProgress),
        InstallStatus::Uninstalled => {
            verify_progress(repository, &record)?;
            return Ok(record.receipt());
        }
        InstallStatus::Complete => {
            verify_progress(repository, &record)?;
            record.status = InstallStatus::Unapplying;
            record.completed = 0;
            record.restored.fill(None);
            record_generation = write_record(repository, &record, record_generation)?;
        }
        InstallStatus::Unapplying => verify_progress(repository, &record)?,
    }

    while record.completed < record.plan.operations.len() {
        let reverse = record.plan.operations.len() - 1 - record.completed;
        let operation = &record.plan.operations[reverse];
        let (current, parent) = observe_bound(
            operation.location.root(repository),
            &operation.destination,
            &operation.parent,
        )?;
        let (restored, needs_cleanup) =
            if restored_matches(current.as_ref(), operation, record.restored[reverse]) {
                (record.restored[reverse], true)
            } else if restoration_image_matches(current.as_ref(), operation) {
                (current.as_ref().map(|observed| observed.generation), true)
            } else if installed_matches(
                current.as_ref(),
                operation,
                record.installed[reverse],
                parent,
                record.installed_parents[reverse],
            ) {
                (
                    restore_operation(
                        repository,
                        operation,
                        current
                            .as_ref()
                            .expect("an installed image match is present")
                            .generation,
                    )?,
                    false,
                )
            } else {
                return Err(drift(operation, current));
            };
        record.restored[reverse] = restored;
        if needs_cleanup {
            cleanup_operation(repository, operation)?;
        }
        record_generation = write_record(repository, &record, record_generation)?;
        if interrupt_after_write == Some(reverse) {
            return Err(InstallError::Interrupted { operation: reverse });
        }
        record.completed += 1;
        record_generation = write_record(repository, &record, record_generation)?;
    }
    record.status = InstallStatus::Uninstalled;
    write_record(repository, &record, record_generation)?;
    Ok(record.receipt())
}

pub(super) fn verify_progress(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<(), InstallError> {
    record.validate_for_repository(repository)?;
    for (index, operation) in record.plan.operations.iter().enumerate() {
        let (observed, parent) = observe_bound(
            operation.location.root(repository),
            &operation.destination,
            &operation.parent,
        )?;
        let valid = match record.status {
            InstallStatus::Applying => record.installed[index].map_or_else(
                || {
                    images_match(observed.as_ref(), operation.previous.as_ref())
                        || (index == record.completed
                            && matches_generated(observed.as_ref(), operation))
                },
                |generation| {
                    installed_matches(
                        observed.as_ref(),
                        operation,
                        Some(generation),
                        parent,
                        record.installed_parents[index],
                    )
                },
            ),
            InstallStatus::Complete => installed_matches(
                observed.as_ref(),
                operation,
                record.installed[index],
                parent,
                record.installed_parents[index],
            ),
            InstallStatus::Unapplying => {
                let reversed = record.plan.operations.len() - 1 - index;
                if reversed < record.completed {
                    restored_matches(observed.as_ref(), operation, record.restored[index])
                } else if reversed == record.completed {
                    restored_matches(observed.as_ref(), operation, record.restored[index])
                        || restoration_image_matches(observed.as_ref(), operation)
                        || installed_matches(
                            observed.as_ref(),
                            operation,
                            record.installed[index],
                            parent,
                            record.installed_parents[index],
                        )
                } else {
                    installed_matches(
                        observed.as_ref(),
                        operation,
                        record.installed[index],
                        parent,
                        record.installed_parents[index],
                    )
                }
            }
            InstallStatus::Uninstalled => {
                restored_matches(observed.as_ref(), operation, record.restored[index])
            }
        };
        if !valid {
            return Err(drift(operation, observed));
        }
    }
    Ok(())
}

fn admit_digest(plan: &InstallPlan, expected: &Digest) -> Result<(), InstallError> {
    let observed = plan.digest()?;
    if &observed == expected {
        Ok(())
    } else {
        Err(InstallError::DigestMismatch {
            expected: expected.clone(),
            observed,
        })
    }
}

fn installed_matches(
    observed: Option<&ObservedFile>,
    operation: &Operation,
    generation: Option<Generation>,
    observed_parent: Option<Generation>,
    expected_parent: Option<Generation>,
) -> bool {
    matches!((observed, generation), (Some(observed), Some(_))
        if observed_parent == expected_parent
            && expected_parent.is_some()
            && matches_generated(Some(observed), operation))
}

fn restored_matches(
    observed: Option<&ObservedFile>,
    operation: &Operation,
    generation: Option<Generation>,
) -> bool {
    match (&operation.previous, observed, generation) {
        (None, None, None) => true,
        (Some(previous), Some(observed), Some(_)) => observed.image == previous.image,
        _ => false,
    }
}

fn restoration_image_matches(observed: Option<&ObservedFile>, operation: &Operation) -> bool {
    match (&operation.previous, observed) {
        (None, None) => true,
        (Some(previous), Some(observed)) => observed.image == previous.image,
        _ => false,
    }
}
