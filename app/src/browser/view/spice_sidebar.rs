//! Spice sidebar content and team-arrangement controls.

use web_sys::{DragEvent, PointerEvent};

use super::*;
use crate::watchband;

const MEMBER_MIME: &str = "application/x-spice-team-member";
/// The drop-target value naming Unassigned Agents rather than a team.
const UNASSIGNED_DROP: &str = "unassigned";
/// How near a scrolling container's visible edge a lifted finger scrolls it.
const TOUCH_SCROLL_EDGE_PX: f64 = 48.0;
/// Each move scrolls this fraction of the finger's depth into the edge band, so
/// pressing deeper scrolls faster without outrunning the target.
const TOUCH_SCROLL_DEPTH_RATE: f64 = 0.25;

thread_local! {
    /// The one roster lift a finger or pen holds; mouse uses HTML5 drag and drop.
    static TOUCH_DRAG: RefCell<watchband::PointerDragSession> = RefCell::default();
    /// A lifted drag's release can still click the slot link; that click is swallowed.
    static SWALLOW_CLICK: Cell<bool> = const { Cell::new(false) };
}

pub(super) struct Context<'a> {
    pub(super) sender: &'a SenderHandle,
}

/// One interactive topology: full-width status slots double as draggable
/// members. A mouse drags the whole slot; a finger or pen drags its name.
pub(super) fn render_arrangement(
    runtime: Option<&AppRuntime>,
    context: &Context<'_>,
    standings: &[watchband::Standing],
) -> Html {
    let lanes = runtime.and_then(AppRuntime::lanes);
    let awaited = match (runtime, lanes) {
        (None, _) => Some("Waiting for resident teams…"),
        (Some(_), None) => Some("Waiting for agent state…"),
        (Some(_), Some(_)) => None,
    };
    let can_configure = lanes.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::TeamConfigure));
    let topology_revision = lanes.map_or(0, |lanes| lanes.topology_revision);
    let teams = lanes.map_or(&[][..], |lanes| lanes.lanes.as_slice());
    let grouped_teams = teams
        .iter()
        .filter(|team| team.teamed())
        .collect::<Vec<_>>();
    let solo_members = teams
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
        standings,
        sender: context.sender,
        can_configure,
        topology_revision,
        empty: awaited.unwrap_or("No unassigned agents"),
    };
    html! {
        <>
            <div class="app-open-teams">
                if grouped_teams.is_empty() && solo_members.is_empty() {
                    <p class="app-empty-state">{awaited.unwrap_or("No agent lanes are connected.")}</p>
                } else {
                    {for grouped_teams.into_iter().map(|team| {
                        render_team(
                            team,
                            standings,
                            context.sender,
                            can_configure,
                            topology_revision,
                        )
                    })}
                }
            </div>
            {render_new_team_drop(
                new_team,
                context.sender,
                can_configure,
                topology_revision,
            )}
            {render_solo_members_drop(
                &solo_members,
                solo_member_ids,
                &solo_render,
            )}
        </>
    }
}

pub(super) fn render_global_settings(
    connection: &Connection,
    runtime: Option<&AppRuntime>,
    identity: Option<&AppIdentity>,
    update: Option<&AppUpdate>,
    sender: &SenderHandle,
) -> Html {
    let settings = runtime.and_then(AppRuntime::settings);
    let fast_mode = settings.is_some_and(|settings| settings.fast_mode);
    let enabled = settings.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::GlobalSettingsSet));
    let sender = sender.clone();
    let onclick = Callback::from(move |event: MouseEvent| {
        if enabled
            && admit_action(
                &sender,
                ClientAction::GlobalSettings(GlobalSettingsSet {
                    fast_mode: !fast_mode,
                }),
            )
        {
            event.stop_propagation();
        }
    });
    let title = match (settings.is_some(), enabled, fast_mode) {
        (false, _, _) => "Fast mode is loading",
        (true, false, _) => "Fast mode cannot be changed by this resident",
        (true, true, true) => "Turbo: every agent runs in fast mode",
        (true, true, false) => "Turbo off: press to run every agent in fast mode",
    };
    let (status, status_details) = match connection {
        Connection::Starting => ("Starting Spice", "Starting Spice".to_owned()),
        Connection::Connecting => (
            "Opening current resident",
            "Opening current resident".to_owned(),
        ),
        Connection::Online { .. } => ("Connected", "Connected".to_owned()),
        Connection::Offline(reason) => ("Reconnecting", format!("Reconnecting / {reason}")),
    };
    html! {
        <section
            class="app-sheet-section app-spice-settings"
            aria-label="Resident and global settings"
            data-connection={connection.indicator_state()}
        >
            <div class="app-resident-summary">
                <span
                    class="app-resident-status"
                    role="status"
                    aria-label={status_details.clone()}
                    title={status_details}
                >
                    {status}
                </span>
                {render_product_version(identity, update)}
            </div>
            <chrome::TurboButton
                class={classes!("app-spice-turbo")}
                engaged={fast_mode}
                label={format!(
                    "Global fast mode: {}",
                    if fast_mode { "on" } else { "off" },
                )}
                disabled={!enabled}
                title={title}
                onclick={onclick}
            />
        </section>
    }
}

/// The cumulative plan-weighted weekly quota remaining across accounts,
/// stacked above the open sidebar's Spice button. Its fixed-width fallback
/// keeps the button still while quota state loads.
pub(super) fn render_capacity(runtime: Option<&AppRuntime>) -> Html {
    let generation = runtime.and_then(AppRuntime::quota);
    let remaining = generation
        .and_then(|generation| generation.last_good.as_deref())
        .and_then(crate::quota::weekly_remaining);
    let stale = generation.is_some_and(crate::quota::last_good_stale);
    let (value, title, state) = if let Some(remaining) = remaining {
        let percentage = format!("{:.0}%", remaining * 100.0);
        let title = if stale {
            format!(
                "Weekly quota {percentage} remaining across accounts ({})",
                crate::quota::Condition::StaleLastGood.label()
            )
        } else {
            format!("Weekly quota {percentage} remaining across accounts")
        };
        (percentage, title, "ready")
    } else if runtime.is_none() {
        (
            "…".to_owned(),
            "Waiting for weekly quota".to_owned(),
            "loading",
        )
    } else {
        (
            "—".to_owned(),
            "Weekly quota is unavailable".to_owned(),
            "unavailable",
        )
    };
    html! {
        <span
            class="app-spice-capacity"
            data-state={state}
            data-quota-stale={stale.then_some("true")}
            aria-label={title.clone()}
            {title}
        >{value}</span>
    }
}

/// The operator's avatar beside the Git `user.name` and `user.email` it is
/// drawn from, as read-only reference rows. Until settings arrive both rows
/// say they are loading, so later admission moves no section below them.
pub(super) fn render_operator(runtime: Option<&AppRuntime>) -> Html {
    let settings = runtime.and_then(AppRuntime::settings);
    let unresolved = wire::OperatorIdentity::default();
    let operator = settings.map_or(&unresolved, |settings| &settings.operator);
    let fact = |label: &'static str, key: &'static str, value: Option<&String>| {
        let (state, text) = match (settings, value) {
            (None, _) => ("loading", "loading".to_owned()),
            (Some(_), None) => ("unset", format!("Git {key} is not set")),
            (Some(_), Some(value)) => ("ready", value.clone()),
        };
        html! {
            <div data-state={state}>
                <dt>{label}</dt>
                <dd title={text.clone()}>{text}</dd>
            </div>
        }
    };
    html! {
        <section class="app-sheet-section app-spice-operator" data-operator-member="">
            <h2>{"You"}</h2>
            <div class="app-spice-operator-identity">
                {super::render_operator_avatar(operator)}
                <dl class="app-spice-operator-facts">
                    {fact("Name", "user.name", operator.name.as_ref())}
                    {fact("Email", "user.email", operator.email.as_ref())}
                </dl>
            </div>
        </section>
    }
}

fn render_product_version(identity: Option<&AppIdentity>, update: Option<&AppUpdate>) -> Html {
    let IdentityPresentation {
        label,
        details,
        accessible,
    } = identity_presentation(identity);
    let product_version = identity.map(|identity| identity.version.product.to_string());
    let native_binary = identity.map(|identity| identity.version.binary.to_string());
    let wire_schema = identity.map(|identity| identity.version.wire.to_string());
    let journal_schema = identity.map(|identity| identity.version.journal.to_string());
    let app_protocol = identity.map(|identity| identity.version.app_protocol.to_string());
    let loaded_app = identity.map(|identity| identity.loaded_app.to_string());
    let served_app = identity.map(|identity| identity.version.app.to_string());
    let reload = Callback::from(move |_| {
        web_sys::window()
            .expect("app update action has a browser window")
            .location()
            .reload()
            .expect("app update action reloads the current document");
    });
    if let Some(update) = update {
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
            format!("Available app protocol: {}", update.version),
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
                class="app-product-version app-product-version--update app-compact-control"
                type="button"
                aria-label={update_accessible}
                title={update_details}
                data-product-version={product_version}
                data-native-binary={native_binary}
                data-wire-schema={wire_schema}
                data-journal-schema={journal_schema}
                data-app-protocol={update.version.to_string()}
                data-loaded-app-generation={update.loaded_generation.to_string()}
                data-served-app-generation={update.generation.to_string()}
                data-app-update-version={update.version.to_string()}
                data-app-update-generation={update.generation.to_string()}
                onclick={reload}
            >
                <span class="app-product-version-label">{label}</span>
                <span class="app-product-version-indicator" aria-hidden="true">{"↻"}</span>
            </button>
        }
    } else {
        html! {
            <strong
                class="app-product-version"
                tabindex="0"
                aria-label={accessible}
                title={details}
                data-product-version={product_version}
                data-native-binary={native_binary}
                data-wire-schema={wire_schema}
                data-journal-schema={journal_schema}
                data-app-protocol={app_protocol}
                data-loaded-app-generation={loaded_app}
                data-served-app-generation={served_app}
            >{label}</strong>
        }
    }
}

struct IdentityPresentation {
    label: String,
    details: Option<String>,
    accessible: String,
}

fn identity_presentation(identity: Option<&AppIdentity>) -> IdentityPresentation {
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
        format!("App protocol: {}", identity.version.app_protocol),
        format!("Loaded app generation: {}", identity.loaded_app),
    ];
    if identity.loaded_app != identity.version.app {
        rows.push(format!("Served app generation: {}", identity.version.app));
    }
    let details = rows.join("\n");
    IdentityPresentation {
        label: format!("Spice {}", identity.version.product),
        accessible: details.replace('\n', "; "),
        details: Some(details),
    }
}

/// The one theme selector, beside the Spice button in the open view's header.
pub(super) fn render_theme_selector() -> Html {
    let selected = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.document_element())
        .and_then(|root| root.get_attribute("data-app-theme"))
        .unwrap_or_else(|| "system".to_owned());
    html! {
            <div class="app-control-rail app-theme-selector" role="group" aria-label="Theme">
                {for [("light", "Light"), ("system", "System"), ("dark", "Dark")]
                    .into_iter()
                    .map(|(mode, label)| {
                        let choose = Callback::from(move |_| {
                            let Some(window) = web_sys::window() else { return; };
                            let Ok(setter) = js_sys::Reflect::get(
                                &window,
                                &JsValue::from_str("__spiceAppSetTheme"),
                            ) else { return; };
                            let Ok(setter) = setter.dyn_into::<js_sys::Function>() else { return; };
                            let _ = setter.call1(&JsValue::NULL, &JsValue::from_str(mode));
                        });
                        let icon = match mode {
                            "light" => html! {
                                <svg class="app-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                    <circle cx="12" cy="12" r="3.25"></circle>
                                    <path d="M12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.28 5.28 6.7 6.7M17.3 17.3l1.42 1.42M18.72 5.28 17.3 6.7M6.7 17.3l-1.42 1.42"></path>
                                </svg>
                            },
                            "system" => html! {
                                <svg class="app-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                    <rect x="3.25" y="4.5" width="17.5" height="13" rx="1.8"></rect>
                                    <path d="M8.5 21h7M12 17.5V21"></path>
                                </svg>
                            },
                            "dark" => html! {
                                <svg class="app-theme-icon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
                                    <path d="M20.2 15.3A8.7 8.7 0 0 1 8.7 3.8a8.7 8.7 0 1 0 11.5 11.5Z"></path>
                                </svg>
                            },
                            _ => unreachable!("app theme modes are closed"),
                        };
                        let accessible = format!("Use {label} theme");
                        html! {
                            <button
                                class="app-choice-chip"
                                type="button"
                                aria-label={accessible}
                                title={format!("{label} theme")}
                                aria-pressed={(selected == mode).to_string()}
                                data-app-theme={mode}
                                onclick={choose}
                            >{icon}</button>
                        }
                    })}
            </div>
    }
}

fn render_team(
    team: &crate::runtime::LaneRow,
    standings: &[watchband::Standing],
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
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
    html! {
        <article
            class="app-team"
            data-team={team.lane.to_string()}
            data-topology-drop={team.lane.to_string()}
            data-drop-enabled={can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <div>
                    <strong>{super::team_name(team)}</strong>
                    <span>{super::topology_count(team.members.len(), "agent", "agents")}</span>
                </div>
            </header>
            <div class="app-team-members">
                {for team.members.iter().filter_map(|member| {
                    let standing = standings.iter().find(|standing| standing.lane == member.lane)?;
                    Some(render_member(
                        member,
                        standing,
                        team.lane,
                        true,
                        sender,
                        can_configure,
                        topology_revision,
                    ))
                })}
            </div>
        </article>
    }
}

fn render_member(
    member: &crate::runtime::LaneMember,
    standing: &watchband::Standing,
    source: CanonicalUuid,
    linked: bool,
    sender: &SenderHandle,
    can_configure: bool,
    topology_revision: u64,
) -> Html {
    let lane = member.lane;
    let ondragstart = begin_drag(lane, can_configure);
    let onpointerdown = touch_drag_start(lane, can_configure, topology_revision);
    let onpointermove = touch_drag_movement(lane, source, can_configure, topology_revision);
    let onpointerup = touch_drag_finish(
        lane,
        source,
        sender.clone(),
        can_configure,
        topology_revision,
    );
    let cancel = touch_drag_cancel(lane);
    html! {
        <div
            class="app-topology-member"
            draggable={can_configure.to_string()}
            data-team-member={lane.to_string()}
            data-source-team={source.to_string()}
            {ondragstart}
            {onpointerdown}
            {onpointermove}
            {onpointerup}
            onpointercancel={cancel.clone()}
            onlostpointercapture={cancel}
            onclick={swallow_drag_click()}
        >
            {super::render_topology_agent(member, standing, linked)}
        </div>
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
            class="app-sheet-section app-team-drop"
            data-topology-drop={target.to_string()}
            data-drop-enabled={can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header><h2>{"New Team"}</h2><span>{"drop agent to create"}</span></header>
            <div class="app-drop-target" data-team-drop="new">{"Drop agent here"}</div>
        </section>
    }
}

struct SoloDropRender<'a> {
    standings: &'a [watchband::Standing],
    sender: &'a SenderHandle,
    can_configure: bool,
    topology_revision: u64,
    /// What the empty member list says: none unassigned, or what is awaited.
    empty: &'a str,
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
            class="app-sheet-section app-unassigned"
            data-topology-drop={UNASSIGNED_DROP}
            data-drop-enabled={render.can_configure.to_string()}
            {ondragover}
            {ondrop}
        >
            <header>
                <h2>{"Unassigned Agents"}</h2>
                <span>{"drop to unassign"}</span>
            </header>
            <div class="app-team-members" data-team-drop="unassigned">
                if members.is_empty() {
                    <p class="app-empty-state">{render.empty}</p>
                } else {
                    {for members.iter().filter_map(|member| {
                        let standing = render.standings.iter().find(|standing| standing.lane == member.lane)?;
                        Some(render_member(
                            member,
                            standing,
                            member.lane,
                            false,
                            render.sender,
                            render.can_configure,
                            render.topology_revision,
                        ))
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

/// Begin a finger or pen lift from a slot's name; a tap still opens the slot.
fn touch_drag_start(
    lane: CanonicalUuid,
    enabled: bool,
    topology_revision: u64,
) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        SWALLOW_CLICK.set(false);
        if event.pointer_type() == "mouse" || !event.is_primary() || !on_touch_handle(&event) {
            return;
        }
        // A primary contact going down ends any lift whose release never arrived.
        if TOUCH_DRAG.with_borrow_mut(watchband::PointerDragSession::clear) {
            clear_touch_presentation();
        }
        TOUCH_DRAG.with_borrow_mut(|session| {
            session.begin(
                enabled,
                lane,
                event.pointer_id(),
                event.client_x(),
                event.client_y(),
                topology_revision,
            )
        });
    })
}

fn on_touch_handle(event: &PointerEvent) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| {
            element
                .closest(".app-spice-topology-agent > header > strong")
                .ok()
                .flatten()
        })
        .is_some()
}

fn touch_drag_movement(
    lane: CanonicalUuid,
    source: CanonicalUuid,
    enabled: bool,
    topology_revision: u64,
) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        if TOUCH_DRAG.with_borrow_mut(|session| session.reconcile(enabled, Some(topology_revision)))
        {
            clear_touch_presentation();
            return;
        }
        let Some(lift) = TOUCH_DRAG.with_borrow_mut(|session| {
            let active = session.movement(
                lane,
                event.pointer_id(),
                event.client_x(),
                event.client_y(),
                topology_revision,
            )?;
            let lift = !active.dragging;
            active.dragging = true;
            Some(lift)
        }) else {
            return;
        };
        let Some(handle) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if lift
            && (handle.set_pointer_capture(event.pointer_id()).is_err()
                || begin_touch_presentation(&handle, event.client_x(), event.client_y()).is_none())
        {
            TOUCH_DRAG.with_borrow_mut(watchband::PointerDragSession::clear);
            clear_touch_presentation();
            return;
        }
        event.prevent_default();
        scroll_toward_edge(&handle, event.client_y());
        update_touch_ghost(event.client_x(), event.client_y());
        preview_touch_drop(lane, source, event.client_x(), event.client_y());
    })
}

fn touch_drag_finish(
    lane: CanonicalUuid,
    source: CanonicalUuid,
    sender: SenderHandle,
    enabled: bool,
    topology_revision: u64,
) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        let finished = TOUCH_DRAG.with_borrow_mut(|session| {
            if session.reconcile(enabled, Some(topology_revision)) {
                return None;
            }
            session.finish(lane, event.pointer_id(), topology_revision)
        });
        let Some(active) = finished.filter(|active| active.dragging) else {
            clear_touch_presentation();
            return;
        };
        let team =
            touch_drop_at(lane, source, event.client_x(), event.client_y()).map(|(team, _)| team);
        clear_touch_presentation();
        SWALLOW_CLICK.set(true);
        event.prevent_default();
        if let Some(team) = team {
            send_member(&sender, team, active.source, topology_revision);
        }
    })
}

fn touch_drag_cancel(lane: CanonicalUuid) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        if TOUCH_DRAG.with_borrow_mut(|session| session.cancel(lane, event.pointer_id())) {
            clear_touch_presentation();
        }
    })
}

fn swallow_drag_click() -> Callback<MouseEvent> {
    Callback::from(|event: MouseEvent| {
        if SWALLOW_CLICK.replace(false) {
            event.prevent_default();
            event.stop_propagation();
        }
    })
}

/// The team a lifted member would join if released here, beside the target
/// element that states it. Releasing where the member already is states nothing.
fn touch_drop_at(
    lane: CanonicalUuid,
    source: CanonicalUuid,
    client_x: i32,
    client_y: i32,
) -> Option<(CanonicalUuid, web_sys::Element)> {
    let (x, y) = (f64::from(client_x), f64::from(client_y));
    let targets = web_sys::window()?
        .document()?
        .query_selector_all("[data-topology-drop]")
        .ok()?;
    let target = (0..targets.length())
        .filter_map(|index| targets.item(index)?.dyn_into::<web_sys::Element>().ok())
        .find(|element| {
            let rect = element.get_bounding_client_rect();
            x >= rect.left() && x <= rect.right() && y >= rect.top() && y <= rect.bottom()
        })?;
    let value = target.get_attribute("data-topology-drop")?;
    let team = if value == UNASSIGNED_DROP {
        lane
    } else {
        value.parse().ok()?
    };
    (team != source).then_some((team, target))
}

fn preview_touch_drop(lane: CanonicalUuid, source: CanonicalUuid, client_x: i32, client_y: i32) {
    remove_touch_attribute("data-topology-drop-target");
    if let Some((_, target)) = touch_drop_at(lane, source, client_x, client_y) {
        let _ = target.set_attribute("data-topology-drop-target", "true");
    }
}

fn begin_touch_presentation(handle: &web_sys::Element, client_x: i32, client_y: i32) -> Option<()> {
    clear_touch_presentation();
    let source = handle.closest(".app-topology-member").ok()??;
    let rect = source.get_bounding_client_rect();
    source
        .set_attribute("data-topology-drag-source", "true")
        .ok()?;
    let ghost = source
        .clone_node_with_deep(true)
        .ok()?
        .dyn_into::<web_sys::HtmlElement>()
        .ok()?;
    ghost.set_class_name("app-topology-member app-topology-drag-ghost");
    for attribute in ["data-topology-drag-source", "draggable", "data-team-member"] {
        let _ = ghost.remove_attribute(attribute);
    }
    let _ = ghost.set_attribute("aria-hidden", "true");
    let _ = ghost.set_attribute("inert", "");
    let _ = ghost.set_attribute(
        "data-touch-offset-x",
        &(f64::from(client_x) - rect.left()).to_string(),
    );
    let _ = ghost.set_attribute(
        "data-touch-offset-y",
        &(f64::from(client_y) - rect.top()).to_string(),
    );
    let _ = ghost
        .style()
        .set_property("width", &format!("{}px", rect.width()));
    web_sys::window()?
        .document()?
        .body()?
        .append_child(&ghost)
        .ok()?;
    Some(())
}

fn update_touch_ghost(client_x: i32, client_y: i32) {
    let Some(ghost) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| {
            document
                .query_selector(".app-topology-drag-ghost")
                .ok()
                .flatten()
        })
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let offset = |name| {
        ghost
            .get_attribute(name)
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or_default()
    };
    let left = f64::from(client_x) - offset("data-touch-offset-x");
    let top = f64::from(client_y) - offset("data-touch-offset-y");
    let _ = ghost.style().set_property("left", &format!("{left}px"));
    let _ = ghost.style().set_property("top", &format!("{top}px"));
}

/// Scroll the nearest scrolling ancestor while the finger rests near its
/// visible edge, so a target outside the visible sheet stays reachable.
fn scroll_toward_edge(handle: &web_sys::Element, client_y: i32) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let mut ancestor = handle.parent_element();
    while let Some(element) = ancestor {
        let scrolls = element.scroll_height() > element.client_height()
            && window
                .get_computed_style(&element)
                .ok()
                .flatten()
                .and_then(|style| style.get_property_value("overflow-y").ok())
                .is_some_and(|overflow| overflow == "auto" || overflow == "scroll");
        if scrolls {
            let rect = element.get_bounding_client_rect();
            let viewport = window
                .inner_height()
                .ok()
                .and_then(|height| height.as_f64())
                .unwrap_or(rect.bottom());
            let top = rect.top().max(0.0) + TOUCH_SCROLL_EDGE_PX;
            let bottom = rect.bottom().min(viewport) - TOUCH_SCROLL_EDGE_PX;
            let y = f64::from(client_y);
            let depth = if y < top {
                y - top
            } else if y > bottom {
                y - bottom
            } else {
                return;
            };
            element.scroll_by_with_x_and_y(0.0, depth * TOUCH_SCROLL_DEPTH_RATE);
            return;
        }
        ancestor = element.parent_element();
    }
}

fn remove_touch_attribute(attribute: &str) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Ok(nodes) = document.query_selector_all(&format!("[{attribute}]")) else {
        return;
    };
    for index in 0..nodes.length() {
        if let Some(element) = nodes
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        {
            let _ = element.remove_attribute(attribute);
        }
    }
}

fn clear_touch_presentation() {
    remove_touch_attribute("data-topology-drag-source");
    remove_touch_attribute("data-topology-drop-target");
    if let Some(document) = web_sys::window().and_then(|window| window.document())
        && let Ok(ghosts) = document.query_selector_all(".app-topology-drag-ghost")
    {
        for index in 0..ghosts.length() {
            if let Some(ghost) = ghosts
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            {
                ghost.remove();
            }
        }
    }
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
            fast_mode: None,
        },
    )
}
