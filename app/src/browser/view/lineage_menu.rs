//! Lanes-page lineage content for the global Spice menu.

use web_sys::DragEvent;

use super::*;
use crate::watchband;

const MEMBER_MIME: &str = "application/x-spice-lineage-member";

pub(super) struct Context<'a> {
    pub(super) cursor: &'a str,
    pub(super) update: Option<&'a CanvasUpdate>,
    pub(super) sender: &'a SenderHandle,
    pub(super) lineage_controls: &'a UseReducerHandle<LineageControls>,
}

pub(super) fn render(
    mosaic: bool,
    active_axis: Axis,
    axis_count: usize,
    runtime: Option<&CanvasRuntime>,
    context: &Context<'_>,
) -> Html {
    if !mosaic {
        return render_axis(active_axis, axis_count, runtime, context);
    }
    render_lanes(runtime, context)
}

fn render_axis(
    axis: Axis,
    count: usize,
    runtime: Option<&CanvasRuntime>,
    context: &Context<'_>,
) -> Html {
    let (noun, description) = match axis {
        Axis::Flow => (
            "active tasks",
            "Tasks follows ready, active, deferred, and completed work across projects.",
        ),
        Axis::Topology => (
            "lineages",
            "Team connects resident organizations, seats, capabilities, and current ownership.",
        ),
    };
    html! {
        <div class="canvas-menu-page" data-menu-page={axis.as_str()}>
            {render_global_settings(runtime, context.sender)}
            <section class="canvas-menu-section canvas-menu-context">
                <h2>{super::axis_label(axis)}</h2>
                <div class="canvas-menu-setting">
                    <strong>{axis.question()}</strong>
                    <span>{format!("{count} {noun}")}</span>
                </div>
                <p class="canvas-menu-context-copy">{description}</p>
            </section>
            {render_version_footer(context.cursor, context.update)}
        </div>
    }
}

fn render_lanes(runtime: Option<&CanvasRuntime>, context: &Context<'_>) -> Html {
    let Some(runtime) = runtime else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="lanes">
                {render_global_settings(None, context.sender)}
                <p>{"Waiting for resident organizations…"}</p>
                {render_version_footer(context.cursor, context.update)}
            </div>
        };
    };
    let Some(lanes) = runtime.lanes() else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="lanes">
                {render_global_settings(Some(runtime), context.sender)}
                <p>{"Waiting for agent state…"}</p>
                {render_version_footer(context.cursor, context.update)}
            </div>
        };
    };
    let waiting = runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let can_configure = runtime.offers(CommandKind::LineageConfigure);
    let grouped_lineages = lanes
        .lanes
        .iter()
        .filter(|lineage| is_grouped_lineage(lineage))
        .collect::<Vec<_>>();
    let solo_members = lanes
        .lanes
        .iter()
        .filter(|lineage| !is_grouped_lineage(lineage))
        .flat_map(|lineage| &lineage.members)
        .collect::<Vec<_>>();
    let solo_member_ids = solo_members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let new_lineage =
        super::super::random_uuid().expect("browser crypto mints a new lineage identity");
    let solo_render = SoloDropRender {
        waiting,
        sender: context.sender,
        can_configure,
        topology_revision: lanes.topology_revision,
        lineage_controls: context.lineage_controls,
    };
    html! {
        <div class="canvas-menu-page" data-menu-page="lanes">
            {render_global_settings(Some(runtime), context.sender)}
            <section class="canvas-menu-section canvas-open-organizations">
                <h2>{"Open Team"}</h2>
                if grouped_lineages.is_empty() {
                    <p class="canvas-menu-empty">{"No open organizations"}</p>
                } else {
                    {for grouped_lineages.into_iter().map(|lineage| {
                        render_lineage(
                            lineage,
                            waiting,
                            context.sender,
                            can_configure,
                            lanes.topology_revision,
                            context.lineage_controls,
                        )
                    })}
                }
            </section>
            {render_new_lineage_drop(
                new_lineage,
                context.sender,
                can_configure,
                lanes.topology_revision,
            )}
            {render_solo_members_drop(
                &solo_members,
                solo_member_ids,
                &solo_render,
            )}
            {render_version_footer(context.cursor, context.update)}
        </div>
    }
}

fn render_global_settings(runtime: Option<&CanvasRuntime>, sender: &SenderHandle) -> Html {
    let settings = runtime.and_then(CanvasRuntime::settings);
    let fast_mode = settings.is_some_and(|settings| settings.fast_mode);
    let enabled = settings.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::GlobalSettingsSet));
    let sender = sender.clone();
    let onclick = Callback::from(move |_| {
        if enabled {
            send_action(&sender, ClientAction::FastMode(!fast_mode));
        }
    });
    html! {
        <section class="canvas-menu-section canvas-menu-global">
            <h2>{"Global"}</h2>
            <button
                class={classes!(
                    "canvas-menu-setting",
                    "canvas-menu-setting--action",
                    fast_mode.then_some("canvas-menu-setting--checked"),
                )}
                type="button"
                role="menuitemcheckbox"
                aria-checked={fast_mode.to_string()}
                disabled={!enabled}
                title={if fast_mode {
                    "Fast mode is on for supported agents"
                } else {
                    "Fast mode is off for supported agents"
                }}
                {onclick}
            >
                <strong>{"Fast mode"}</strong>
                <span>{if settings.is_some() {
                    if fast_mode { "on" } else { "off" }
                } else {
                    "loading"
                }}</span>
            </button>
        </section>
    }
}

fn render_version_footer(cursor: &str, update: Option<&CanvasUpdate>) -> Html {
    let version = wire::CanvasVersion::CURRENT;
    let reload = Callback::from(move |_| {
        web_sys::window()
            .expect("Canvas update action has a browser window")
            .location()
            .reload()
            .expect("Canvas update action reloads the current document");
    });
    html! {
        <footer class="canvas-menu-footer">
            <div class="canvas-menu-version-group">
                <strong
                    class="canvas-menu-version"
                    data-canvas-version={version.to_string()}
                >{format!("Canvas v{version}")}</strong>
                if let Some(update) = update {
                    <button
                        class="canvas-menu-update"
                        type="button"
                        aria-label={format!("Reload Canvas v{}", update.version)}
                        title={format!("Reload Canvas v{}", update.version)}
                        data-canvas-update-version={update.version.to_string()}
                        data-canvas-update-generation={update.generation.to_string()}
                        onclick={reload}
                    >{"Update"}</button>
                }
            </div>
            <span class="bar-meta canvas-menu-cursor" aria-label="Global cursor">
                {format!("cursor {cursor}")}
            </span>
        </footer>
    }
}

fn is_grouped_lineage(lineage: &crate::runtime::LaneRow) -> bool {
    lineage.members.len() > 1
        || lineage
            .host()
            .is_some_and(|member| member.lane != lineage.lane)
}

fn render_lineage(
    lineage: &crate::runtime::LaneRow,
    waiting: &[wire::intent::LaneInbox],
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
    lineage_controls: &UseReducerHandle<LineageControls>,
) -> Html {
    let target = lineage.lane;
    let current = lineage
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_lineage(
        sender.clone(),
        target,
        current,
        can_configure,
        topology_revision,
    );
    let closed = lineage_controls.is_closed(lineage.lane);
    let reopen = lineage_controls.clone();
    html! {
        <article
            class="canvas-organization"
            data-organization-lineage={lineage.lane.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <div>
                    <strong>{format!("Team {}", watchband::short(lineage.lane))}</strong>
                    <span>{format!(
                        "open any member; {} agents open together",
                        lineage.members.len()
                    )}</span>
                </div>
                if closed {
                    <button
                        type="button"
                        class="canvas-organization-open"
                        onclick={Callback::from(move |_| {
                            reopen.dispatch(LineageControlAction::OpenLineage {
                                lineage: target,
                            });
                        })}
                    >{"Open team"}</button>
                }
            </header>
            <div class="canvas-organization-members">
                {for lineage.members.iter().enumerate().map(|(position, member)| {
                    render_member(
                        member,
                        watchband::accent(lineage.members.len(), position),
                        waiting,
                        lineage.lane,
                        can_configure,
                        lineage_controls,
                        false,
                    )
                })}
            </div>
        </article>
    }
}

fn render_member(
    member: &crate::runtime::LaneMember,
    accent: usize,
    waiting: &[wire::intent::LaneInbox],
    source: CanonicalUuid,
    can_configure: bool,
    lineage_controls: &UseReducerHandle<LineageControls>,
    show_reopen: bool,
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
    let closed = lineage_controls.is_closed(source);
    let reopen = lineage_controls.clone();
    html! {
        <article
            class="canvas-organization-member"
            draggable={can_configure.to_string()}
            aria-disabled={(!can_configure).to_string()}
            data-organization-member={member.lane.to_string()}
            data-source-lineage={source.to_string()}
            data-accent={accent.to_string()}
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
            if closed && show_reopen {
                <button
                    type="button"
                    class="canvas-organization-open"
                    onclick={Callback::from(move |_| {
                        reopen.dispatch(LineageControlAction::OpenLineage { lineage: source });
                    })}
                >{"Open team"}</button>
            }
        </article>
    }
}

fn render_new_lineage_drop(
    target: CanonicalUuid,
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
) -> Html {
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_new_lineage(sender.clone(), target, can_configure, topology_revision);
    html! {
        <section class="canvas-menu-section canvas-organization-drop" {ondragover} {ondrop}>
            <header><h2>{"New Team"}</h2><span>{"drop agent to create"}</span></header>
            <div class="canvas-drop-target" data-organization-drop="new">{"Drop agent here"}</div>
        </section>
    }
}

struct SoloDropRender<'a> {
    waiting: &'a [wire::intent::LaneInbox],
    sender: &'a SenderHandle,
    can_configure: bool,
    topology_revision: u64,
    lineage_controls: &'a UseReducerHandle<LineageControls>,
}

fn render_solo_members_drop(
    members: &[&crate::runtime::LaneMember],
    current: Vec<CanonicalUuid>,
    render: &SoloDropRender<'_>,
) -> Html {
    let ondragover = allow_drop(render.can_configure);
    let ondrop = drop_into_solo_lineage(
        render.sender.clone(),
        current,
        render.can_configure,
        render.topology_revision,
    );
    html! {
        <section class="canvas-menu-section canvas-unassigned" {ondragover} {ondrop}>
            <header>
                <h2>{"Agents Without Team"}</h2>
                <span>{"drop here to remove from team"}</span>
            </header>
            <div class="canvas-organization-members" data-organization-drop="unassigned">
                if members.is_empty() {
                    <p class="canvas-menu-empty">{"No unassigned agents"}</p>
                } else {
                    {for members.iter().map(|member| {
                        render_member(
                            member,
                            watchband::accent(1, 0),
                            render.waiting,
                            member.lane,
                            render.can_configure,
                            render.lineage_controls,
                            true,
                        )
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
    current: Vec<CanonicalUuid>,
    enabled: bool,
    topology_revision: u64,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if enabled && !current.contains(&member) {
            send_member(&sender, target, member, topology_revision);
        }
    })
}

fn drop_into_new_lineage(
    sender: SenderHandle,
    target: CanonicalUuid,
    enabled: bool,
    topology_revision: u64,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if enabled {
            send_member(&sender, target, member, topology_revision);
        }
    })
}

fn drop_into_solo_lineage(
    sender: SenderHandle,
    current: Vec<CanonicalUuid>,
    enabled: bool,
    topology_revision: u64,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(member) = dragged_member(&event) else {
            return;
        };
        if enabled && !current.contains(&member) {
            send_member(&sender, member, member, topology_revision);
        }
    })
}

fn send_member(
    sender: &SenderHandle,
    lineage: CanonicalUuid,
    member: CanonicalUuid,
    topology_revision: u64,
) {
    send_action(
        sender,
        ClientAction::ConfigureLineage {
            topology_revision,
            lineage,
            member: Some(member),
            life: None,
        },
    );
}
