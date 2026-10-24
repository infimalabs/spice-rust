//! Fair resident command dispatch, including deferred launch replies.

use super::*;
pub(in crate::resident) enum ControlDeliveryTurn {
    Control,
    Delivery,
}

pub(in crate::resident) async fn control_delivery_turn(
    control: &mut serve::axum::Control,
    inbox_deliveries: &InboxDeliveries,
    control_enabled: bool,
) -> (ControlDeliveryTurn, Option<serve::axum::ControlRequest>) {
    if inbox_deliveries.is_empty() {
        return if control_enabled {
            (ControlDeliveryTurn::Control, control.recv().await)
        } else {
            std::future::pending().await
        };
    }
    if !control_enabled {
        return (ControlDeliveryTurn::Delivery, None);
    }
    tokio::select! {
        biased;
        request = control.recv() => (ControlDeliveryTurn::Control, request),
        () = std::future::ready(()) => (ControlDeliveryTurn::Delivery, None),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a delivery burst borrows each resident runtime a control command can advance"
)]
pub(super) async fn dispatch_control_delivery_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut serve::axum::Control,
    request: Option<serve::axum::ControlRequest>,
    inbox_deliveries: &mut InboxDeliveries,
    lifecycle_reconciliation: &mut lifecycle::Reconciliation,
    launches: &mut super::super::launch_runtime::Runtime,
    live_changes: &lane_surface::LiveChanges,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<bool> {
    let dispatched = dispatch_control_burst(
        command_surface,
        seats,
        lifecycle,
        control,
        request,
        inbox_deliveries,
        launches,
        live_changes,
        diagnostic,
    )
    .await?;
    if let Some(witness) = dispatched.lifecycle_witness {
        lifecycle_reconciliation.request(witness);
    }
    if !dispatched.reached_dark {
        deliver_owed_inbox(command_surface, inbox_deliveries).await?;
    }
    Ok(dispatched.reached_dark)
}

pub(super) struct ControlDispatch {
    pub(super) reached_dark: bool,
    pub(super) lifecycle_witness: Option<lifecycle::ReconciliationWitness>,
    pub(super) inbox_lanes: Vec<CanonicalUuid>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "a control command borrows each resident runtime it can advance"
)]
pub(super) async fn dispatch_control(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut serve::axum::Control,
    request: Option<serve::axum::ControlRequest>,
    launches: &mut super::super::launch_runtime::Runtime,
    live_changes: &lane_surface::LiveChanges,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<ControlDispatch> {
    let request = request
        .ok_or_else(|| environment_fault("serve::control-closed", "app control receiver closed"))?;
    let replayed = command_surface.replay(seats, request.command());
    let is_replay = replayed.is_some();
    let dark = is_dark_command(request.command());
    if dark && !is_replay {
        launch_completion::settle_before_dark(
            command_surface,
            seats,
            lifecycle,
            launches,
            diagnostic,
        )
        .await?;
    }
    let lifecycle_released = if is_replay {
        false
    } else {
        match command_surface.prepare_explicit_lifecycle(lifecycle, request.command()) {
            Ok(released) => released,
            Err(Outcome::Refusal(refusal)) => {
                // A refused request leaves the resident owning every pending
                // launch and its eventual completion.
                complete_response(request, Outcome::Refusal(refusal)).await;
                return Ok(ControlDispatch {
                    reached_dark: false,
                    lifecycle_witness: None,
                    inbox_lanes: Vec::new(),
                });
            }
            Err(outcome) => return Err(outcome),
        }
    };
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
                inbox_lanes: Vec::new(),
            });
        }
    };
    live_changes.owe(effects.live_changes);
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
        inbox_lanes: effects.inbox_lanes,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "a control burst borrows each resident runtime a control command can advance"
)]
pub(super) async fn dispatch_control_burst(
    command_surface: &command_surface::Surface<'_>,
    seats: &mut Seats,
    lifecycle: &mut lifecycle::Runtime,
    control: &mut serve::axum::Control,
    request: Option<serve::axum::ControlRequest>,
    inbox_deliveries: &mut InboxDeliveries,
    launches: &mut super::super::launch_runtime::Runtime,
    live_changes: &lane_surface::LiveChanges,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<ControlDispatch> {
    let mut request = request;
    let mut burst = ControlDispatch {
        reached_dark: false,
        lifecycle_witness: None,
        inbox_lanes: Vec::new(),
    };
    for turn in 0..serve::axum::Control::FAIR_CYCLE_LIMIT {
        let dispatched = dispatch_control(
            command_surface,
            seats,
            lifecycle,
            control,
            request.take(),
            launches,
            live_changes,
            diagnostic,
        )
        .await?;
        for lane in dispatched.inbox_lanes {
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

pub(super) async fn complete_response(
    request: serve::axum::ControlRequest,
    outcome: Outcome<CommandAnswer>,
) {
    if let Ok(response) = request.respond(outcome) {
        response.wait().await;
    }
}

fn is_dark_command(command: &wire::AdmittedCommand) -> bool {
    matches!(
        command.body(),
        Intent::UpChange(wire::intent::UpChange {
            state: ResidentState::Dark
        })
    )
}
