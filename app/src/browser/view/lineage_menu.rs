//! Agents-page content for the global Spice menu.

use web_sys::DragEvent;

use super::*;
use crate::watchband;

const MEMBER_MIME: &str = "application/x-spice-lineage-member";

pub(super) fn render(
    mosaic: bool,
    active_axis: Axis,
    axis_count: usize,
    runtime: Option<&CanvasRuntime>,
    sender: &SenderHandle,
) -> Html {
    if !mosaic {
        return render_axis(active_axis, axis_count);
    }
    render_agents(runtime, sender)
}

fn render_axis(axis: Axis, count: usize) -> Html {
    let (noun, context) = match axis {
        Axis::Magnitude => (
            "rules",
            "Magnitude summarizes current policy, consent budgets, and trust standing.",
        ),
        Axis::Flow => (
            "active tasks",
            "Flow follows ready, active, deferred, and completed work across projects.",
        ),
        Axis::Topology => (
            "lineages",
            "Topology connects resident teams, seats, capabilities, and current ownership.",
        ),
        Axis::Chronology => (
            "events",
            "Chronology follows facts, command outcomes, and their causal thread.",
        ),
    };
    html! {
        <div class="canvas-menu-page" data-menu-page={axis.as_str()}>
            <section class="canvas-menu-section canvas-menu-global">
                <h2>{"Global"}</h2>
                <div class="canvas-menu-setting">
                    <strong>{"Fast mode"}</strong>
                    <span>{"off"}</span>
                </div>
            </section>
            <section class="canvas-menu-section canvas-menu-context">
                <h2>{title(axis.as_str())}</h2>
                <div class="canvas-menu-setting">
                    <strong>{axis.question()}</strong>
                    <span>{format!("{count} {noun}")}</span>
                </div>
                <p class="canvas-menu-context-copy">{context}</p>
            </section>
        </div>
    }
}

fn render_agents(runtime: Option<&CanvasRuntime>, sender: &SenderHandle) -> Html {
    let Some(runtime) = runtime else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="agents">
                <p>{"Waiting for resident teams…"}</p>
            </div>
        };
    };
    let Some(lanes) = runtime.lanes() else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="agents">
                <p>{"Waiting for lane state…"}</p>
            </div>
        };
    };
    let waiting = runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let can_configure = runtime.offers(CommandKind::LineageConfigure);
    let teams = lanes
        .lanes
        .iter()
        .filter(|lineage| is_lineage_group(lineage))
        .collect::<Vec<_>>();
    let unteamed = lanes
        .lanes
        .iter()
        .filter(|lineage| !is_lineage_group(lineage))
        .flat_map(|lineage| &lineage.members)
        .collect::<Vec<_>>();
    let life_by_member = lanes
        .lanes
        .iter()
        .flat_map(|lineage| {
            lineage
                .members
                .iter()
                .map(move |member| (member.lane, lineage.subscription))
        })
        .collect::<BTreeMap<_, _>>();
    let unteamed_ids = unteamed
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let new_team = super::super::random_uuid().expect("browser crypto mints a new team identity");
    html! {
        <div class="canvas-menu-page" data-menu-page="agents">
            <section class="canvas-menu-section canvas-menu-global">
                <h2>{"Global"}</h2>
                <div class="canvas-menu-setting">
                    <strong>{"Fast mode"}</strong>
                    <span>{"off"}</span>
                </div>
            </section>
            <section class="canvas-menu-section canvas-open-teams">
                <h2>{"Open Team"}</h2>
                if teams.is_empty() {
                    <p class="canvas-menu-empty">{"No open teams"}</p>
                } else {
                    {for teams.into_iter().map(|lineage| {
                        render_lineage_group(lineage, waiting, sender, can_configure)
                    })}
                }
            </section>
            {render_new_lineage_drop(
                new_team,
                life_by_member.clone(),
                sender,
                can_configure,
            )}
            {render_unassigned_drop(
                &unteamed,
                waiting,
                unteamed_ids,
                life_by_member,
                sender,
                can_configure,
            )}
        </div>
    }
}

fn is_lineage_group(lineage: &crate::runtime::LaneRow) -> bool {
    lineage.members.len() > 1
        || lineage
            .host()
            .is_some_and(|member| member.lane != lineage.lane)
}

fn render_lineage_group(
    lineage: &crate::runtime::LaneRow,
    waiting: &[wire::intent::LaneInbox],
    sender: &SenderHandle,
    can_configure: bool,
) -> Html {
    let target = lineage.lane;
    let life = lineage.subscription;
    let current = lineage
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_lineage(sender.clone(), target, life, current, can_configure);
    html! {
        <article
            class="canvas-team"
            data-team-lineage={lineage.lane.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <strong>{format!("Team {}", watchband::short(lineage.lane))}</strong>
                <span>{format!(
                    "open any member; {} agents open together",
                    lineage.members.len()
                )}</span>
            </header>
            <div class="canvas-team-members">
                {for lineage.members.iter().map(|member| {
                    render_member(member, waiting, lineage.lane, can_configure)
                })}
            </div>
        </article>
    }
}

fn render_member(
    member: &crate::runtime::LaneMember,
    waiting: &[wire::intent::LaneInbox],
    source: CanonicalUuid,
    can_configure: bool,
) -> Html {
    let pending = waiting
        .iter()
        .find(|row| row.lane == member.lane)
        .map_or(0, |row| row.pending.len());
    let status = member.status(pending);
    let label = member
        .name
        .clone()
        .unwrap_or_else(|| watchband::leaf(&member.worktree).to_owned());
    let worktree = watchband::leaf(&member.worktree).to_owned();
    let ondragstart = begin_drag(member.lane, can_configure);
    html! {
        <article
            class="canvas-team-member"
            draggable={can_configure.to_string()}
            aria-disabled={(!can_configure).to_string()}
            data-team-member={member.lane.to_string()}
            data-source-lineage={source.to_string()}
            data-accent={watchband::accent(member.lane).to_string()}
            {ondragstart}
        >
            <span class={classes!("canvas-member-presence", status.key())}></span>
            <span class="canvas-member-copy">
                <strong>{format!("{label} on {worktree}")}</strong>
                <small>{format!(
                    "{} · {}{}",
                    member.vendor,
                    status.label(),
                    if pending == 0 { String::new() } else { format!(" · {pending} pending") }
                )}</small>
            </span>
            <span class="canvas-member-drag" aria-hidden="true">{"↕"}</span>
        </article>
    }
}

fn render_new_lineage_drop(
    target: CanonicalUuid,
    life_by_member: BTreeMap<CanonicalUuid, LineageLife>,
    sender: &SenderHandle,
    can_configure: bool,
) -> Html {
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_new_lineage(sender.clone(), target, life_by_member, can_configure);
    html! {
        <section class="canvas-menu-section canvas-team-drop" {ondragover} {ondrop}>
            <header><h2>{"New Team"}</h2><span>{"drop agent to create"}</span></header>
            <div class="canvas-drop-target" data-team-drop="new">{"Drop agent here"}</div>
        </section>
    }
}

fn render_unassigned_drop(
    members: &[&crate::runtime::LaneMember],
    waiting: &[wire::intent::LaneInbox],
    current: Vec<CanonicalUuid>,
    life_by_member: BTreeMap<CanonicalUuid, LineageLife>,
    sender: &SenderHandle,
    can_configure: bool,
) -> Html {
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_without_lineage(sender.clone(), current, life_by_member, can_configure);
    html! {
        <section class="canvas-menu-section canvas-unteamed" {ondragover} {ondrop}>
            <header>
                <h2>{"Agents Without Team"}</h2>
                <span>{"drop here to remove from team"}</span>
            </header>
            <div class="canvas-team-members" data-team-drop="unteamed">
                if members.is_empty() {
                    <p class="canvas-menu-empty">{"No unteamed agents"}</p>
                } else {
                    {for members.iter().map(|member| {
                        render_member(member, waiting, member.lane, can_configure)
                    })}
                }
            </div>
        </section>
    }
}

fn begin_drag(lane: CanonicalUuid, enabled: bool) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if !enabled {
            event.prevent_default();
            return;
        }
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if transfer.set_data(MEMBER_MIME, &lane.to_string()).is_ok() {
            transfer.set_effect_allowed("move");
        }
    })
}

fn allow_drop(enabled: bool) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        if enabled {
            event.prevent_default();
            if let Some(transfer) = event.data_transfer() {
                transfer.set_drop_effect("move");
            }
        }
    })
}

fn dragged_member(event: &DragEvent) -> Option<CanonicalUuid> {
    event
        .data_transfer()?
        .get_data(MEMBER_MIME)
        .ok()?
        .parse()
        .ok()
}

fn drop_into_lineage(
    sender: SenderHandle,
    target: CanonicalUuid,
    life: LineageLife,
    current: Vec<CanonicalUuid>,
    enabled: bool,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if enabled && !current.contains(&member) {
            send_member(&sender, target, member, life);
        }
    })
}

fn drop_into_new_lineage(
    sender: SenderHandle,
    target: CanonicalUuid,
    life_by_member: BTreeMap<CanonicalUuid, LineageLife>,
    enabled: bool,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if let Some(life) = enabled.then(|| life_by_member.get(&member)).flatten() {
            send_member(&sender, target, member, *life);
        }
    })
}

fn drop_without_lineage(
    sender: SenderHandle,
    current: Vec<CanonicalUuid>,
    life_by_member: BTreeMap<CanonicalUuid, LineageLife>,
    enabled: bool,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if enabled
            && !current.contains(&member)
            && let Some(life) = life_by_member.get(&member)
        {
            send_member(&sender, member, member, *life);
        }
    })
}

fn send_member(
    sender: &SenderHandle,
    lineage: CanonicalUuid,
    member: CanonicalUuid,
    life: LineageLife,
) {
    send_action(
        sender,
        ClientAction::ConfigureLineage {
            lineage,
            member: Some(member),
            life,
        },
    );
}
