//! Every execution task whose current wall needs operator intervention.
//!
//! Environment holds and dependency walls belong here. Normal Plan/Design
//! sequencing and Design selection remain reachable through their ordinary
//! Work surfaces without entering the danger queue.

use super::{DependencyWall, WorkAvailability, WorkProjection, wall::DependencyWalls};

/// One task only the operator can move, and why.
#[derive(Debug, PartialEq, Eq)]
pub struct OperatorWait<'a> {
    task: usize,
    environment: bool,
    wall: Option<DependencyWall<'a>>,
    holds: Vec<usize>,
}

impl<'a> OperatorWait<'a> {
    const fn new(task: usize) -> Self {
        Self {
            task,
            environment: false,
            wall: None,
            holds: Vec::new(),
        }
    }

    /// The waiting task's index in the projection.
    #[must_use]
    pub const fn task(&self) -> usize {
        self.task
    }

    /// Its environment needs an act only the operator can take.
    #[must_use]
    pub const fn environment(&self) -> bool {
        self.environment
    }

    /// The wall it presents to the tasks it holds when its environment does
    /// not explain it.
    #[must_use]
    pub const fn wall(&self) -> Option<DependencyWall<'a>> {
        self.wall
    }

    /// Projection indexes of the tasks the prerequisite gate holds behind
    /// this one, in board order.
    #[must_use]
    pub fn holds(&self) -> &[usize] {
        &self.holds
    }
}

/// The board's operator-waiting tasks in board order.
#[derive(Debug, PartialEq, Eq)]
pub struct OperatorQueue<'a> {
    waits: Vec<OperatorWait<'a>>,
}

impl<'a> OperatorQueue<'a> {
    #[must_use]
    pub fn new(work: &WorkProjection<'a>) -> Self {
        let tasks = work.tasks();
        let mut waits = (0..tasks.len()).map(|_| None).collect::<Vec<_>>();
        for (index, task) in tasks.iter().enumerate() {
            if task.environment_blocked() && !set_aside(task.availability()) {
                waits[index]
                    .get_or_insert_with(|| OperatorWait::new(index))
                    .environment = true;
            }
        }
        let mut walls = DependencyWalls::new(work);
        for (index, task) in tasks.iter().enumerate() {
            if task.availability() != WorkAvailability::Blocked {
                continue;
            }
            for dependency in task.dependencies() {
                let Some((at, wall)) = walls.wall(index, dependency) else {
                    continue;
                };
                let wait = waits[at].get_or_insert_with(|| OperatorWait::new(at));
                match wall {
                    DependencyWall::Environment(_) => wait.environment = true,
                    _ => wait.wall = Some(wall),
                }
                if wait.holds.last() != Some(&index) {
                    wait.holds.push(index);
                }
            }
        }
        Self {
            waits: waits.into_iter().flatten().collect(),
        }
    }

    #[must_use]
    pub fn waits(&self) -> &[OperatorWait<'a>] {
        &self.waits
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.waits.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    /// The waiting task to open after `current`: the one following it in
    /// board order, wrapping to the first, or the first when `current` is
    /// absent or not waiting. `None` only when nothing waits.
    #[must_use]
    pub fn next_after(&self, current: Option<usize>) -> Option<usize> {
        let following = current
            .and_then(|current| self.waits.iter().position(|wait| wait.task == current))
            .map_or(0, |position| position + 1);
        self.waits
            .get(following)
            .or_else(|| self.waits.first())
            .map(OperatorWait::task)
    }
}

/// The operator already set this work aside, so it waits on nothing for its
/// own sake.
const fn set_aside(availability: WorkAvailability) -> bool {
    matches!(
        availability,
        WorkAvailability::Terminal
            | WorkAvailability::Draft
            | WorkAvailability::Parked
            | WorkAvailability::Deferred
            | WorkAvailability::Passive
    )
}

#[cfg(test)]
mod tests;
