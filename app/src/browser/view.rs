//! Canvas surface rendering for the operator browser app.

use super::*;

mod chrome;

use chrome::Correspondent;

#[derive(Clone)]
pub(super) struct AgentControls {
    pub(super) selected_lane: UseStateHandle<String>,
    pub(super) seat_worktree: UseStateHandle<String>,
    pub(super) seat_vendor: UseStateHandle<String>,
    pub(super) seat_life: UseStateHandle<String>,
    pub(super) seat_notice: UseStateHandle<Notice>,
    pub(super) submit_seat: Callback<SubmitEvent>,
    /// Read one message aloud, using the resident's current speech settings.
    pub(super) speak: Callback<String>,
}

/// Render the reason the last submission was refused, or nothing.
///
/// The node is absent rather than empty while a submission stands. `role`
/// `alert` is an assertive live region, so an element that were always present
/// would be re-announced empty on every keystroke; leaving it out until there
/// is a sentence makes the announcement coincide with the refusal.
fn render_notice(notice: &Notice) -> Html {
    if !notice.is_some() {
        return Html::default();
    }
    html! {
        <p class="form-refusal" role="alert">{notice.to_string()}</p>
    }
}

pub(super) struct CanvasRender<'a> {
    pub connection: &'a UseStateHandle<Connection>,
    pub surface: &'a UseStateHandle<SurfaceData>,
    pub axis: &'a UseStateHandle<Axis>,
    pub mosaic: &'a UseStateHandle<bool>,
    pub composer_drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub sender: &'a SenderHandle,
    pub agents: &'a AgentControls,
    pub note_task: &'a UseStateHandle<String>,
    pub note_text: &'a UseStateHandle<String>,
    pub submit_note: Callback<SubmitEvent>,
    pub refresh: Callback<MouseEvent>,
    pub speech: &'a UseStateHandle<wire::SpeechPlayback>,
    pub speech_status: &'a UseStateHandle<SpeechStatus>,
    pub toggle_speech: Callback<MouseEvent>,
}

pub(super) fn render_canvas(render: CanvasRender<'_>) -> Html {
    let active_axis = **render.axis;
    let cursor = (**render.surface)
        .as_ref()
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <div class="shell">
            {chrome::render_command_bar(
                render.axis,
                render.mosaic,
                active_axis,
                render.surface,
                &cursor,
            )}
            {render_workspace(&render, active_axis)}
        </div>
    }
}

pub(super) fn render_workspace(render: &CanvasRender<'_>, active_axis: Axis) -> Html {
    html! {
        <div class="workspace">
            <header class="topbar">
                <div class="mobile-brand">{"Spice Canvas"}</div>
                <div class={render.connection.class()}>
                    <span class="status-dot"></span>
                    <span class="status-copy">{render.connection.label()}</span>
                </div>
                <div class="topbar-actions">
                    {speech_control(
                        render.speech,
                        render.speech_status,
                        render.toggle_speech.clone(),
                    )}
                    <button
                        class="refresh"
                        type="button"
                        onclick={render.refresh.clone()}
                    >{"Refresh"}</button>
                </div>
            </header>
            <main class="content">
                if **render.mosaic {
                    {render_agent_console(render.surface, render.agents)}
                    {composer::render_mosaic(
                        render.surface,
                        render.sender,
                        render.composer_drafts,
                        &|lane, runtime| {
                            lane_inbox(lane, runtime, render.agents, render.composer_drafts)
                        },
                    )}
                } else {
                    {render_analytics(render, active_axis)}
                }
            </main>
        </div>
    }
}

fn render_analytics(render: &CanvasRender<'_>, active_axis: Axis) -> Html {
    let runtime = (**render.surface).as_ref();
    let task_count = runtime
        .and_then(CanvasRuntime::board)
        .map_or(0, |board| board.tasks.len());
    let active_tasks = runtime
        .and_then(CanvasRuntime::board)
        .map_or(0, |board| board.active_public_tasks().count());
    let lane_count = runtime
        .and_then(CanvasRuntime::lanes)
        .map_or(0, |lanes| lanes.lanes.len());
    let pending_count = runtime
        .and_then(CanvasRuntime::consent)
        .map_or(0, |consent| consent.pending.len());
    let cursor = runtime
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <>
            <div class="page-heading">
                <div>
                    <h1>{title(active_axis.as_str())}</h1>
                    <p>{active_axis.question()}</p>
                </div>
            </div>
            <section class="metrics" aria-label="Current resident metrics">
                {metric("Tasks", task_count.to_string(), format!("{active_tasks} active"))}
                {metric("Lanes", lane_count.to_string(), resident_state(render.surface))}
                {metric(
                    "Consent",
                    pending_count.to_string(),
                    "pending decisions".to_owned(),
                )}
                {metric("Journal", cursor, "admitted stream cursor".to_owned())}
            </section>
            <div class="pane-strip" aria-label="Panes on this axis">
                {for active_axis.panes().map(|pane| html! {
                    <span class="pane-chip">{title(pane.as_str())}</span>
                })}
            </div>
            {render_axis(
                active_axis,
                render.surface,
                render.connection,
                render.sender,
                render.note_task,
                render.note_text,
                render.submit_note.clone(),
            )}
        </>
    }
}

/// The operator's primary surface: current lanes, one selected conversation,
/// and the controls that create or direct an agent without leaving Canvas.
pub(super) fn render_agent_console(surface: &SurfaceData, controls: &AgentControls) -> Html {
    let runtime = surface.as_ref();
    let lanes = runtime
        .and_then(CanvasRuntime::lanes)
        .map(|lanes| lanes.lanes.as_slice())
        .unwrap_or(&[]);
    html! {
        <section class="agent-console" data-pane="agent-console">
            <div class="agent-console-heading">
                <div>
                    <span class="eyebrow">{"Live workspace"}</span>
                    <h1>{"Agents"}</h1>
                    <p>{"Seat a lane here; direct it from its own tile below."}</p>
                </div>
                <span class="agent-count">{format!("{} active", lanes.len())}</span>
            </div>
            {chrome::render_watchband(surface, controls)}
            {chrome::render_ticker(surface)}
            {render_seat_panel(runtime, controls)}
        </section>
    }
}

/// The packed inbox belonging to one seated lane.
///
/// Every lane gets its own, which is what the predecessor did: each lane there
/// owned an engine-packed plane keyed to itself, rather than one plane
/// following whichever lane was selected. Quoting a card therefore has an
/// unambiguous destination -- the draft of the lane the card is waiting on --
/// so this builds the correspondent from the lane rather than from a selection.
pub(super) fn lane_inbox(
    lane: &crate::runtime::LaneRow,
    runtime: &CanvasRuntime,
    controls: &AgentControls,
    drafts: &UseReducerHandle<composer::ComposerDrafts>,
) -> Html {
    let mut cards = Vec::new();
    for member in &lane.members {
        let Some(pending) = runtime
            .inbox()
            .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == member.lane))
        else {
            continue;
        };
        let lane_key = lane.lane.to_string();
        let drafts = drafts.clone();
        let correspondent = Correspondent {
            name: crate::watchband::leaf(&member.worktree).to_owned(),
            accent: crate::watchband::accent(member.lane),
            speak: controls.speak.clone(),
            quote: Callback::from(move |text: String| {
                drafts.dispatch(composer::ComposerAction::AddQuote {
                    lane: lane_key.clone(),
                    quote: text,
                });
            }),
        };
        cards.extend(pending.pending.iter().cloned().map(|directive| InboxCard {
            directive,
            correspondent: correspondent.clone(),
        }));
    }
    html! { <MosaicInbox {cards} /> }
}

/// Which lane the watchband is currently showing as its selected tab.
///
/// The band is an ARIA tablist, so exactly one tab carries the selection even
/// before the operator has chosen; falling back to the first seated lane is
/// what keeps that true on a fresh load. Nothing else reads the selection now
/// that each lane carries its own inbox and composer.
fn selected_agent(lanes: &[crate::runtime::LaneRow], requested: &str) -> Option<CanonicalUuid> {
    requested
        .parse::<CanonicalUuid>()
        .ok()
        .and_then(|requested| {
            lanes
                .iter()
                .flat_map(|team| &team.members)
                .find(|member| member.lane == requested)
                .map(|member| member.lane)
        })
        .or_else(|| {
            lanes
                .first()
                .and_then(|team| team.host())
                .map(|member| member.lane)
        })
}

#[derive(Properties, PartialEq)]
struct MosaicInboxProps {
    cards: Vec<InboxCard>,
}

#[derive(Clone, PartialEq)]
struct InboxCard {
    directive: wire::intent::PendingDirective,
    correspondent: Correspondent,
}

#[function_component(MosaicInbox)]
fn mosaic_inbox(properties: &MosaicInboxProps) -> Html {
    let host = use_node_ref();
    let width = use_element_width(host.clone());
    let geometry = crate::mosaic::Geometry::new(16.0, *width);
    let candidate_sets = properties
        .cards
        .iter()
        .map(|card| message_candidates(&card.directive, &geometry))
        .collect::<Vec<_>>();
    let cards = crate::mosaic::pack(&candidate_sets, crate::mosaic::TRACK_COUNT);
    let (max_row, _) = crate::mosaic::extent(&cards);
    use_scroll_compensation(
        host.clone(),
        max_row,
        geometry.module,
        (*width).round() as i32,
    );
    let feed = classes!(
        "conversation-feed",
        (!properties.cards.is_empty()).then_some("mosaic-feed"),
    );
    html! {
        <div
            ref={host}
            class={feed}
            aria-live="polite"
            data-lanes={geometry.lane_count.to_string()}
        >
            if properties.cards.is_empty() {
                <div class="conversation-empty">
                    <strong>{"Inbox clear"}</strong>
                    <p>{"Pending direction and acknowledgement state appear here."}</p>
                </div>
            } else {
                {for properties.cards.iter().zip(cards.iter()).map(|(entry, card)| {
                    chrome::render_mosaic_card(
                        (&entry.directive, card),
                        max_row,
                        &entry.correspondent,
                    )
                })}
            }
        </div>
    }
}

fn message_candidates(
    directive: &wire::intent::PendingDirective,
    geometry: &crate::mosaic::Geometry,
) -> Vec<crate::mosaic::Candidate> {
    crate::mosaic::span_candidates(geometry, |span| {
        let width = f64::from(geometry.card_width(0, span).max(160));
        let characters_per_line = (width / 7.2).floor().max(18.0) as usize;
        let text_lines = directive
            .text
            .lines()
            .map(|line| line.chars().count().max(1).div_ceil(characters_per_line))
            .sum::<usize>()
            .max(1);
        let attachment_height = directive
            .attachments
            .iter()
            .map(|attachment| {
                if attachment.content_type.starts_with("image/") {
                    crate::mosaic::Reservation::Image.pixels(16.0) + 10.0
                } else {
                    42.0
                }
            })
            .sum::<f64>();
        54.0 + text_lines as f64 * 19.0 + attachment_height
    })
}

#[hook]
fn use_element_width(host: NodeRef) -> UseStateHandle<f64> {
    let width = use_state_eq(|| 0.0);
    {
        let width = width.clone();
        use_effect_with(host, move |host| {
            let registration = host.cast::<web_sys::Element>().and_then(|element| {
                width.set(f64::from(element.client_width()));
                let observed = element.clone();
                let width = width.clone();
                let callback = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
                    move |_, _| width.set(f64::from(observed.client_width())),
                );
                let observer =
                    web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()).ok()?;
                observer.observe(&element);
                Some((observer, callback))
            });
            move || {
                if let Some((observer, _callback)) = registration {
                    observer.disconnect();
                }
            }
        });
    }
    width
}

#[hook]
fn use_scroll_compensation(host: NodeRef, max_row: i32, module: i32, width: i32) {
    let previous = use_mut_ref(|| None::<(i32, i32)>);
    use_effect_with(
        (max_row, module, width),
        move |&(max_row, module, width)| {
            if let Some(element) = host.cast::<web_sys::HtmlElement>() {
                let prior = *previous.borrow();
                if let Some((previous_max_row, previous_width)) = prior {
                    let scrolled =
                        crate::mosaic::pane_is_scrolled(f64::from(element.scroll_top()), None);
                    let replaying = previous_width != width;
                    if let Some(delta) = crate::mosaic::scroll_compensation(
                        previous_max_row,
                        max_row,
                        module,
                        scrolled,
                        replaying,
                    ) {
                        element.set_scroll_top(element.scroll_top() + delta.round() as i32);
                    }
                }
                *previous.borrow_mut() = Some((max_row, width));
            }
            || ()
        },
    );
}

fn render_seat_panel(runtime: Option<&CanvasRuntime>, controls: &AgentControls) -> Html {
    let can_seat = runtime.is_some_and(|runtime| runtime.offers(CommandKind::LaneSeat));
    let ready = can_seat
        && !controls.seat_worktree.trim().is_empty()
        && !controls.seat_vendor.trim().is_empty()
        && !controls.seat_life.trim().is_empty();
    html! {
        <aside class="seat-panel">
            <div class="seat-panel-heading">
                <span class="eyebrow">{"Add agent"}</span>
                <h2>{"Seat a lane"}</h2>
                <p>{"Use an existing worktree in this repository."}</p>
            </div>
            <form class="seat-form" onsubmit={controls.submit_seat.clone()}>
                <label for="seat-worktree">{"Worktree"}</label>
                <input id="seat-worktree" type="text" placeholder="/absolute/path/to/worktree"
                    value={(*controls.seat_worktree).clone()}
                    oninput={state_from_input(controls.seat_worktree.clone())} />
                <label for="seat-vendor">{"Vendor"}</label>
                <input id="seat-vendor" type="text" value={(*controls.seat_vendor).clone()}
                    oninput={state_from_input(controls.seat_vendor.clone())} />
                <label for="seat-life">{"Life"}</label>
                <input id="seat-life" type="text" list="lane-lives"
                    value={(*controls.seat_life).clone()}
                    oninput={state_from_input(controls.seat_life.clone())} />
                <datalist id="lane-lives">
                    <option value="steer" /><option value="drive" /><option value="drain" />
                </datalist>
                <button type="submit" disabled={!ready}>{"Seat agent"}</button>
                {render_notice(&controls.seat_notice)}
            </form>
        </aside>
    }
}

pub(super) fn speech_control(
    speech: &wire::SpeechPlayback,
    status: &SpeechStatus,
    toggle: Callback<MouseEvent>,
) -> Html {
    if matches!(speech, wire::SpeechPlayback::Disabled {}) {
        return Html::default();
    }
    let unavailable = matches!(
        speech,
        wire::SpeechPlayback::External {
            available: false,
            ..
        }
    );
    let label = if unavailable {
        "Speech unavailable"
    } else {
        status.label()
    };
    let title = if unavailable {
        "External speech backend is not configured".to_owned()
    } else {
        status.title()
    };
    html! {
        <>
            <button
                class={classes!("speech-button", status.active().then_some("stop"))}
                type="button"
                aria-label={label}
                title={title}
                data-state={status.state()}
                disabled={unavailable}
                onclick={toggle}
            >
                <span class="speech-icon" aria-hidden="true"></span>
            </button>
            <span class="visually-hidden" aria-live="polite">{status.title()}</span>
        </>
    }
}

pub(super) fn render_axis(
    axis: Axis,
    surface: &SurfaceData,
    connection: &Connection,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    match axis {
        Axis::Magnitude => render_magnitude(surface),
        Axis::Flow => render_flow(surface, note_task, note_text, submit_note),
        Axis::Topology => render_topology(surface),
        Axis::Chronology => render_chronology(surface, sender, connection),
    }
}

pub(super) fn render_magnitude(surface: &SurfaceData) -> Html {
    let Some(consent) = surface.as_ref().and_then(CanvasRuntime::consent) else {
        return empty("No magnitude snapshot is available.");
    };
    html! {
        <section class="section">
            <div class="section-heading">
                <h2>{"Consent budgets"}</h2>
                <span class="section-count">{format!("{} rules", consent.budgets.len())}</span>
            </div>
            if consent.budgets.is_empty() {
                {empty("No bounded consent rules are active.")}
            } else {
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Rule"}</th><th>{"Remaining"}</th><th>{"Bound"}</th>
                    </tr></thead>
                    <tbody>{for consent.budgets.iter().map(|budget| html! {
                        <tr>
                            <td class="mono">{budget.rule.to_string()}</td>
                            <td>{budget.remaining}</td>
                            <td>{budget.bound}</td>
                        </tr>
                    })}</tbody>
                </table></div>
            }
        </section>
    }
}

pub(super) fn render_flow(
    surface: &SurfaceData,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    let Some(board) = surface.as_ref().and_then(CanvasRuntime::board) else {
        return empty("No board snapshot is available.");
    };
    let can_note = surface
        .as_ref()
        .is_some_and(|runtime| runtime.offers(CommandKind::TaskNote));
    let selected = (**note_task).clone();
    let note = (**note_text).clone();
    html! {
        <div class="split">
            <section class="section">
                <div class="section-heading">
                    <h2>{"Current work"}</h2>
                    <span class="section-count">{format!("{} tasks", board.tasks.len())}</span>
                </div>
                <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Task"}</th><th>{"Phase"}</th><th>{"Owner"}</th><th>{"Project"}</th>
                    </tr></thead>
                    <tbody>{for board.tasks.iter().map(|task| {
                        let statement = &task.statement.body;
                        let workflow = &task.workflow.body;
                        let title = task.presentation_title().to_owned();
                        html! {
                            <tr>
                                <td class="title-cell">
                                    <div class="mono muted">{statement.handle.to_string()}</div>
                                    <div>{title}</div>
                                </td>
                                <td><span class={phase_class(&workflow.status)}>
                                    {workflow.phase.to_string()}
                                </span></td>
                                <td class="mono">{
                                    workflow
                                        .assignment
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| "-".to_owned())
                                }</td>
                                <td>{statement.project.clone()}</td>
                            </tr>
                        }
                    })}</tbody>
                </table></div>
            </section>
            if can_note {
                <section class="section">
                    <div class="section-heading"><h2>{"Append observation"}</h2></div>
                    <form class="form" onsubmit={submit_note}>
                    <label for="note-task">{"Task"}</label>
                    <input
                        id="note-task"
                        list="note-tasks"
                        value={selected}
                        placeholder="SPICE-aBcD"
                        oninput={state_from_input(note_task.clone())}
                    />
                    <datalist id="note-tasks">
                        {for board.tasks.iter().map(|task| {
                            let statement = &task.statement.body;
                            let title = task.presentation_title().to_owned();
                            html! {
                                <option value={statement.handle.to_string()}>
                                    {title}
                                </option>
                            }
                        })}
                    </datalist>
                    <label for="note-body">{"Observation"}</label>
                    <textarea
                        id="note-body"
                        value={note}
                        oninput={state_from_textarea(note_text.clone())}
                    ></textarea>
                    <button
                        type="submit"
                        disabled={note_task.is_empty() || note_text.trim().is_empty()}
                    >{"Add note"}</button>
                    </form>
                </section>
            }
        </div>
    }
}

pub(super) fn render_topology(surface: &SurfaceData) -> Html {
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted topology is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No lane snapshot is available.");
    };
    html! {
        <>
            <section class="section">
                <div class="section-heading">
                    <h2>{"Resident topology"}</h2>
                    <span class="section-count">{format!("{} lanes", lanes.lanes.len())}</span>
                </div>
                if lanes.lanes.is_empty() {
                    {empty("No lanes are seated.")}
                } else {
                    <div class="table-wrap"><table>
                        <thead><tr>
                            <th>{"Lane"}</th><th>{"Standing"}</th><th>{"Occupant"}</th>
                            <th>{"Worktree"}</th><th>{"Vendor"}</th>
                        </tr></thead>
                        <tbody>{for lanes.lanes.iter().flat_map(|team| {
                            team.members.iter().map(move |member| (team, member))
                        }).map(|(team, member)| html! {
                            <tr>
                                <td class="mono">{short(&team.lane.to_string())}</td>
                                <td><span class="tag active">{member.disposition.clone()}</span></td>
                                <td class="mono">{
                                    member
                                        .occupant
                                        .map(|value| short(&value.to_string()))
                                        .unwrap_or_else(|| "vacant".to_owned())
                                }</td>
                                <td class="mono">{member.worktree.clone()}</td>
                                <td>{member.vendor.to_string()}</td>
                            </tr>
                        })}</tbody>
                    </table></div>
                }
            </section>
            {render_causal_thread(runtime)}
        </>
    }
}

pub(super) fn render_chronology(
    surface: &SurfaceData,
    sender: &SenderHandle,
    connection: &Connection,
) -> Html {
    let runtime = surface.as_ref();
    let can_decide = runtime
        .is_some_and(|runtime| runtime.grants() && runtime.offers(CommandKind::ConsentDecide));
    let pending = surface
        .as_ref()
        .and_then(CanvasRuntime::consent)
        .map(|value| value.pending.as_slice())
        .unwrap_or(&[]);
    let directives = surface
        .as_ref()
        .and_then(CanvasRuntime::inbox)
        .map(|inbox| {
            inbox
                .lanes
                .iter()
                .flat_map(|lane| lane.pending.iter())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let events = runtime.map_or(&[][..], CanvasRuntime::events);
    html! {
        <div class="split">
            <div>
                if can_decide {
                    {render_consent_pending(pending, sender)}
                }
                {render_pending_direction(&directives)}
            </div>
            <div>
                {runtime.map_or_else(Html::default, render_trust)}
                {render_command_outcomes(connection, events)}
                {runtime.map_or_else(Html::default, render_fact_chronology)}
            </div>
        </div>
    }
}

/// The consent decisions this operator is currently asked to answer.
fn render_consent_pending(
    pending: &[wire::intent::ConsentPendingAnswer],
    sender: &SenderHandle,
) -> Html {
    html! {
            <section class="section">
                <div class="section-heading">
                    <h2>{"Pending consent"}</h2>
                    <span class="section-count">{pending.len()}</span>
                </div>
                if pending.is_empty() {
                    {empty("No consent decisions are pending.")}
                } else {
                    <div class="table-wrap"><table>
                    <thead><tr>
                        <th>{"Operation"}</th><th>{"Rule"}</th>
                        <th>{"Lane"}</th><th>{"Decision"}</th>
                    </tr></thead>
                    <tbody>{for pending.iter().map(|request| {
                        let allow = ClientAction::Consent {
                            request: request.request,
                            rule: request.rule.clone(),
                            answer: Answer::Granted,
                        };
                        let deny = ClientAction::Consent {
                            request: request.request,
                            rule: request.rule.clone(),
                            answer: Answer::Denied,
                        };
                        let allow_sender = sender.clone();
                        let deny_sender = sender.clone();
                        html! {
                            <tr>
                                <td class="title-cell">{request.operation.clone()}</td>
                                <td class="mono">{request.rule.to_string()}</td>
                                <td class="mono">{short(&request.lane.to_string())}</td>
                                <td><div class="decision-row">
                                    <button
                                        type="button"
                                        onclick={Callback::from(move |_| {
                                            send_action(&allow_sender, allow.clone());
                                        })}
                                    >{"Allow"}</button>
                                    <button
                                        class="deny"
                                        type="button"
                                        onclick={Callback::from(move |_| {
                                            send_action(&deny_sender, deny.clone());
                                        })}
                                    >{"Deny"}</button>
                                </div></td>
                            </tr>
                        }
                    })}</tbody>
                    </table></div>
                }
            </section>
    }
}

/// The steering directives still waiting on a lane.
fn render_pending_direction(directives: &[&wire::intent::PendingDirective]) -> Html {
    html! {
        <section class="section">
            <div class="section-heading">
                <h2>{"Pending direction"}</h2>
                <span class="section-count">{directives.len()}</span>
            </div>
            if directives.is_empty() {
                {empty("No directives are pending.")}
            } else {
                <div class="event-list">{for directives.iter().map(|directive| html! {
                    <article class="event">
                        <strong>
                            {format!("{} / {}", directive.key, directive.expires)}
                        </strong>
                        <p>{directive.text.clone()}</p>
                    </article>
                })}</div>
            }
        </section>
    }
}

/// Command outcomes, led by an explicit offline reason when one exists.
fn render_command_outcomes(connection: &Connection, events: &[crate::RuntimeEvent]) -> Html {
    html! {
        <section class="section">
            <div class="section-heading"><h2>{"Command outcomes"}</h2></div>
            if !matches!(connection, Connection::Offline(_)) && events.is_empty() {
                {empty("No command outcomes have been recorded.")}
            } else {
                <div class="event-list">
                    if let Connection::Offline(reason) = connection {
                        <article class="event fault">
                            <strong>{"Canvas connection reset"}</strong>
                            <p>{reason}</p>
                        </article>
                    }
                    {for events.iter().map(|event| html! {
                        <article class={classes!("event", event.class())}>
                            <strong>{event.title()}</strong>
                            <p>{event.detail()}</p>
                        </article>
                    })}
                </div>
            }
        </section>
    }
}

pub(super) fn render_causal_thread(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.thread_pages();
    let walk = runtime.thread_walk();
    let end = walk.as_ref().map(|walk| match walk.end() {
        ChainEnd::Root => "root".to_owned(),
        ChainEnd::Dangling(position) => format!("dangling at {position}"),
        ChainEnd::Cycle(position) => format!("cycle at {position}"),
    });
    let end_label = end.unwrap_or_else(|| "awaiting thread".to_owned());
    html! {
        <section class="section" data-pane="lineage">
            <div class="section-heading">
                <h2>{"Causal thread"}</h2>
                <span class="section-count">{end_label}</span>
            </div>
            if pages.is_empty() {
                {empty("No causal thread has been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "thread",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="table-wrap"><table>
                                <thead><tr>
                                    <th>{"Seq"}</th>
                                    <th>{"Kind"}</th>
                                    <th>{"Origin"}</th>
                                    <th>{"Cause"}</th>
                                </tr></thead>
                                <tbody>{for page.rows().iter().map(|row| {
                                    let cause = row.cause().map_or_else(
                                        || "root".to_owned(),
                                        |cause| cause.to_string(),
                                    );
                                    html! {
                                        <tr>
                                            <td class="mono">{row.seq().to_string()}</td>
                                            <td>{row.kind().to_string()}</td>
                                            <td class="mono">{row.origin().to_string()}</td>
                                            <td class="mono">{cause}</td>
                                        </tr>
                                    }
                                })}</tbody>
                            </table></div>
                        </div>
                    }
                })}
            }
        </section>
    }
}

pub(super) fn render_trust(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.trust_pages();
    let history = runtime
        .fact_pages()
        .into_iter()
        .flat_map(|page| page.rows().to_vec())
        .filter(|row| {
            matches!(
                row.kind(),
                wire::FactKind::TrustPended
                    | wire::FactKind::TrustGranted
                    | wire::FactKind::TrustRevoked
            )
        })
        .collect::<Vec<_>>();
    if !runtime.grants() {
        return Html::default();
    }
    html! {
        <section class="section" data-pane="trust">
            <div class="section-heading">
                <h2>{"Trust"}</h2>
                <span class="section-count">{format!("{} transitions", history.len())}</span>
            </div>
            if pages.is_empty() {
                {empty("No trust heads have been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "trust",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="table-wrap"><table>
                                <thead><tr>
                                    <th>{"Digest"}</th>
                                    <th>{"Standing"}</th>
                                    <th>{"Position"}</th>
                                </tr></thead>
                                <tbody>{for page.rows().iter().map(|row| {
                                    let standing = format!("{:?}", row.head.state).to_lowercase();
                                    html! {
                                        <tr>
                                            <td class="mono">
                                                {short(&row.head.digest.to_string())}
                                            </td>
                                            <td><span class="tag active">{standing}</span></td>
                                            <td class="mono">{row.position.to_string()}</td>
                                        </tr>
                                    }
                                })}</tbody>
                            </table></div>
                        </div>
                    }
                })}
            }
            if !history.is_empty() {
                <div class="event-list trust-history">{for history.iter().rev().map(|row| html! {
                    <article class="event">
                        <strong>{row.kind().to_string()}</strong>
                        <p>{format!("{} / {} / {}", row.seq(), row.actor(), row.origin())}</p>
                    </article>
                })}</div>
            }
        </section>
    }
}

pub(super) fn render_fact_chronology(runtime: &CanvasRuntime) -> Html {
    let pages = runtime.fact_pages();
    html! {
        <section class="section" data-pane="commands">
            <div class="section-heading">
                <h2>{"Admitted journal"}</h2>
                <span class="section-count">{format!("{} packed pages", pages.len())}</span>
            </div>
            if pages.is_empty() {
                {empty("No journal page has been admitted.")}
            } else {
                {for pages.iter().enumerate().map(|(index, page)| {
                    let provenance = packed_provenance(
                        "facts",
                        index + 1,
                        page.rows().len(),
                        page.overflow(),
                        page.horizon(),
                    );
                    html! {
                        <div class="packed-page">
                            {provenance}
                            <div class="event-list">
                                {for page.rows().iter().rev().map(|row| html! {
                                    <article class="event">
                                        <strong>
                                            {format!("{} / {}", row.seq(), row.kind())}
                                        </strong>
                                        <p>{format!("{} / {}", row.actor(), row.origin())}</p>
                                    </article>
                                })}
                            </div>
                        </div>
                    }
                })}
            }
        </section>
    }
}

pub(super) fn packed_provenance(
    subject: &'static str,
    page: usize,
    rows: usize,
    overflow: Option<wire::Overflow>,
    horizon: Option<wire::Seq>,
) -> Html {
    let horizon = horizon.map_or_else(|| "-".to_owned(), |value| value.to_string());
    html! {
        <div class="packed-provenance" data-subject={subject}>
            <span>{format!("page {page} / {rows} rows")}</span>
            <span>{format!("horizon {horizon}")}</span>
            if let Some(overflow) = overflow {
                <span class="overflow-marker">
                    {format!("{} omitted / continue {}", overflow.omitted, overflow.continuation)}
                </span>
            }
        </div>
    }
}
