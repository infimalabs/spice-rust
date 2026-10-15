//! Which unsatisfied dependencies only the operator can move.
//!
//! A dependency on work an agent can claim or is already working resolves
//! organically, so it is an ordinary wait. A chain is a wall only when every
//! path forward needs the operator: a deferral, typed OOPS work, a cycle,
//! a prerequisite that can never complete, an environment requirement only
//! the operator can satisfy, a passive Design, or a Design awaiting the
//! operator's selection.

use wire::TaskStamp;

use super::{WorkAvailability, WorkDependency, WorkProjection};

/// The task where an unsatisfied dependency chain stops moving, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyWall<'a> {
    /// The task carries a deferral.
    Deferred(&'a TaskStamp),
    /// The task remains OOPS or Draft work until it is filed.
    Parked(&'a TaskStamp),
    /// The task depends on itself through the chain.
    Cycle(&'a TaskStamp),
    /// The task was closed without completing.
    Unreachable(&'a TaskStamp),
    /// The task needs an environment requirement only the operator can satisfy.
    Environment(&'a TaskStamp),
    /// The task is a Design awaiting the operator's selection of its outputs.
    Selection(&'a TaskStamp),
    /// The task is a Design outside every working group.
    Passive(&'a TaskStamp),
}

/// Memoized wall search over one projection.
///
/// A task's verdict holds regardless of the path that reached it: a cycle
/// found below a task always runs back through that task.
pub(super) struct DependencyWalls<'p, 'a> {
    work: &'p WorkProjection<'a>,
    verdicts: Vec<Option<Option<(usize, DependencyWall<'a>)>>>,
    on_path: Vec<bool>,
}

impl<'p, 'a> DependencyWalls<'p, 'a> {
    #[must_use]
    pub(super) fn new(work: &'p WorkProjection<'a>) -> Self {
        let tasks = work.tasks().len();
        Self {
            work,
            verdicts: vec![None; tasks],
            on_path: vec![false; tasks],
        }
    }

    /// The wall behind one dependency of task `dependent` with the projection
    /// index of the task where it stands, or none when the dependency is
    /// satisfied or agent work will move it.
    pub(super) fn wall(
        &mut self,
        dependent: usize,
        dependency: &WorkDependency<'a>,
    ) -> Option<(usize, DependencyWall<'a>)> {
        self.on_path[dependent] = true;
        let wall = self.dependency(dependency);
        self.on_path[dependent] = false;
        wall
    }

    fn dependency(
        &mut self,
        dependency: &WorkDependency<'a>,
    ) -> Option<(usize, DependencyWall<'a>)> {
        if dependency.satisfied() {
            return None;
        }
        let task = dependency.task()?;
        if self.on_path[task] {
            return Some((task, DependencyWall::Cycle(dependency.born())));
        }
        if let Some(verdict) = self.verdicts[task] {
            return verdict;
        }
        let verdict = self.task(task);
        self.verdicts[task] = Some(verdict);
        verdict
    }

    fn task(&mut self, index: usize) -> Option<(usize, DependencyWall<'a>)> {
        let work = self.work;
        let task = &work.tasks()[index];
        let born = &task.row().statement.body.born;
        let wall = match task.availability() {
            WorkAvailability::InFlight
            | WorkAvailability::Ready
            | WorkAvailability::Review
            | WorkAvailability::Waiting
            | WorkAvailability::Selection
            | WorkAvailability::Stale => return None,
            WorkAvailability::Held => DependencyWall::Environment(born),
            WorkAvailability::Passive => DependencyWall::Passive(born),
            WorkAvailability::Deferred => DependencyWall::Deferred(born),
            WorkAvailability::Parked | WorkAvailability::Draft => DependencyWall::Parked(born),
            WorkAvailability::Terminal => DependencyWall::Unreachable(born),
            WorkAvailability::Blocked if task.environment_blocked() => {
                DependencyWall::Environment(born)
            }
            WorkAvailability::Blocked => {
                self.on_path[index] = true;
                let wall = task
                    .dependencies()
                    .iter()
                    .find_map(|dependency| self.dependency(dependency));
                self.on_path[index] = false;
                return wall;
            }
        };
        Some((index, wall))
    }
}

#[cfg(test)]
mod tests;
