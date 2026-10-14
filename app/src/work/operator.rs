//! Every task only the operator can move, across the whole board.
//!
//! A Design the resident holds for the operator's selection of its outputs
//! waits on the operator, whatever its standing or claim. A task whose
//! environment needs an operator act waits on the operator unless the
//! operator already set it aside (terminal, draft, parked, deferred, or a
//! passive Design). A task also waits on the operator when the dependency
//! chain of a task the prerequisite gate holds stops at it behind an
//! operator-only wall. Each waiting task appears once, in board order,
//! however many reasons and held tasks reach it, so stepping through the
//! queue is stable.

use super::{DependencyWall, WorkAvailability, WorkProjection, wall::DependencyWalls};

/// One task only the operator can move, and why.
#[derive(Debug, PartialEq, Eq)]
pub struct OperatorWait<'a> {
    task: usize,
    environment: bool,
    selection: bool,
    wall: Option<DependencyWall<'a>>,
    holds: Vec<usize>,
}

impl<'a> OperatorWait<'a> {
    const fn new(task: usize) -> Self {
        Self {
            task,
            environment: false,
            selection: false,
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

    /// It is a Design awaiting the operator's selection of its outputs.
    #[must_use]
    pub const fn selection(&self) -> bool {
        self.selection
    }

    /// The wall it presents to the tasks it holds when neither its
    /// environment nor a selection explains it.
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
            if task.awaiting_selection() {
                waits[index]
                    .get_or_insert_with(|| OperatorWait::new(index))
                    .selection = true;
            }
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
                    DependencyWall::Selection(_) => wait.selection = true,
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
