//! Which unsatisfied dependencies only the operator can move.
//!
//! A dependency on work an agent can claim or is already working resolves
//! organically, so it is an ordinary wait. A chain is a wall only when every
//! path forward needs the operator: a deferral, a parked hidden row, a cycle,
//! a prerequisite that can never complete, or an environment requirement only
//! the operator can satisfy.

use wire::TaskStamp;

use super::{WorkAvailability, WorkDependency, WorkProjection};

/// The task where an unsatisfied dependency chain stops moving, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyWall<'a> {
    /// The task carries a deferral.
    Deferred(&'a TaskStamp),
    /// The task sits in a hidden project until the operator promotes it.
    Parked(&'a TaskStamp),
    /// The task depends on itself through the chain.
    Cycle(&'a TaskStamp),
    /// The task was closed without completing.
    Unreachable(&'a TaskStamp),
    /// The task needs an environment requirement only the operator can satisfy.
    Environment(&'a TaskStamp),
}

/// Memoized wall search over one projection.
///
/// A task's verdict holds regardless of the path that reached it: a cycle
/// found below a task always runs back through that task.
pub struct DependencyWalls<'p, 'a> {
    work: &'p WorkProjection<'a>,
    verdicts: Vec<Option<Option<DependencyWall<'a>>>>,
    on_path: Vec<bool>,
}

impl<'p, 'a> DependencyWalls<'p, 'a> {
    #[must_use]
    pub fn new(work: &'p WorkProjection<'a>) -> Self {
        let tasks = work.tasks().len();
        Self {
            work,
            verdicts: vec![None; tasks],
            on_path: vec![false; tasks],
        }
    }

    /// The wall behind one dependency of task `dependent`, or none when the
    /// dependency is satisfied or agent work will move it.
    pub fn wall(
        &mut self,
        dependent: usize,
        dependency: &WorkDependency<'a>,
    ) -> Option<DependencyWall<'a>> {
        self.on_path[dependent] = true;
        let wall = self.dependency(dependency);
        self.on_path[dependent] = false;
        wall
    }

    fn dependency(&mut self, dependency: &WorkDependency<'a>) -> Option<DependencyWall<'a>> {
        if dependency.satisfied() {
            return None;
        }
        let task = dependency.task()?;
        if self.on_path[task] {
            return Some(DependencyWall::Cycle(dependency.born()));
        }
        if let Some(verdict) = self.verdicts[task] {
            return verdict;
        }
        let verdict = self.task(task);
        self.verdicts[task] = Some(verdict);
        verdict
    }

    fn task(&mut self, index: usize) -> Option<DependencyWall<'a>> {
        let work = self.work;
        let task = &work.tasks()[index];
        let born = &task.row().statement.body.born;
        match task.availability() {
            WorkAvailability::InFlight
            | WorkAvailability::Ready
            | WorkAvailability::Review
            | WorkAvailability::Stale => None,
            WorkAvailability::Held => Some(DependencyWall::Environment(born)),
            WorkAvailability::Deferred => Some(DependencyWall::Deferred(born)),
            WorkAvailability::Parked => Some(DependencyWall::Parked(born)),
            WorkAvailability::Terminal => Some(DependencyWall::Unreachable(born)),
            WorkAvailability::Blocked if task.environment_blocked() => {
                Some(DependencyWall::Environment(born))
            }
            WorkAvailability::Blocked => {
                self.on_path[index] = true;
                let wall = task
                    .dependencies()
                    .iter()
                    .find_map(|dependency| self.dependency(dependency));
                self.on_path[index] = false;
                wall
            }
        }
    }
}

#[cfg(test)]
mod tests;
