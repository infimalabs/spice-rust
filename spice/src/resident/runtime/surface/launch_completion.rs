//! Resident-owned completion of off-loop supervisor proof establishment.

use super::*;

#[allow(
    clippy::too_many_arguments,
    reason = "one launch completion rejoins every resident authority it can settle"
)]
pub(super) async fn accept(
    bus: &Bus,
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    reconciliation: &mut lifecycle::Reconciliation,
    timers: &mut timers::Runtime,
    launches: &mut super::super::launch_runtime::Runtime,
    door: &mut capture::SupervisorReports,
    diagnostic: &mut dyn io::Write,
    completed: super::super::launch_runtime::Completion,
) -> CommandResult<()> {
    match completed.authority {
        super::super::launch_runtime::Authority::Command {
            request,
            completion,
            lane,
            lifecycle_released,
            command,
            epoch,
        } => {
            lifecycle.finish_external_launch(lane);
            let outcome = command_surface.complete_command_launch(
                seats,
                request.command(),
                completion,
                completed.result,
            );
            if lifecycle_released {
                let position = match &outcome {
                    Outcome::Reply(reply) => Some(reply.position),
                    Outcome::Refusal(_) | Outcome::Fault(_) => None,
                };
                reconciliation.request(lifecycle::ReconciliationWitness::new(
                    command, epoch, position,
                ));
            }
            complete_response(request, outcome).await;
            reconcile_for_control(
                reconciliation,
                bus,
                command_surface,
                seats,
                diagnostic,
                timers,
                lifecycle,
                launches,
                door,
            )
        }
        super::super::launch_runtime::Authority::Automatic(pending) => {
            command_surface.complete_automatic_launch(
                lifecycle,
                seats,
                diagnostic,
                pending,
                completed.result,
            )?;
            reconcile_wakes(
                bus,
                command_surface,
                seats,
                diagnostic,
                timers,
                lifecycle,
                launches,
                door,
            )
        }
    }
}
