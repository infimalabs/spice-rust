//! Fair resident command dispatch, including deferred launch replies.

use super::*;

pub(super) struct ControlDispatch {
    pub(super) reached_dark: bool,
    pub(super) lifecycle_witness: Option<lifecycle::ReconciliationWitness>,
    pub(super) inbox_delivery: Option<CanonicalUuid>,
}

pub(super) async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut serve::axum::Control,
    request: Option<serve::axum::ControlRequest>,
    launches: &mut super::super::launch_runtime::Runtime,
) -> CommandResult<ControlDispatch> {
    let request = request
        .ok_or_else(|| environment_fault("serve::control-closed", "app control receiver closed"))?;
    let replayed = command_surface.replay(seats, request.command());
    let is_replay = replayed.is_some();
    let lifecycle_released = if is_replay {
        false
    } else {
        command_surface.prepare_explicit_lifecycle(lifecycle, request.command())?
    };
    let dark = is_dark_command(request.command());
    let command = request.command().id();
    let epoch = request.command().epoch();
    let dispatched = replayed.map_or_else(
        || command_surface.dispatch_fresh(seats, request.command()),
        CommandDispatch::ordinary,
    );
    let (outcome, effects) = match dispatched {
        CommandDispatch::Complete { outcome, effects } => (outcome, effects),
        CommandDispatch::Launch(launch) => {
            lifecycle.begin_external_launch(launch.lane);
            launches.start_command(launch, request, lifecycle_released, command, epoch);
            return Ok(ControlDispatch {
                reached_dark: false,
                lifecycle_witness: None,
                inbox_delivery: None,
            });
        }
    };
    let lifecycle_witness = lifecycle_released.then(|| {
        let position = match &outcome {
            Outcome::Reply(reply) => Some(reply.position),
            Outcome::Refusal(_) | Outcome::Fault(_) => None,
        };
        lifecycle::ReconciliationWitness::new(command, epoch, position)
    });
    let reached_dark = !is_replay && dark && matches!(outcome, Outcome::Reply(_));
    complete_response(request, outcome).await;
    if reached_dark {
        control.close();
    }
    Ok(ControlDispatch {
        reached_dark,
        lifecycle_witness,
        inbox_delivery: effects.inbox_delivery,
    })
}

pub(super) async fn dispatch_control_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut serve::axum::Control,
    request: Option<serve::axum::ControlRequest>,
    inbox_deliveries: &mut InboxDeliveries,
    launches: &mut super::super::launch_runtime::Runtime,
) -> CommandResult<ControlDispatch> {
    let mut request = request;
    let mut burst = ControlDispatch {
        reached_dark: false,
        lifecycle_witness: None,
        inbox_delivery: None,
    };
    for turn in 0..serve::axum::Control::FAIR_CYCLE_LIMIT {
        let dispatched = dispatch_control(
            command_surface,
            seats,
            lifecycle,
            control,
            request.take(),
            launches,
        )
        .await?;
        if let Some(lane) = dispatched.inbox_delivery {
            inbox_deliveries.insert(lane);
        }
        burst.reached_dark |= dispatched.reached_dark;
        if dispatched.lifecycle_witness.is_some() {
            burst.lifecycle_witness = dispatched.lifecycle_witness;
        }
        if burst.reached_dark
            || burst.lifecycle_witness.is_some()
            || turn + 1 == serve::axum::Control::FAIR_CYCLE_LIMIT
        {
            break;
        }
        request = match control.try_recv() {
            Ok(request) => Some(request),
            Err(
                tokio::sync::mpsc::error::TryRecvError::Empty
                | tokio::sync::mpsc::error::TryRecvError::Disconnected,
            ) => break,
        };
    }
    Ok(burst)
}
