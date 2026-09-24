//! Chat-page team content for the global Spice menu.

use web_sys::DragEvent;

use super::*;
use crate::watchband;

const MEMBER_MIME: &str = "application/x-spice-team-member";

pub(super) struct Context<'a> {
    pub(super) cursor: &'a str,
    pub(super) identity: Option<&'a CanvasIdentity>,
    pub(super) update: Option<&'a CanvasUpdate>,
    pub(super) sender: &'a SenderHandle,
    pub(super) team_controls: &'a UseReducerHandle<TeamControls>,
    pub(super) submissions: &'a SubmissionStatus,
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
    render_chat(runtime, context)
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
            "Work follows ready, active, deferred, and completed work across projects.",
        ),
        Axis::Topology => (
            "teams",
            "Info connects resident teams, seats, capabilities, and current ownership.",
        ),
    };
    html! {
        <div class="canvas-menu-page" data-menu-page={super::axis_label(axis)}>
            {render_global_settings(runtime, context.sender)}
            <section class="canvas-menu-section canvas-menu-context">
                <h2>{super::axis_label(axis)}</h2>
                <div class="canvas-menu-setting">
                    <strong>{axis.question()}</strong>
                    <span>{format!("{count} {noun}")}</span>
                </div>
                <p class="canvas-menu-context-copy">{description}</p>
            </section>
            {render_version_footer(context.cursor, context.identity, context.update)}
        </div>
    }
}

fn render_chat(runtime: Option<&CanvasRuntime>, context: &Context<'_>) -> Html {
    let Some(runtime) = runtime else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="Chat">
                {render_global_settings(None, context.sender)}
                <p>{"Waiting for resident teams…"}</p>
                {render_version_footer(context.cursor, context.identity, context.update)}
            </div>
        };
    };
    let Some(lanes) = runtime.lanes() else {
        return html! {
            <div class="canvas-menu-page" data-menu-page="Chat">
                {render_global_settings(Some(runtime), context.sender)}
                <p>{"Waiting for agent state…"}</p>
                {render_version_footer(context.cursor, context.identity, context.update)}
            </div>
        };
    };
    let waiting = runtime
        .inbox()
        .map(|inbox| inbox.lanes.as_slice())
        .unwrap_or(&[]);
    let can_configure = runtime.offers(CommandKind::TeamConfigure);
    let grouped_teams = lanes
        .lanes
        .iter()
        .filter(|team| team.teamed())
        .collect::<Vec<_>>();
    let solo_members = lanes
        .lanes
        .iter()
        .filter(|team| !team.teamed())
        .flat_map(|team| &team.members)
        .collect::<Vec<_>>();
    let solo_member_ids = solo_members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let new_team = super::super::random_uuid().expect("browser crypto mints a new team identity");
    let solo_render = SoloDropRender {
        waiting,
        submissions: context.submissions,
        sender: context.sender,
        can_configure,
        topology_revision: lanes.topology_revision,
        team_controls: context.team_controls,
    };
    html! {
        <div class="canvas-menu-page" data-menu-page="Chat">
            {render_global_settings(Some(runtime), context.sender)}
            <section class="canvas-menu-section canvas-open-teams">
                <h2>{"Open Team"}</h2>
                if grouped_teams.is_empty() {
                    <p class="canvas-menu-empty">{"No open teams"}</p>
                } else {
                    {for grouped_teams.into_iter().map(|team| {
                        render_team(
                            team,
                            waiting,
                            context.submissions,
                            context.sender,
                            can_configure,
                            lanes.topology_revision,
                            context.team_controls,
                        )
                    })}
                }
            </section>
            {render_new_team_drop(
                new_team,
                context.sender,
                can_configure,
                lanes.topology_revision,
            )}
            {render_solo_members_drop(
                &solo_members,
                solo_member_ids,
                &solo_render,
            )}
            {render_version_footer(context.cursor, context.identity, context.update)}
        </div>
    }
}

fn render_global_settings(runtime: Option<&CanvasRuntime>, sender: &SenderHandle) -> Html {
    let settings = runtime.and_then(CanvasRuntime::settings);
    let fast_mode = settings.is_some_and(|settings| settings.fast_mode);
    let enabled = settings.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::GlobalSettingsSet));
    let sender = sender.clone();
    let onclick = Callback::from(move |event: MouseEvent| {
        if enabled && admit_action(&sender, ClientAction::FastMode(!fast_mode)) {
            event.stop_propagation();
        }
    });
    html! {
        <section class="canvas-menu-section canvas-menu-global">
            <h2>{"Global"}</h2>
            <div class="canvas-menu-global-controls">
                <button
                    class={classes!(
                        "canvas-menu-setting",
                        "canvas-menu-setting--action",
                        "canvas-fast-setting",
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
                {render_theme_selector()}
            </div>
        </section>
    }
}

fn render_version_footer(
    cursor: &str,
    identity: Option<&CanvasIdentity>,
    update: Option<&CanvasUpdate>,
) -> Html {
    let IdentityPresentation {
        label,
        details,
        accessible,
    } = identity_presentation(identity);
    let product_version = identity.map(|identity| identity.version.product.to_string());
    let native_binary = identity.map(|identity| identity.version.binary.to_string());
    let wire_schema = identity.map(|identity| identity.version.wire.to_string());
    let journal_schema = identity.map(|identity| identity.version.journal.to_string());
    let canvas_protocol = identity.map(|identity| identity.version.canvas.to_string());
    let loaded_app = identity.map(|identity| identity.loaded_app.to_string());
    let served_app = identity.map(|identity| identity.version.app.to_string());
    let reload = Callback::from(move |_| {
        web_sys::window()
            .expect("Canvas update action has a browser window")
            .location()
            .reload()
            .expect("Canvas update action reloads the current document");
    });
    let version = if let Some(update) = update {
        let product = product_version.as_deref().unwrap_or("loading");
        let mut update_rows = vec![format!("Current product: {product}")];
        if let Some(identity) = identity {
            update_rows.extend([
                format!("Native binary: {}", identity.version.binary),
                format!("Wire schema: {}", identity.version.wire),
                format!("Journal schema: {}", identity.version.journal),
            ]);
        }
        update_rows.extend([
            format!("Loaded app generation: {}", update.loaded_generation),
            format!("Available Canvas protocol: {}", update.version),
            format!("Served app generation: {}", update.generation),
        ]);
        let update_identity = update_rows.join("\n");
        let update_details = format!("{update_identity}\nAction: Reload to update");
        let update_accessible = format!(
            "Reload Spice to update; {}",
            update_identity.replace('\n', "; ")
        );
        html! {
            <button
                class="canvas-menu-version canvas-menu-version--update"
                type="button"
                aria-label={update_accessible}
                title={update_details}
                data-product-version={product_version}
                data-native-binary={native_binary}
                data-wire-schema={wire_schema}
                data-journal-schema={journal_schema}
                data-canvas-protocol={update.version.to_string()}
                data-loaded-app-generation={update.loaded_generation.to_string()}
                data-served-app-generation={update.generation.to_string()}
                data-canvas-update-version={update.version.to_string()}
                data-canvas-update-generation={update.generation.to_string()}
                onclick={reload}
            >
                <span class="canvas-menu-version-label">{label}</span>
                <span class="canvas-menu-version-indicator" aria-hidden="true">{"↻"}</span>
            </button>
        }
    } else {
        html! {
            <strong
                class="canvas-menu-version"
                tabindex="0"
                aria-label={accessible}
                title={details}
                data-product-version={product_version}
                data-native-binary={native_binary}
                data-wire-schema={wire_schema}
                data-journal-schema={journal_schema}
                data-canvas-protocol={canvas_protocol}
                data-loaded-app-generation={loaded_app}
                data-served-app-generation={served_app}
            >{label}</strong>
        }
    };
    html! {
        <footer class="canvas-menu-footer">
            <div class="canvas-menu-version-group">
                {version}
            </div>
            <span class="bar-meta canvas-menu-cursor" aria-label="Global cursor">
                {format!("cursor {cursor}")}
            </span>
        </footer>
    }
}

struct IdentityPresentation {
    label: String,
    details: Option<String>,
    accessible: String,
}

fn identity_presentation(identity: Option<&CanvasIdentity>) -> IdentityPresentation {
    let Some(identity) = identity else {
        return IdentityPresentation {
            label: "Spice".to_owned(),
            details: None,
            accessible: "Spice product identity is loading".to_owned(),
        };
    };
    let mut rows = vec![
        format!("Product: {}", identity.version.product),
        format!("Native binary: {}", identity.version.binary),
        format!("Wire schema: {}", identity.version.wire),
        format!("Journal schema: {}", identity.version.journal),
        format!("Canvas protocol: {}", identity.version.canvas),
        format!("Loaded app generation: {}", identity.loaded_app),
    ];
    if identity.loaded_app != identity.version.app {
        rows.push(format!("Served app generation: {}", identity.version.app));
    }
    let details = rows.join("\n");
    IdentityPresentation {
        label: format!(
            "Spice {}",
            abbreviated_product(&identity.version.product.to_string())
        ),
        accessible: details.replace('\n', "; "),
        details: Some(details),
    }
}

/// Characters kept from one build-metadata component, ellipsis included.
const BUILD_COMPONENT_BUDGET: usize = 9;

/// The product version with its build metadata cut to a change indicator.
///
/// The build component carries the whole appliance digest, which is wider
/// than the menu and reads as noise at full length; eight characters are
/// enough to see that the appliance changed. The release identity is never
/// cut, and the exact spelling stays in the hover details, the accessible
/// name, and the `data-product-version` attribute.
fn abbreviated_product(product: &str) -> String {
    let Some((release, build)) = product.split_once('+') else {
        return product.to_owned();
    };
    let build = build
        .split('.')
        .map(|component| crate::watchband::truncate(component, BUILD_COMPONENT_BUDGET))
        .collect::<Vec<_>>()
        .join(".");
    format!("{release}+{build}")
}

fn render_theme_selector() -> Html {
    let selected = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .and_then(|root| root.get_attribute("data-canvas-theme"))
        .unwrap_or_else(|| "system".to_owned());
    html! {
        <div
            class="canvas-theme-selector"
            role="group"
            aria-label="Canvas theme"
            data-selected-theme={selected.clone()}
        >
            <span class="canvas-theme-thumb" aria-hidden="true"></span>
            {for [("light", "Light"), ("system", "System"), ("dark", "Dark")]
                .into_iter()
                .map(|(mode, label)| {
                    let choose = Callback::from(move |_| {
                        let Some(window) = web_sys::window() else { return; };
                        let Ok(setter) = js_sys::Reflect::get(
                            &window,
                            &JsValue::from_str("__spiceCanvasSetTheme"),
                        ) else { return; };
                        let Ok(setter) = setter.dyn_into::<js_sys::Function>() else { return; };
                        let _ = setter.call1(&JsValue::NULL, &JsValue::from_str(mode));
                    });
                    let icon = match mode {
                        "light" => html! {
                            <svg class="canvas-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                <circle cx="12" cy="12" r="3.25"></circle>
                                <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.28 5.28 6.7 6.7M17.3 17.3l1.42 1.42M18.72 5.28 17.3 6.7M6.7 17.3l-1.42 1.42"></path>
                            </svg>
                        },
                        "system" => html! {
                            <svg class="canvas-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                <rect x="3.25" y="4.5" width="17.5" height="13" rx="1.8"></rect>
                                <path d="M8.5 21h7M12 17.5V21"></path>
                            </svg>
                        },
                        "dark" => html! {
                            <svg class="canvas-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                <path d="M20.2 15.3A8.7 8.7 0 0 1 8.7 3.8a8.7 8.7 0 1 0 11.5 11.5Z"></path>
                            </svg>
                        },
                        _ => unreachable!("Canvas theme modes are closed"),
                    };
                    let accessible = format!("Use {label} theme");
                    html! {
                        <button
                            class="canvas-theme-option"
                            type="button"
                            aria-label={accessible}
                            title={format!("{label} theme")}
                            aria-pressed={(selected == mode).to_string()}
                            data-canvas-theme={mode}
                            onclick={choose}
                        >{icon}</button>
                    }
                })}
        </div>
    }
}

fn render_team(
    team: &crate::runtime::LaneRow,
    waiting: &[wire::intent::LaneInbox],
    submissions: &SubmissionStatus,
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
    team_controls: &UseReducerHandle<TeamControls>,
) -> Html {
    let target = team.lane;
    let current = team
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<Vec<_>>();
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_team(
        sender.clone(),
        target,
        current,
        can_configure,
        topology_revision,
    );
    let closed = team_controls.is_closed(team.lane);
    let reopen = team_controls.clone();
    html! {
        <article
            class="canvas-team"
            data-team={team.lane.to_string()}
            data-drop-enabled={can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <div>
                    <strong>{format!("Team {}", watchband::short(team.lane))}</strong>
                    <span>{format!(
                        "open any member; {} agents open together",
                        team.members.len()
                    )}</span>
                </div>
                if closed {
                    <button
                        type="button"
                        class="canvas-team-open"
                        onclick={Callback::from(move |_| {
                            reopen.dispatch(TeamControlAction::OpenTeam {
                                team: target,
                            });
                        })}
                    >{"Open team"}</button>
                }
            </header>
            <div class="canvas-team-members">
                {for team.members.iter().enumerate().map(|(position, member)| {
                    render_member(
                        member,
                        watchband::accent(team.members.len(), position),
                        PresenceProjection {
                            waiting,
                            submissions,
                        },
                        team.lane,
                        can_configure,
                        team_controls,
                        false,
                    )
                })}
            </div>
        </article>
    }
}

struct PresenceProjection<'a> {
    waiting: &'a [wire::intent::LaneInbox],
    submissions: &'a SubmissionStatus,
}

fn render_member(
    member: &crate::runtime::LaneMember,
    accent: usize,
    presence: PresenceProjection<'_>,
    source: CanonicalUuid,
    can_configure: bool,
    team_controls: &UseReducerHandle<TeamControls>,
    show_reopen: bool,
) -> Html {
    let pending = super::member_pending(presence.waiting, presence.submissions, member.lane);
    let status = member.status(pending);
    let label = member
        .name
        .clone()
        .unwrap_or_else(|| watchband::leaf(&member.worktree).to_owned());
    let worktree = watchband::leaf(&member.worktree).to_owned();
    let ondragstart = begin_drag(member.lane, can_configure);
    let closed = team_controls.is_closed(source);
    let reopen = team_controls.clone();
    html! {
        <article
            class="canvas-team-member"
            draggable={can_configure.to_string()}
            aria-disabled={(!can_configure).to_string()}
            data-team-member={member.lane.to_string()}
            data-source-team={source.to_string()}
            data-accent={accent.to_string()}
            {ondragstart}
        >
            {render_member_presence(status)}
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
                    class="canvas-team-open"
                    onclick={Callback::from(move |_| {
                        reopen.dispatch(TeamControlAction::OpenTeam { team: source });
                    })}
                >{"Open team"}</button>
            }
        </article>
    }
}

fn render_new_team_drop(
    target: CanonicalUuid,
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
) -> Html {
    let ondragover = allow_drop(can_configure);
    let ondrop = drop_into_new_team(sender.clone(), target, can_configure, topology_revision);
    html! {
        <section
            class="canvas-menu-section canvas-team-drop"
            data-drop-enabled={can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header><h2>{"New Team"}</h2><span>{"drop agent to create"}</span></header>
            <div class="canvas-drop-target" data-team-drop="new">{"Drop agent here"}</div>
        </section>
    }
}

struct SoloDropRender<'a> {
    waiting: &'a [wire::intent::LaneInbox],
    submissions: &'a SubmissionStatus,
    sender: &'a SenderHandle,
    can_configure: bool,
    topology_revision: u64,
    team_controls: &'a UseReducerHandle<TeamControls>,
}

fn render_solo_members_drop(
    members: &[&crate::runtime::LaneMember],
    current: Vec<CanonicalUuid>,
    render: &SoloDropRender<'_>,
) -> Html {
    let ondragover = allow_drop(render.can_configure);
    let ondrop = drop_into_solo_team(
        render.sender.clone(),
        current,
        render.can_configure,
        render.topology_revision,
    );
    html! {
        <section
            class="canvas-menu-section canvas-unassigned"
            data-drop-enabled={render.can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <h2>{"Unassigned Agents"}</h2>
                <span>{"drop to unassign"}</span>
            </header>
            <div class="canvas-team-members" data-team-drop="unassigned">
                if members.is_empty() {
                    <p class="canvas-menu-empty">{"No unassigned agents"}</p>
                } else {
                    {for members.iter().map(|member| {
                        render_member(
                            member,
                            watchband::accent(1, 0),
                            PresenceProjection {
                                waiting: render.waiting,
                                submissions: render.submissions,
                            },
                            member.lane,
                            render.can_configure,
                            render.team_controls,
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

fn drop_into_team(
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
        if enabled
            && !current.contains(&member)
            && send_member(&sender, target, member, topology_revision)
        {
            event.stop_propagation();
        }
    })
}

fn drop_into_new_team(
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
        if enabled && send_member(&sender, target, member, topology_revision) {
            event.stop_propagation();
        }
    })
}

fn drop_into_solo_team(
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
        if enabled
            && !current.contains(&member)
            && send_member(&sender, member, member, topology_revision)
        {
            event.stop_propagation();
        }
    })
}

fn send_member(
    sender: &SenderHandle,
    team: CanonicalUuid,
    member: CanonicalUuid,
    topology_revision: u64,
) -> bool {
    admit_action(
        sender,
        ClientAction::ConfigureTeam {
            topology_revision,
            team,
            member: Some(member),
            life: None,
        },
    )
}
