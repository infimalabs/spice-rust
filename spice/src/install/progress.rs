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
        .map(|operation| {
            materialize_generated(operation, spice_binary, &record.plan.repository.common_git)
        })
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
    record_generation = apply_activation(
        repository,
        &mut record,
        record_generation,
        interrupt_after_write,
    )?;
    record.status = InstallStatus::Complete;
    write_record(
        repository,
        &record,
        record_generation.expect("the install record was published"),
    )?;
    Ok(record.receipt())
}

fn apply_activation(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: Option<Generation>,
    interrupt_after_write: Option<usize>,
) -> Result<Option<Generation>, InstallError> {
    if record.plan.hook_activation.is_some() && record.completed == record.plan.operations.len() {
        let index = record.completed;
        hook_activation::apply(repository, &record.plan)?;
        if interrupt_after_write == Some(index) {
            return Err(InstallError::Interrupted { operation: index });
        }
        record.completed += 1;
        record_generation = Some(write_record(
            repository,
            record,
            record_generation.expect("the install record was published"),
        )?);
    }
    Ok(record_generation)
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
    if record.status == InstallStatus::Complete
        && refresh_required(repository, &record.plan, spice_binary)?
    {
        // The refresh transaction: one record swap from the complete install to
        // the current build's complete plan, then in-place replacement of each
        // destination. A binary-only drift is admitted as the exact image this
        // plan replaces; every fixed asset remains receipt-strict.
        let binary_superseded = verify_refresh_progress(repository, &record)?;
        let plan = build_plan_over(
            repository,
            spice_binary,
            Some(&record.plan),
            binary_superseded.as_ref(),
        )?;
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

fn refresh_required(
    repository: &Repository,
    plan: &InstallPlan,
    spice_binary: Option<&Path>,
) -> Result<bool, InstallError> {
    Ok(current_build_differs(plan, spice_binary)?
        || hook_activation::topology_changed(repository, plan)?)
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
    } else if operation.expects_in_place(current.as_ref()) {
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
    // Reversal is an executable direction, not historical inspection. Finish
    // it with the build that owns the live receipt; a newer build should derive
    // and apply its refresh instead of withdrawing the appliance.
    record.validate_for_repository(repository)?;
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

    while record.completed < record.plan.operation_count() {
        if let Some(generation) = reverse_activation(
            repository,
            &mut record,
            record_generation,
            interrupt_after_write,
        )? {
            record_generation = generation;
            continue;
        }
        record_generation = reverse_file_operation(
            repository,
            &mut record,
            record_generation,
            interrupt_after_write,
        )?;
    }
    record.status = InstallStatus::Uninstalled;
    write_record(repository, &record, record_generation)?;
    Ok(record.receipt())
}

fn reverse_activation(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: Generation,
    interrupt_after_write: Option<usize>,
) -> Result<Option<Generation>, InstallError> {
    let Some(activation) = record
        .plan
        .hook_activation
        .as_ref()
        .filter(|_| record.completed == 0)
    else {
        return Ok(None);
    };
    let reverse = record.plan.operation_count() - 1;
    hook_activation::restore(activation)?;
    record_generation = write_record(repository, record, record_generation)?;
    if interrupt_after_write == Some(reverse) {
        return Err(InstallError::Interrupted { operation: reverse });
    }
    record.completed += 1;
    record_generation = write_record(repository, record, record_generation)?;
    Ok(Some(record_generation))
}

fn reverse_file_operation(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: Generation,
    interrupt_after_write: Option<usize>,
) -> Result<Generation, InstallError> {
    let hook_offset = usize::from(record.plan.hook_activation.is_some());
    let file_completed = record.completed - hook_offset;
    let reverse = record.plan.operations.len() - 1 - file_completed;
    let operation = &record.plan.operations[reverse];
    let (current, parent) = observe_bound(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
    )?;
    let (restored, needs_cleanup) = restored_image(
        repository,
        record,
        operation,
        reverse,
        current.as_ref(),
        parent,
    )?;
    record.restored[reverse] = restored;
    if needs_cleanup {
        cleanup_operation(repository, operation)?;
    }
    record_generation = write_record(repository, record, record_generation)?;
    if interrupt_after_write == Some(reverse) {
        return Err(InstallError::Interrupted { operation: reverse });
    }
    record.completed += 1;
    write_record(repository, record, record_generation)
}

fn restored_image(
    repository: &Repository,
    record: &InstallRecord,
    operation: &Operation,
    reverse: usize,
    current: Option<&ObservedFile>,
    parent: Option<Generation>,
) -> Result<(Option<Generation>, bool), InstallError> {
    if restored_matches(current, operation, record.restored[reverse]) {
        return Ok((record.restored[reverse], true));
    }
    if restoration_image_matches(current, operation) {
        return Ok((current.map(|observed| observed.generation), true));
    }
    if installed_matches(
        current,
        operation,
        record.installed[reverse],
        parent,
        record.installed_parents[reverse],
    ) {
        let generation = current
            .expect("an installed image match is present")
            .generation;
        return Ok((restore_operation(repository, operation, generation)?, false));
    }
    Err(drift(operation, current.cloned()))
}

pub(super) fn verify_progress(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<(), InstallError> {
    record.validate_loaded_for_repository(repository)?;
    verify_file_progress(repository, record, reversed_file_count(record))?;
    verify_activation_progress(record)
}

fn reversed_file_count(record: &InstallRecord) -> usize {
    match record.status {
        InstallStatus::Unapplying => record
            .completed
            .saturating_sub(usize::from(record.plan.hook_activation.is_some())),
        _ => record.completed.min(record.plan.operations.len()),
    }
}

fn verify_file_progress(
    repository: &Repository,
    record: &InstallRecord,
    file_completed: usize,
) -> Result<(), InstallError> {
    for (index, operation) in record.plan.operations.iter().enumerate() {
        let (observed, parent) = observe_bound(
            operation.location.root(repository),
            &operation.destination,
            &operation.parent,
        )?;
        let valid = file_progress_valid(
            record,
            operation,
            index,
            observed.as_ref(),
            parent,
            file_completed,
        );
        if !valid {
            return Err(drift(operation, observed));
        }
    }
    Ok(())
}

fn file_progress_valid(
    record: &InstallRecord,
    operation: &Operation,
    index: usize,
    observed: Option<&ObservedFile>,
    parent: Option<Generation>,
    file_completed: usize,
) -> bool {
    match record.status {
        InstallStatus::Applying => record.installed[index].map_or_else(
            || {
                operation.expects_in_place(observed)
                    || (index == record.completed && matches_generated(observed, operation))
            },
            |generation| {
                installed_matches(
                    observed,
                    operation,
                    Some(generation),
                    parent,
                    record.installed_parents[index],
                )
            },
        ),
        InstallStatus::Complete => installed_matches(
            observed,
            operation,
            record.installed[index],
            parent,
            record.installed_parents[index],
        ),
        InstallStatus::Unapplying => {
            unapplying_file_valid(record, operation, index, observed, parent, file_completed)
        }
        InstallStatus::Uninstalled => restored_matches(observed, operation, record.restored[index]),
    }
}

fn unapplying_file_valid(
    record: &InstallRecord,
    operation: &Operation,
    index: usize,
    observed: Option<&ObservedFile>,
    parent: Option<Generation>,
    file_completed: usize,
) -> bool {
    let reversed = record.plan.operations.len() - 1 - index;
    if reversed < file_completed {
        return restored_matches(observed, operation, record.restored[index]);
    }
    if reversed == file_completed {
        return restored_matches(observed, operation, record.restored[index])
            || restoration_image_matches(observed, operation)
            || installed_matches(
                observed,
                operation,
                record.installed[index],
                parent,
                record.installed_parents[index],
            );
    }
    installed_matches(
        observed,
        operation,
        record.installed[index],
        parent,
        record.installed_parents[index],
    )
}

fn verify_activation_progress(record: &InstallRecord) -> Result<(), InstallError> {
    if let Some(activation) = &record.plan.hook_activation {
        match record.status {
            InstallStatus::Applying if record.completed < record.plan.operations.len() => {
                hook_activation::verify(activation, false)?;
            }
            InstallStatus::Applying => hook_activation::verify_transition(activation)?,
            InstallStatus::Complete => hook_activation::verify(activation, true)?,
            InstallStatus::Unapplying if record.completed == 0 => {
                hook_activation::verify_transition(activation)?;
            }
            InstallStatus::Unapplying | InstallStatus::Uninstalled => {
                hook_activation::verify(activation, false)?;
            }
        }
    }
    Ok(())
}

/// Verify a completed install before deriving a rebuilt-binary refresh.
///
/// Every receipt-owned destination remains strict except the executable. A
/// regular executable under the same recorded parent may carry different
/// bytes or mode: the returned image becomes the exact superseded side of the
/// refresh plan, so its digest admits that reconciliation and apply still
/// compare-and-swaps against what preview observed.
pub(super) fn verify_refresh_progress(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<Option<GeneratedImage>, InstallError> {
    record.validate_loaded_for_repository(repository)?;
    if record.status != InstallStatus::Complete {
        verify_progress(repository, record)?;
        return Ok(None);
    }

    let mut binary_superseded = None;
    for (index, operation) in record.plan.operations.iter().enumerate() {
        let (observed, parent) = observe_bound(
            operation.location.root(repository),
            &operation.destination,
            &operation.parent,
        )?;
        if installed_matches(
            observed.as_ref(),
            operation,
            record.installed[index],
            parent,
            record.installed_parents[index],
        ) {
            continue;
        }
        if operation.asset == Asset::Binary
            && parent == record.installed_parents[index]
            && parent.is_some()
            && let Some(observed) = observed.as_ref()
        {
            binary_superseded = Some(observed.generated());
            continue;
        }
        return Err(drift(operation, observed));
    }
    if let Some(activation) = &record.plan.hook_activation {
        hook_activation::verify_refreshable(activation)?;
    }
    Ok(binary_superseded)
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
