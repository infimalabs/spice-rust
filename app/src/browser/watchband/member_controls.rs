//! Lane controls inside one Dash member's disclosure.
//!
//! Every control is drawn only when the resident offers its command to this
//! session and the seat's current state admits it, so a missing control means
//! the action is unavailable here rather than disabled for an unstated reason.
//! Ending an occupancy or removing the seat confirms first.

use super::*;
use crate::runtime::{MemberCommand, MemberControlState};

/// Issue one member command: reserve it on the member, then queue it.
pub(super) fn command_callback(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    team: CanonicalUuid,
) -> Callback<(CanonicalUuid, MemberCommand)> {
    let sender = sender.clone();
    let surface = surface.clone();
    Callback::from(move |(lane, command): (CanonicalUuid, MemberCommand)| {
        issue(&sender, &surface, team, lane, command);
    })
}

fn issue(
    sender: &SenderHandle,
    surface: &UseStateHandle<SurfaceData>,
    team: CanonicalUuid,
    lane: CanonicalUuid,
    command: MemberCommand,
) {
    let reserved = {
        let mut data = surface.write();
        let Some(runtime) = data.as_mut() else {
            return;
        };
        let Some(topology_revision) = runtime.lanes().map(|lanes| lanes.topology_revision) else {
            return;
        };
        runtime
            .begin_member_command(lane, command)
            .map(|request| (request, topology_revision))
    };
    let Some((request, topology_revision)) = reserved else {
        return;
    };
    surface.set(surface.publication());
    let action = match request.command {
        MemberCommand::Wake | MemberCommand::Drain | MemberCommand::Remove => {
            ClientAction::LaneState {
                request: request.clone(),
            }
        }
        MemberCommand::Renew { requested } => ClientAction::RenewLane {
            topology_revision,
            team,
            lane,
            requested,
            member: Some(request.clone()),
        },
        MemberCommand::Retune { .. } => ClientAction::RetuneLane {
            topology_revision,
            request: request.clone(),
        },
    };
    if !admit_action(sender, action) {
        let mut data = surface.write();
        if let Some(runtime) = data.as_mut() {
            runtime.settle_member_command(
                &request,
                Some(format!(
                    "{}: Canvas could not queue the command.",
                    request.command.failure()
                )),
            );
        }
        drop(data);
        surface.set(surface.publication());
    }
}

/// Which member controls the session's authority admits.
struct Offered {
    lifecycle: bool,
    renew: bool,
    retune: bool,
}

impl Offered {
    fn of(runtime: &CanvasRuntime) -> Self {
        Self {
            lifecycle: runtime.offers(CommandKind::LaneState),
            renew: runtime.offers(CommandKind::LaneRenewal),
            retune: runtime.offers(CommandKind::LaneRetune),
        }
    }
}

fn close_confirmation(event: &MouseEvent) {
    if let Some(confirm) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|target| target.closest(".team-dash-confirm").ok().flatten())
    {
        let _removed = confirm.remove_attribute("open");
    }
}

fn send(
    command: &Callback<(CanonicalUuid, MemberCommand)>,
    lane: CanonicalUuid,
    next: MemberCommand,
) -> Callback<MouseEvent> {
    let command = command.clone();
    Callback::from(move |event: MouseEvent| {
        close_confirmation(&event);
        command.emit((lane, next.clone()));
    })
}

/// One control that asks before it acts.
fn confirmed(
    key: &'static str,
    label: &str,
    question: String,
    action: String,
    busy: bool,
    onclick: Callback<MouseEvent>,
) -> Html {
    html! {
        <details class="team-dash-confirm" data-member-control={key}>
            <summary class="team-mode">{label.to_owned()}</summary>
            <div class="team-dash-confirm-body">
                <p>{question}</p>
                <button type="button" class="team-mode" disabled={busy} {onclick}>{action}</button>
            </div>
        </details>
    }
}

#[derive(Properties, PartialEq)]
struct ModelFormProps {
    lane: CanonicalUuid,
    /// The seat's authoritative model spelling; absent follows the vendor.
    model: Option<String>,
    placeholder: String,
    busy: bool,
    command: Callback<(CanonicalUuid, MemberCommand)>,
}

/// The next-launch model editor.
///
/// The typed spelling is component state, so a surface re-render cannot
/// reset it; only a change to the seat's authoritative model replaces it.
#[function_component(ModelForm)]
fn model_form(props: &ModelFormProps) -> Html {
    let draft = use_state(|| props.model.clone().unwrap_or_default());
    {
        let draft = draft.clone();
        use_effect_with(props.model.clone(), move |model| {
            draft.set(model.clone().unwrap_or_default());
        });
    }
    let lane = props.lane;
    let id = format!("team-dash-model-{lane}");
    let oninput = {
        let draft = draft.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            draft.set(input.value());
        })
    };
    let submit = {
        let command = props.command.clone();
        let draft = draft.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let model = Some(draft.trim().to_owned()).filter(|model| !model.is_empty());
            command.emit((lane, MemberCommand::Retune { model }));
        })
    };
    let reset = props.model.is_some().then(|| {
        let command = props.command.clone();
        Callback::from(move |_: MouseEvent| {
            command.emit((lane, MemberCommand::Retune { model: None }));
        })
    });
    let busy = props.busy;
    html! {
        <form class="team-dash-model" data-member-control="model" onsubmit={submit}>
            <label for={id.clone()}>{"Next launch model"}</label>
            <input
                {id}
                type="text"
                spellcheck="false"
                autocomplete="off"
                value={(*draft).clone()}
                placeholder={props.placeholder.clone()}
                disabled={busy}
                {oninput}
            />
            <button type="submit" class="team-mode" disabled={busy}>{"Set model"}</button>
            if let Some(reset) = reset {
                <button type="button" class="team-mode" disabled={busy} onclick={reset}>
                    {"Use default"}
                </button>
            }
        </form>
    }
}

fn render_retune(
    runtime: &CanvasRuntime,
    member: &crate::runtime::LaneMember,
    busy: bool,
    command: &Callback<(CanonicalUuid, MemberCommand)>,
) -> Html {
    let placeholder = runtime
        .vendor_catalog()
        .and_then(|catalog| {
            catalog
                .vendors
                .iter()
                .find(|vendor| vendor.name == member.vendor)
        })
        .map_or_else(
            || "vendor default".to_owned(),
            |vendor| format!("{} (default)", vendor.default_model),
        );
    html! {
        <ModelForm
            key={member.lane.to_string()}
            lane={member.lane}
            model={member.seat_model.clone()}
            {placeholder}
            {busy}
            command={command.clone()}
        />
    }
}

fn render_renewal(
    runtime: &CanvasRuntime,
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    busy: bool,
    command: &Callback<(CanonicalUuid, MemberCommand)>,
) -> Html {
    let renewal = watchband::agent_renewal_control(
        &standing.label,
        true,
        runtime
            .lanes()
            .and_then(|lanes| lanes.renewal(standing.team, member.lane)),
    );
    // A vacant seat has no occupancy to renew, but a request already placed
    // on it can still be withdrawn.
    if member.occupant.is_none() && renewal.next_requested {
        return Html::default();
    }
    let next = MemberCommand::Renew {
        requested: renewal.next_requested,
    };
    let onclick = send(command, member.lane, next);
    if renewal.next_requested {
        confirmed(
            "renew",
            renewal.heading,
            format!(
                "Start a fresh successor for {}? Its current occupancy ends at the handoff.",
                standing.label
            ),
            format!("Renew {}", standing.label),
            busy,
            onclick,
        )
    } else {
        html! {
            <button
                type="button"
                class="team-mode"
                data-member-control="renew"
                aria-label={renewal.label}
                title={renewal.title}
                disabled={busy}
                {onclick}
            >{renewal.heading}</button>
        }
    }
}

fn render_lifecycle(
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    busy: bool,
    command: &Callback<(CanonicalUuid, MemberCommand)>,
) -> Html {
    let lane = member.lane;
    let label = &standing.label;
    html! {
        <>
            if member.occupant.is_none() && member.wakeable {
                <button
                    type="button"
                    class="team-mode"
                    data-member-control="wake"
                    disabled={busy}
                    onclick={send(command, lane, MemberCommand::Wake)}
                >{"Wake"}</button>
            }
            if member.occupant.is_some() {
                {confirmed(
                    "drain",
                    "Drain",
                    format!("End {label}'s occupancy? The seat stays, vacant, until it is woken."),
                    format!("Drain {label}"),
                    busy,
                    send(command, lane, MemberCommand::Drain),
                )}
            }
            if member.occupant.is_none() && standing.handle.is_none() {
                {confirmed(
                    "remove",
                    "Remove",
                    format!("Remove {label}'s seat from this team?"),
                    format!("Remove {label}"),
                    busy,
                    send(command, lane, MemberCommand::Remove),
                )}
            }
        </>
    }
}

/// The member's controls, their in-flight command, and its last refusal.
pub(super) fn render(
    runtime: &CanvasRuntime,
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    command: &Callback<(CanonicalUuid, MemberCommand)>,
) -> Html {
    let offered = Offered::of(runtime);
    if !(offered.lifecycle || offered.renew || offered.retune) {
        return Html::default();
    }
    let state = runtime.member_control(member.lane);
    let pending = state.and_then(MemberControlState::pending);
    let issue = state.and_then(MemberControlState::issue);
    let busy = pending.is_some();
    html! {
        <section
            class="team-dash-member-controls"
            aria-label={format!("Controls for {}", standing.label)}
            aria-busy={busy.to_string()}
        >
            <div class="team-dash-member-actions">
                if offered.lifecycle {
                    {render_lifecycle(member, standing, busy, command)}
                }
                if offered.renew {
                    {render_renewal(runtime, member, standing, busy, command)}
                }
            </div>
            if offered.retune {
                {render_retune(runtime, member, busy, command)}
            }
            if let Some(pending) = pending {
                <p class="team-dash-member-progress" role="status">{pending.progress()}</p>
            }
            if let Some(issue) = issue {
                <p class="form-refusal" role="alert">{issue.to_owned()}</p>
            }
        </section>
    }
}
