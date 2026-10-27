//! One open Design's pass cycle as every CLI surface presents it.
//!
//! The state is the resident's [`BoardAnswer::design_cycles`] judgment and
//! the text is [`wire::rooms::DesignCyclePresentation`], so task detail,
//! board rows, group reads, and command replies never derive readiness
//! themselves.

use super::*;

/// The holder's next command on an open Design: one proposal ends the pass.
pub(super) fn design_holder_next(handle: &TaskHandle) -> String {
    format!(
        "next: spice design propose JUSTIFICATION (assembles what this pass built and its \
         room; ends this pass of {handle} and releases its claim; only the operator concludes a \
         Design)"
    )
}

/// `task`'s cycle when `board` judged it an open Design.
pub(super) fn design_cycle(
    board: &BoardAnswer,
    task: &BoardTask,
) -> Option<wire::rooms::DesignCyclePresentation> {
    let cycle = *board.design_cycles.get(&task.statement.body.born)?;
    Some(wire::rooms::DesignCyclePresentation::new(
        cycle,
        task.design_cycle,
        &task.statement.body.handle,
        task.design_availability(),
    ))
}

/// The cycle lines of `task`, or none when `board` judged no open Design.
pub(super) fn design_cycle_lines(board: &BoardAnswer, task: &BoardTask) -> Vec<String> {
    design_cycle(board, task).map_or_else(Vec::new, |cycle| cycle.lines())
}

/// The exact action `board`'s cycle names for `task`, as a `next:` line.
pub(super) fn design_cycle_next(board: &BoardAnswer, task: &BoardTask) -> Option<String> {
    design_cycle(board, task).map(|cycle| format!("next: {}", cycle.action))
}
