//! Org-local actions opened by one lineage toolbar's hamburger.

use super::controls::LineageControl;
use super::*;

pub(super) struct Context<'a> {
    pub(super) controls: &'a UseReducerHandle<LineageControls>,
    pub(super) sender: &'a SenderHandle,
    pub(super) topology: &'a [(CanonicalUuid, Vec<CanonicalUuid>)],
    pub(super) topology_revision: u64,
    pub(super) can_configure: bool,
    pub(super) can_arrange: bool,
}

pub(super) fn trigger(
    lineage: CanonicalUuid,
    control: &LineageControl,
    controls: &UseReducerHandle<LineageControls>,
) -> Html {
    let toggle = controls.clone();
    html! {
        <button
            class="organization-menu"
            type="button"
            aria-label={format!("Organization actions for {}", watchband::short(lineage))}
            aria-haspopup="menu"
            aria-expanded={(control.menu_open || control.import_open).to_string()}
            title="Organization actions"
            onclick={Callback::from(move |_| {
                toggle.dispatch(LineageControlAction::ToggleMenu { lineage });
            })}
        ><span class="organization-menu-icon" aria-hidden="true"></span></button>
    }
}

pub(super) fn render(
    lineage: &crate::runtime::LaneRow,
    control: &LineageControl,
    context: &Context<'_>,
) -> Html {
    if control.arrangement_open {
        return render_arrangement(lineage.lane, context);
    }
    if control.import_open {
        return render_import(lineage, context);
    }
    if !control.menu_open {
        return Html::default();
    }
    let identity = lineage.lane;
    let member_count = lineage.members.len();
    let close = context.controls.clone();
    let import = context.controls.clone();
    let split = watchband::split_lineage(context.topology, identity).map(|rows| topology(&rows));
    let split_enabled = context.can_arrange && split.is_some();
    let split_controls = context.controls.clone();
    let split_sender = context.sender.clone();
    let original = topology(context.topology);
    let restore = control.previous_topology.clone();
    let restore_count = restore
        .as_ref()
        .and_then(|rows| rows.iter().find(|row| row.lineage_id == identity))
        .map_or(0, |row| row.lanes.len());
    let restore_enabled = context.can_arrange && restore.is_some();
    let restore_controls = context.controls.clone();
    let restore_sender = context.sender.clone();
    let topology_revision = context.topology_revision;
    html! {
        <div
            class="organization-action-menu"
            role="menu"
            aria-label={format!("Actions for organization {}", watchband::short(identity))}
        >
            <button
                type="button"
                class="organization-action"
                role="menuitem"
                onclick={Callback::from(move |_| {
                    close.dispatch(LineageControlAction::CloseLineage { lineage: identity });
                })}
            >
                <span class="organization-action-label">{"Close team"}</span>
                <span class="organization-action-detail">{members(member_count)}</span>
            </button>
            <button
                type="button"
                class="organization-action"
                role="menuitem"
                disabled={!context.can_configure}
                onclick={Callback::from(move |_| {
                    import.dispatch(LineageControlAction::ToggleImport { lineage: identity });
                })}
            >
                <span class="organization-action-label">{"Import agent"}</span>
                <span class="organization-action-detail">{"choose agent"}</span>
            </button>
            <button
                type="button"
                class="organization-action"
                role="menuitem"
                disabled={!split_enabled}
                onclick={Callback::from(move |_| {
                    let Some(lineages) = split.clone() else { return; };
                    split_controls.dispatch(LineageControlAction::SaveSplit {
                        lineage: identity,
                        topology: original.clone(),
                    });
                    send_action(
                        &split_sender,
                        ClientAction::ArrangeLineages {
                            topology_revision,
                            lineages,
                        },
                    );
                })}
            >
                <span class="organization-action-label">{"Split into individuals"}</span>
                <span class="organization-action-detail">{
                    if split_enabled { "one team per agent" } else { "already individual" }
                }</span>
            </button>
            <button
                type="button"
                class="organization-action"
                role="menuitem"
                disabled={!restore_enabled}
                onclick={Callback::from(move |_| {
                    let Some(lineages) = restore.clone() else { return; };
                    restore_controls.dispatch(LineageControlAction::ClearRestore {
                        lineage: identity,
                    });
                    send_action(
                        &restore_sender,
                        ClientAction::ArrangeLineages {
                            topology_revision,
                            lineages,
                        },
                    );
                })}
            >
                <span class="organization-action-label">{"Restore previous organization"}</span>
                <span class="organization-action-detail">{
                    if restore_enabled { members(restore_count) } else { "no saved subgroup".to_owned() }
                }</span>
            </button>
        </div>
    }
}

fn render_arrangement(lineage: CanonicalUuid, context: &Context<'_>) -> Html {
    let choices = context
        .topology
        .iter()
        .filter(|(target, _)| *target != lineage)
        .flat_map(|(target, _)| {
            [
                ("before", "Move before", watchband::LineageDrop::Reorder),
                ("after", "Move after", watchband::LineageDrop::ReorderAfter),
                ("join", "Join into", watchband::LineageDrop::Join),
            ]
            .into_iter()
            .filter_map(|(kind, verb, drop)| {
                watchband::arrangement(context.topology, lineage, *target, drop)
                    .map(|rows| (kind, verb, *target, topology(&rows)))
            })
            .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    html! {
        <div
            class="organization-action-menu organization-arrangement-menu"
            role="menu"
            aria-label={format!("Arrange organization {}", watchband::short(lineage))}
        >
            {for choices.into_iter().map(|(kind, verb, target, lineages)| {
                let sender = context.sender.clone();
                let controls = context.controls.clone();
                let topology_revision = context.topology_revision;
                html! {
                    <button
                        type="button"
                        class="organization-action organization-arrangement-action"
                        role="menuitem"
                        data-lineage-arrangement={kind}
                        data-lineage-arrangement-target={target.to_string()}
                        onclick={Callback::from(move |_| {
                            send_action(
                                &sender,
                                ClientAction::ArrangeLineages {
                                    topology_revision,
                                    lineages: lineages.clone(),
                                },
                            );
                            controls.dispatch(LineageControlAction::CloseMenus);
                        })}
                    >
                        <span class="organization-action-label">{format!("{verb} organization {}", watchband::short(target))}</span>
                        <span class="organization-action-detail">{
                            if kind == "join" { "combine agents" } else { "reorder organizations" }
                        }</span>
                    </button>
                }
            })}
        </div>
    }
}

fn render_import(lineage: &crate::runtime::LaneRow, context: &Context<'_>) -> Html {
    let identity = lineage.lane;
    let current = lineage
        .members
        .iter()
        .map(|member| member.lane)
        .collect::<BTreeSet<_>>();
    let candidates = context
        .topology
        .iter()
        .flat_map(|(_, members)| members)
        .copied()
        .filter(|member| !current.contains(member))
        .collect::<Vec<_>>();
    let close = context.controls.clone();
    let topology_revision = context.topology_revision;
    html! {
        <div
            class="organization-action-menu organization-import-menu"
            role="menu"
            aria-label={format!("Import agent into organization {}", watchband::short(identity))}
        >
            <header>
                <strong>{"Import agent"}</strong>
                <button
                    type="button"
                    onclick={Callback::from(move |_| {
                        close.dispatch(LineageControlAction::ToggleImport { lineage: identity });
                    })}
                >{"Cancel"}</button>
            </header>
            if candidates.is_empty() {
                <p class="organization-import-empty">{"No available agents"}</p>
            } else {
                {for candidates.into_iter().map(|member| {
                    let sender = context.sender.clone();
                    let controls = context.controls.clone();
                    html! {
                        <button
                            type="button"
                            class="organization-import-choice"
                            role="menuitem"
                            onclick={Callback::from(move |_| {
                                send_action(
                                    &sender,
                                    ClientAction::ConfigureLineage {
                                        topology_revision,
                                        lineage: identity,
                                        member: Some(member),
                                        life: None,
                                    },
                                );
                                controls.dispatch(LineageControlAction::ToggleImport {
                                    lineage: identity,
                                });
                            })}
                        >
                            <span>{format!("Agent {}", watchband::short(member))}</span>
                            <span>{"Import"}</span>
                        </button>
                    }
                })}
            }
        </div>
    }
}

fn topology(rows: &[(CanonicalUuid, Vec<CanonicalUuid>)]) -> Vec<LineageArrangement> {
    rows.iter()
        .map(|(lineage_id, lanes)| LineageArrangement {
            lineage_id: *lineage_id,
            lanes: lanes.clone(),
        })
        .collect()
}

fn members(count: usize) -> String {
    format!("{count} {}", if count == 1 { "agent" } else { "agents" })
}
