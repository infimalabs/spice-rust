use super::record::{
    InstallRecord, LoadedRecord, RefreshRecord, read_record_for_refresh, write_initial_record,
    write_record,
};
use super::*;

pub(super) fn apply_with_interruption(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    interrupt_after_write: Option<usize>,
) -> Result<InstallReceipt, InstallError> {
    let (mut record, record_generation, fresh) = prepare_apply(repository, spice_binary, expected)?;
    reclaim_temporary_records(repository)?;
    if record.status == InstallStatus::Complete {
        return Ok(record.receipt());
    }
    let _rollback_lock = record.plan.lock_rollback(repository)?;
    let generated = materialize_operations(&record, spice_binary)?;
    if fresh {
        record.plan.validate_pristine(repository)?;
    } else {
        record.plan.validate_for_repository(repository)?;
    }
    record.plan.publish_staged_rollback(repository)?;
    let mut record_generation = match record_generation {
        Some(generation) => write_record(repository, &record, generation)?,
        None => write_initial_record(repository, &record)?,
    };
    record_generation = apply_file_operations(
        repository,
        &mut record,
        &generated,
        record_generation,
        interrupt_after_write,
    )?;
    record_generation = apply_retirements(
        repository,
        &mut record,
        record_generation,
        interrupt_after_write,
    )?;
    record_generation = apply_activation(
        repository,
        &mut record,
        record_generation,
        interrupt_after_write,
    )?;
    record.status = InstallStatus::Complete;
    write_record(repository, &record, record_generation)?;
    Ok(record.receipt())
}

fn apply_file_operations(
    repository: &Repository,
    record: &mut InstallRecord,
    generated: &[FileImage],
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<ReceiptGeneration, InstallError> {
    while record.completed < record.plan.operations.len() {
        let index = record.completed;
        record_generation = apply_operation(
            repository,
            record,
            &generated[index],
            index,
            record_generation,
        )?;
        if interrupt_after_write == Some(index) {
            return Err(InstallError::Interrupted { operation: index });
        }
        record.completed += 1;
        record_generation = write_record(repository, record, record_generation)?;
    }
    Ok(record_generation)
}

fn apply_retirements(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<ReceiptGeneration, InstallError> {
    while record.completed < record.plan.operations.len() + record.plan.retirements.len() {
        let index = record.completed - record.plan.operations.len();
        let step = record.completed;
        if record.plan.retirements[index].discards_payload()
            && record.retired_parents[index].is_none()
        {
            record.retired_parents[index] =
                prepare_retirement(repository, &record.plan.retirements[index], index)?;
            record_generation = write_record(repository, record, record_generation)?;
        }
        let parent = retire_path(repository, &record.plan.retirements[index], index)?;
        record.retired_parents[index] = parent;
        record_generation = write_record(repository, record, record_generation)?;
        if interrupt_after_write == Some(step) {
            return Err(InstallError::Interrupted { operation: step });
        }
        record.completed += 1;
        record_generation = write_record(repository, record, record_generation)?;
    }
    Ok(record_generation)
}

fn materialize_operations(
    record: &InstallRecord,
    spice_binary: Option<&Path>,
) -> Result<Vec<FileImage>, InstallError> {
    record
        .plan
        .operations
        .iter()
        .map(|operation| {
            materialize_generated(operation, spice_binary, &record.plan.repository.common_git)
        })
        .collect()
}

fn apply_activation(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<ReceiptGeneration, InstallError> {
    let activation_step = record.plan.operations.len() + record.plan.retirements.len();
    if record.plan.hook_activation.is_some() && record.completed == activation_step {
        let index = record.completed;
        hook_activation::apply(repository, &record.plan)?;
        if interrupt_after_write == Some(index) {
            return Err(InstallError::Interrupted { operation: index });
        }
        record.completed += 1;
        record_generation = write_record(repository, record, record_generation)?;
    }
    Ok(record_generation)
}

fn prepare_apply(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
) -> Result<(InstallRecord, Option<ReceiptGeneration>, bool), InstallError> {
    match read_record_for_refresh(repository)? {
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
    refresh: RefreshRecord,
) -> Result<(InstallRecord, Option<ReceiptGeneration>, bool), InstallError> {
    let repairable = matches!(refresh, RefreshRecord::Repairable { .. });
    let loaded = refresh.into_loaded();
    let LoadedRecord {
        record,
        generation,
        image,
    } = loaded;
    if record.status == InstallStatus::Uninstalled {
        return prepare_reinstall(repository, spice_binary, expected, generation, record);
    }
    if record.status == InstallStatus::Complete
        && (repairable || refresh_required(repository, &record.plan, spice_binary)?)
    {
        return prepare_refresh(
            repository,
            spice_binary,
            expected,
            generation,
            image,
            record,
        );
    }
    prepare_resume(repository, spice_binary, expected, generation, record)
}

fn prepare_reinstall(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    generation: ReceiptGeneration,
    record: InstallRecord,
) -> Result<(InstallRecord, Option<ReceiptGeneration>, bool), InstallError> {
    verify_progress(repository, &record)?;
    let plan = build_plan(repository, spice_binary)?;
    admit_digest(&plan, expected)?;
    Ok((InstallRecord::new(plan)?, Some(generation), true))
}

fn prepare_refresh(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    generation: ReceiptGeneration,
    prior_receipt: Option<FileImage>,
    record: InstallRecord,
) -> Result<(InstallRecord, Option<ReceiptGeneration>, bool), InstallError> {
    // One record swap derives a complete current plan before any destination
    // changes. Binary-only drift is the sole reconciled input; fixed assets
    // remain receipt-strict.
    let binary_superseded = verify_refresh_progress(repository, &record)?;
    let rollback_record = (record.plan.version == MIGRATABLE_RECORD_VERSION).then(|| {
        prior_receipt
            .as_ref()
            .expect("only a version-7 receipt retains its source image")
    });
    let rollback_archive = (record.plan.version != MIGRATABLE_RECORD_VERSION)
        .then_some(record.plan.rollback_archive.as_ref())
        .flatten();
    let plan = build_plan_over(
        repository,
        spice_binary,
        Some(&record.plan),
        binary_superseded.as_ref(),
        rollback_record,
        rollback_archive,
    )?;
    admit_digest(&plan, expected)?;
    Ok((InstallRecord::new(plan)?, Some(generation), true))
}

fn prepare_resume(
    repository: &Repository,
    spice_binary: Option<&Path>,
    expected: &Digest,
    generation: ReceiptGeneration,
    record: InstallRecord,
) -> Result<(InstallRecord, Option<ReceiptGeneration>, bool), InstallError> {
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
            Ok((record, Some(generation), false))
        }
        InstallStatus::Unapplying => Err(InstallError::UnapplyInProgress),
        InstallStatus::Uninstalled => unreachable!("handled above"),
        InstallStatus::Applying => {
            verify_progress(repository, &record)?;
            Ok((record, Some(generation), false))
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
    record_generation: ReceiptGeneration,
) -> Result<ReceiptGeneration, InstallError> {
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
    if record.version != RECORD_VERSION || record.plan.version != PLAN_VERSION {
        return Err(InstallError::InvalidRecord(format!(
            "version-{} install from RETIREM-1kSR91Zl must be refreshed to version {RECORD_VERSION} before unapply",
            record.version,
        )));
    }
    let mut record_generation = loaded.generation;
    let _rollback_lock = record.plan.lock_rollback(repository)?;
    let rollback = record.plan.validated_rollback_state(repository)?;
    // Reversal is an executable direction, not historical inspection. Finish
    // it with the build that owns the live receipt; a newer build should derive
    // and apply its refresh instead of withdrawing the appliance.
    record.validate_for_repository(repository)?;
    match record.status {
        InstallStatus::Applying => return Err(InstallError::ApplyInProgress),
        InstallStatus::Uninstalled => {
            verify_progress_with_rollback(repository, &record, rollback.as_ref())?;
            return Ok(record.receipt());
        }
        InstallStatus::Complete => {
            verify_progress_with_rollback(repository, &record, rollback.as_ref())?;
            record.status = InstallStatus::Unapplying;
            record.completed = 0;
            record.restored.fill(None);
            record.restored_retirements.fill(None);
            record_generation = write_record(repository, &record, record_generation)?;
        }
        InstallStatus::Unapplying => {
            verify_progress_with_rollback(repository, &record, rollback.as_ref())?;
        }
    }

    record_generation = reverse_operations(
        repository,
        &mut record,
        rollback.as_ref(),
        record_generation,
        interrupt_after_write,
    )?;
    record.status = InstallStatus::Uninstalled;
    let receipt = record.receipt();
    if let Some(rollback) = &rollback {
        publish_bound(
            repository.common_dir(),
            &record.plan.record.destination,
            &record.plan.record.parent,
            0,
            &rollback.record,
            Some(record_generation),
        )?;
    } else {
        write_record(repository, &record, record_generation)?;
    }
    Ok(receipt)
}

fn reverse_operations(
    repository: &Repository,
    record: &mut InstallRecord,
    rollback: Option<&RollbackState>,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<ReceiptGeneration, InstallError> {
    while record.completed < record.plan.operation_count() {
        if let Some(generation) = reverse_activation(
            repository,
            record,
            record_generation.clone(),
            interrupt_after_write,
        )? {
            record_generation = generation;
            continue;
        }
        if let Some(generation) = reverse_retirement(
            repository,
            record,
            rollback,
            record_generation.clone(),
            interrupt_after_write,
        )? {
            record_generation = generation;
            continue;
        }
        record_generation = reverse_file_operation(
            repository,
            record,
            rollback,
            record_generation,
            interrupt_after_write,
        )?;
    }
    Ok(record_generation)
}

fn reverse_retirement(
    repository: &Repository,
    record: &mut InstallRecord,
    rollback: Option<&RollbackState>,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<Option<ReceiptGeneration>, InstallError> {
    let activation_offset = usize::from(record.plan.hook_activation.is_some());
    let completed = record.completed.saturating_sub(activation_offset);
    if completed >= record.plan.retirements.len() {
        return Ok(None);
    }
    let index = record.plan.retirements.len() - 1 - completed;
    let retirement = &record.plan.retirements[index];
    let previous = retirement_previous(retirement, rollback, index);
    let (current, parent) = observe_bound(
        repository.common_dir(),
        &retirement.destination,
        &retirement.parent,
    )?;
    let already_restored = image_matches(current.as_ref(), previous);
    let installed = current.is_none() && parent == record.retired_parents[index];
    if !already_restored && !installed {
        return Err(retirement_drift(retirement, current));
    }
    if !already_restored {
        record.restored_retirements[index] = restore_retirement(repository, retirement, previous)?;
    } else {
        record.restored_retirements[index] = current.map(|observed| observed.generation);
    }
    record_generation = write_record(repository, record, record_generation)?;
    let reverse = record.plan.operations.len() + index;
    if interrupt_after_write == Some(reverse) {
        return Err(InstallError::Interrupted { operation: reverse });
    }
    record.completed += 1;
    record_generation = write_record(repository, record, record_generation)?;
    Ok(Some(record_generation))
}

fn reverse_activation(
    repository: &Repository,
    record: &mut InstallRecord,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<Option<ReceiptGeneration>, InstallError> {
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
    rollback: Option<&RollbackState>,
    mut record_generation: ReceiptGeneration,
    interrupt_after_write: Option<usize>,
) -> Result<ReceiptGeneration, InstallError> {
    let non_file_offset =
        usize::from(record.plan.hook_activation.is_some()) + record.plan.retirements.len();
    let file_completed = record.completed - non_file_offset;
    let reverse = record.plan.operations.len() - 1 - file_completed;
    let operation = &record.plan.operations[reverse];
    let previous = operation_previous(operation, rollback, reverse);
    let (current, parent) = observe_bound(
        operation.location.root(repository),
        &operation.destination,
        &operation.parent,
    )?;
    let (restored, needs_cleanup) = restored_image(
        repository,
        record,
        operation,
        previous,
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
    previous: Option<&FileImage>,
    reverse: usize,
    current: Option<&ObservedFile>,
    parent: Option<Generation>,
) -> Result<(Option<Generation>, bool), InstallError> {
    if restored_matches(current, previous, record.restored[reverse]) {
        return Ok((record.restored[reverse], true));
    }
    if restoration_image_matches(current, previous) {
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
        return Ok((
            restore_operation(repository, operation, generation, previous)?,
            false,
        ));
    }
    Err(drift(operation, current.cloned()))
}

pub(super) fn verify_progress(
    repository: &Repository,
    record: &InstallRecord,
) -> Result<(), InstallError> {
    let rollback = match record.status {
        InstallStatus::Complete => None,
        InstallStatus::Applying | InstallStatus::Unapplying | InstallStatus::Uninstalled => {
            record.plan.validated_rollback_state(repository)?
        }
    };
    verify_progress_with_rollback(repository, record, rollback.as_ref())
}

fn verify_progress_with_rollback(
    repository: &Repository,
    record: &InstallRecord,
    rollback: Option<&RollbackState>,
) -> Result<(), InstallError> {
    record.validate_loaded_for_repository(repository)?;
    verify_file_progress(repository, record, rollback, reversed_file_count(record))?;
    verify_retirement_progress(repository, record, rollback)?;
    verify_activation_progress(record)
}

fn reversed_file_count(record: &InstallRecord) -> usize {
    match record.status {
        InstallStatus::Unapplying => record.completed.saturating_sub(
            usize::from(record.plan.hook_activation.is_some()) + record.plan.retirements.len(),
        ),
        _ => record.completed.min(record.plan.operations.len()),
    }
}

fn verify_retirement_progress(
    repository: &Repository,
    record: &InstallRecord,
    rollback: Option<&RollbackState>,
) -> Result<(), InstallError> {
    let applying_completed = record
        .completed
        .saturating_sub(record.plan.operations.len())
        .min(record.plan.retirements.len());
    let activation_offset = usize::from(record.plan.hook_activation.is_some());
    let reversed = record
        .completed
        .saturating_sub(activation_offset)
        .min(record.plan.retirements.len());
    for (index, retirement) in record.plan.retirements.iter().enumerate() {
        let previous = retirement_previous(retirement, rollback, index);
        let (observed, parent) = observe_bound(
            repository.common_dir(),
            &retirement.destination,
            &retirement.parent,
        )?;
        let absent = observed.is_none() && parent == record.retired_parents[index];
        let prior = image_matches(observed.as_ref(), previous);
        let planned = retirement.expects_in_place(observed.as_ref());
        let valid = match record.status {
            InstallStatus::Applying if index < applying_completed => absent,
            InstallStatus::Applying if index == applying_completed => planned || absent,
            InstallStatus::Applying => planned,
            InstallStatus::Complete => observed.is_none(),
            InstallStatus::Unapplying => {
                let reverse = record.plan.retirements.len() - 1 - index;
                if reverse < reversed {
                    prior
                } else if reverse == reversed {
                    absent || prior
                } else {
                    absent
                }
            }
            InstallStatus::Uninstalled => prior,
        };
        if !valid {
            return Err(retirement_drift(retirement, observed));
        }
    }
    Ok(())
}

fn verify_file_progress(
    repository: &Repository,
    record: &InstallRecord,
    rollback: Option<&RollbackState>,
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
            operation_previous(operation, rollback, index),
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
    previous: Option<&FileImage>,
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
        InstallStatus::Unapplying => unapplying_file_valid(
            record,
            operation,
            previous,
            index,
            observed,
            parent,
            file_completed,
        ),
        InstallStatus::Uninstalled => restored_matches(observed, previous, record.restored[index]),
    }
}

fn unapplying_file_valid(
    record: &InstallRecord,
    operation: &Operation,
    previous: Option<&FileImage>,
    index: usize,
    observed: Option<&ObservedFile>,
    parent: Option<Generation>,
    file_completed: usize,
) -> bool {
    let reversed = record.plan.operations.len() - 1 - index;
    if reversed < file_completed {
        return restored_matches(observed, previous, record.restored[index]);
    }
    if reversed == file_completed {
        return restored_matches(observed, previous, record.restored[index])
            || restoration_image_matches(observed, previous)
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
            InstallStatus::Applying
                if record.completed
                    < record.plan.operations.len() + record.plan.retirements.len() =>
            {
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
    verify_retirement_progress(repository, record, None)?;
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
    previous: Option<&FileImage>,
    generation: Option<Generation>,
) -> bool {
    match (previous, observed, generation) {
        (None, None, None) => true,
        (Some(previous), Some(observed), Some(_)) => observed.image == *previous,
        _ => false,
    }
}

fn restoration_image_matches(
    observed: Option<&ObservedFile>,
    previous: Option<&FileImage>,
) -> bool {
    match (previous, observed) {
        (None, None) => true,
        (Some(previous), Some(observed)) => observed.image == *previous,
        _ => false,
    }
}

fn operation_previous<'a>(
    operation: &'a Operation,
    rollback: Option<&'a RollbackState>,
    index: usize,
) -> Option<&'a FileImage> {
    match rollback {
        Some(rollback) => rollback.operations[index].as_ref(),
        None => operation.previous.as_ref().map(|previous| &previous.image),
    }
}

fn retirement_previous<'a>(
    retirement: &'a Retirement,
    rollback: Option<&'a RollbackState>,
    index: usize,
) -> Option<&'a FileImage> {
    match rollback {
        Some(rollback) => rollback.retirements[index].as_ref(),
        None => retirement.previous.as_ref().map(|previous| &previous.image),
    }
}

fn image_matches(observed: Option<&ObservedFile>, expected: Option<&FileImage>) -> bool {
    match (observed, expected) {
        (None, None) => true,
        (Some(observed), Some(expected)) => observed.image == *expected,
        _ => false,
    }
}
