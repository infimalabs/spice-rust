//! Spice sidebar content and team-arrangement controls.

use web_sys::DragEvent;

use super::*;
use crate::watchband;

const MEMBER_MIME: &str = "application/x-spice-team-member";

pub(super) struct Context<'a> {
    pub(super) sender: &'a SenderHandle,
}

/// One interactive topology: compact status tiles double as draggable members.
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

pub(super) fn render_global_settings(runtime: Option<&AppRuntime>, sender: &SenderHandle) -> Html {
    let settings = runtime.and_then(AppRuntime::settings);
    let fast_mode = settings.is_some_and(|settings| settings.fast_mode);
    let operator_name = settings.and_then(|settings| settings.operator_name.clone());
    let operator_email = settings.and_then(|settings| settings.operator_email.clone());
    let enabled = settings.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::GlobalSettingsSet));
    let sender = sender.clone();
    let onclick = Callback::from(move |event: MouseEvent| {
        if enabled
            && admit_action(
                &sender,
                ClientAction::GlobalSettings(GlobalSettingsSet {
                    fast_mode: !fast_mode,
                    operator_name: operator_name.clone(),
                    operator_email: operator_email.clone(),
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
        (true, true, false) => "Basic: press to run every agent in fast mode",
    };
    html! {
        <section class="app-sheet-section app-spice-settings">
            <h2>{"Settings"}</h2>
            <div class="app-spice-settings-controls">
                <chrome::TurboButton
                    class={classes!("app-spice-turbo")}
                    engaged={fast_mode}
                    label={format!(
                        "Global fast mode: {}",
                        if fast_mode { "Turbo" } else { "Basic" },
                    )}
                    disabled={!enabled}
                    title={title}
                    onclick={onclick}
                />
                {render_theme_selector()}
            </div>
        </section>
    }
}

/// The cumulative plan-weighted weekly quota remaining across accounts, placed
/// beside the open sidebar's Spice button. Its fixed-width fallback keeps the
/// button still while quota state loads.
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

/// The operator's avatar and override form. Until settings arrive the same
/// form stands disabled and says it is loading, so later admission moves no
/// section below it.
pub(super) fn render_operator_settings(
    runtime: Option<&AppRuntime>,
    sender: &SenderHandle,
) -> Html {
    const LOADING: &str = "loading";
    let settings = runtime.and_then(AppRuntime::settings);
    let unresolved = wire::OperatorIdentity::default();
    let operator = settings.map_or(&unresolved, |settings| &settings.operator);
    let enabled = settings.is_some()
        && runtime.is_some_and(|runtime| runtime.offers(CommandKind::GlobalSettingsSet));
    let name = settings
        .and_then(|settings| settings.operator_name.clone())
        .unwrap_or_default();
    let email = settings
        .and_then(|settings| settings.operator_email.clone())
        .unwrap_or_default();
    let (name_placeholder, email_placeholder) = match settings {
        Some(_) => (
            operator
                .name
                .clone()
                .unwrap_or_else(|| "Git user.name is not set".to_owned()),
            operator
                .email
                .clone()
                .unwrap_or_else(|| "Git user.email is not set".to_owned()),
        ),
        None => (LOADING.to_owned(), LOADING.to_owned()),
    };
    let source = |explicit: Option<&Option<String>>| match explicit {
        None => LOADING,
        Some(Some(_)) => "explicit",
        Some(None) => "Git",
    };
    let name_source = source(settings.map(|settings| &settings.operator_name));
    let email_source = source(settings.map(|settings| &settings.operator_email));
    let fast_mode = settings.is_some_and(|settings| settings.fast_mode);
    let sender = sender.clone();
    let onsubmit = Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        event.stop_propagation();
        if !enabled {
            return;
        }
        let setting = |id| {
            input_value(id).and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            })
        };
        let _ = admit_action(
            &sender,
            ClientAction::GlobalSettings(GlobalSettingsSet {
                fast_mode,
                operator_name: setting("operator-name"),
                operator_email: setting("operator-email"),
            }),
        );
    });
    html! {
        <section class="app-sheet-section app-spice-operator" data-operator-member="">
            <h2>{"You"}</h2>
            <div class="app-spice-operator-identity">
                {super::render_operator_avatar(operator)}
                <form class="app-operator-form" {onsubmit}>
                <label for="operator-name">
                    <span>{"Name"}</span>
                    <small>{name_source}</small>
                </label>
                <input
                    id="operator-name"
                    type="text"
                    value={name}
                    placeholder={name_placeholder}
                    autocomplete="name"
                    disabled={!enabled}
                />
                <label for="operator-email">
                    <span>{"Email"}</span>
                    <small>{email_source}</small>
                </label>
                <input
                    id="operator-email"
                    type="email"
                    value={email}
                    placeholder={email_placeholder}
                    autocomplete="email"
                    disabled={!enabled}
                />
                <button type="submit" class="app-compact-control" disabled={!enabled}>
                    {"Save identity"}
                </button>
                </form>
            </div>
        </section>
    }
}

fn input_value(id: &str) -> Option<String> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<web_sys::HtmlInputElement>()
        .ok()
        .map(|input| input.value())
}

pub(super) fn render_resident(
    connection: &Connection,
    runtime: Option<&AppRuntime>,
    cursor: &str,
    identity: Option<&AppIdentity>,
    update: Option<&AppUpdate>,
) -> Html {
    let status = match connection {
        Connection::Starting => "Starting Spice".to_owned(),
        Connection::Connecting => "Opening current resident".to_owned(),
        Connection::Online { .. } => "Connected".to_owned(),
        Connection::Offline(reason) => format!("Reconnecting / {reason}"),
    };
    let epoch = runtime
        .and_then(AppRuntime::lanes)
        .map(|lanes| lanes.epoch.to_string())
        .or_else(|| match connection {
            Connection::Online { epoch } => Some(epoch.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "loading".to_owned());
    html! {
        <section
            class="app-sheet-section app-spice-resident"
            data-connection={connection.indicator_state()}
        >
            <h2>{"Resident"}</h2>
            <dl class="app-spice-resident-facts">
                <div>
                    <dt>{"Status"}</dt>
                    <dd>{status}</dd>
                </div>
                <div>
                    <dt>{"Epoch"}</dt>
                    <dd>{epoch}</dd>
                </div>
            </dl>
            {render_version_footer(cursor, identity, update)}
        </section>
    }
}
fn render_version_footer(
    cursor: &str,
    identity: Option<&AppIdentity>,
    update: Option<&AppUpdate>,
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
    };
    html! {
        <footer class="app-resident-footer">
            <div class="app-product-version-group">
                {version}
            </div>
            <span class="bar-meta app-resident-cursor" aria-label="Global cursor">
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

fn render_theme_selector() -> Html {
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
                        can_configure,
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
    can_configure: bool,
) -> Html {
    let ondragstart = begin_drag(member.lane, can_configure);
    html! {
        <div
            class="app-topology-member"
            draggable={can_configure.to_string()}
            data-team-member={member.lane.to_string()}
            data-source-team={source.to_string()}
            {ondragstart}
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
                            render.can_configure,
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
