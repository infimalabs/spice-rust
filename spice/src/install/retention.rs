//! Bounded app generation history carried from one install to the next.

use wire::Digest;

use super::binding::bind_destination;
use super::{
    InstallError, InstallPlan, ParentObservation, RETAINED_APP_PREDECESSORS, Repository,
    Retirement, app,
};

/// Generations a plan publishing `current` retains before it, oldest first.
///
/// The installed current generation is always the newest predecessor. A
/// receipt's ordered predecessor inventory carries forward until the bound
/// drops the oldest generation.
pub(super) fn predecessors(installed: Option<&InstallPlan>, current: &Digest) -> Vec<Digest> {
    let mut retained = installed
        .into_iter()
        .flat_map(|plan| plan.app_predecessors.iter().chain(&plan.app))
        .filter(|generation| *generation != current)
        .cloned()
        .collect::<Vec<_>>();
    let excess = retained.len().saturating_sub(RETAINED_APP_PREDECESSORS);
    retained.drain(..excess);
    retained
}

/// Discard every installed file of a generation that is neither `current` nor
/// retained, in generation then asset order.
///
/// Each witness is the image the superseded receipt installed, so apply
/// retires only files still carrying Spice's own bytes. The last retirement of
/// a generation also withdraws its then-empty directory.
pub(super) fn retirements(
    repository: &Repository,
    installed: Option<&InstallPlan>,
    current: &Digest,
    predecessors: &[Digest],
) -> Result<Vec<Retirement>, InstallError> {
    let mut retired = installed
        .into_iter()
        .flat_map(|plan| &plan.operations)
        .filter_map(|operation| {
            let generation = app::operation_generation(operation.asset, &operation.path)?;
            (generation != *current && !predecessors.contains(&generation))
                .then_some((generation, operation))
        })
        .collect::<Vec<_>>();
    retired.sort_by(|(left, left_operation), (right, right_operation)| {
        left.cmp(right)
            .then(left_operation.asset.cmp(&right_operation.asset))
    });
    let mut bound: Option<(&Digest, String, ParentObservation)> = None;
    let mut retirements = Vec::with_capacity(retired.len());
    for (generation, operation) in &retired {
        if bound
            .as_ref()
            .is_none_or(|(prior, ..)| *prior != generation)
        {
            let (directory, parent) = generation_binding(repository, generation.as_str())?;
            bound = Some((generation, directory, parent));
        }
        let (_, directory, parent) = bound.as_ref().expect("bound above");
        let file = app::file(operation.asset).expect("a generation operation names an app file");
        retirements.push(Retirement {
            path: operation.path.clone(),
            destination: format!("{directory}/{file}"),
            parent: parent.clone(),
            previous: None,
            observed: None,
            superseded: Some(operation.generated.clone()),
            discard: true,
        });
    }
    Ok(retirements)
}

/// Current binding of one retirement, as planning derives it.
///
/// A retired generation's files bind below the generations directory with the
/// generation directory itself as the missing suffix, so the binding stays
/// valid after that directory is withdrawn and names exactly the one directory
/// the last retirement may remove.
pub(super) fn retirement_binding(
    repository: &Repository,
    retirement: &Retirement,
) -> Result<(String, ParentObservation), InstallError> {
    let generated = retirement
        .discard
        .then(|| app::generation_file(&retirement.path))
        .flatten();
    let Some((generation, file)) = generated else {
        let bound = bind_destination(repository.common_dir(), &retirement.path)?;
        return Ok((bound.destination, bound.parent));
    };
    let (directory, parent) = generation_binding(repository, generation)?;
    Ok((format!("{directory}/{file}"), parent))
}

fn generation_binding(
    repository: &Repository,
    generation: &str,
) -> Result<(String, ParentObservation), InstallError> {
    let logical = format!("{}/{generation}", app::GENERATIONS_PATH);
    let bound = bind_destination(repository.common_dir(), &logical)?;
    if !bound.parent.missing.is_empty() {
        return Err(InstallError::ParentChanged(
            repository.common_dir().join(logical),
        ));
    }
    Ok((
        bound.destination,
        ParentObservation {
            ancestor: bound.parent.ancestor,
            generation: bound.parent.generation,
            missing: generation.to_owned(),
        },
    ))
}
