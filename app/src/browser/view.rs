//! Canvas surface rendering for the operator browser app.

use super::*;

pub(super) fn render_canvas(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    axis: &UseStateHandle<Surface>,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
    refresh: Callback<MouseEvent>,
    speech: &UseStateHandle<wire::SpeechPlayback>,
    speech_status: &UseStateHandle<SpeechStatus>,
    toggle_speech: Callback<MouseEvent>,
) -> Html {
    let active_axis = **axis;
    let cursor = (**surface)
        .as_ref()
        .and_then(CanvasRuntime::resume)
        .map(|cursor| cursor.seq.to_string())
        .unwrap_or_else(|| "-".to_owned());
    html! {
        <div class="shell">
            {render_rail(axis, active_axis, &cursor)}
            {render_workspace(
                connection,
                surface,
                active_axis,
                sender,
                note_task,
                note_text,
                submit_note,
                refresh,
                speech,
                speech_status,
                toggle_speech,
            )}
        </div>
    }
}

pub(super) fn render_rail(
    axis: &UseStateHandle<Surface>,
    active_axis: Surface,
    cursor: &str,
) -> Html {
    html! {
        <aside class="rail">
            <div class="brand">
                <span class="brand-mark">{"SP"}</span>
                <span class="brand-name">{"Spice Canvas"}</span>
            </div>
            <nav class="axis-nav" aria-label="Canvas axes">
                {for Surface::all().map(|candidate| {
                    let axis = axis.clone();
                    let selected = candidate == active_axis;
                    let mosaic = matches!(candidate, Surface::Mosaic);
                    html! {
                        <button
                            class={classes!(
                                "axis-button",
                                mosaic.then_some("axis-button-mosaic"),
                            )}
                            aria-current={selected.then_some("page")}
                            title={candidate.question()}
                            onclick={Callback::from(move |_| axis.set(candidate))}
                        >
                            <span class="axis-glyph"></span>
                            <span class="axis-label">{title(candidate.as_str())}</span>
                        </button>
                    }
                })}
            </nav>
            <div class="rail-meta">{format!("cursor {cursor}")}</div>
        </aside>
    }
}

pub(super) fn render_workspace(
    connection: &UseStateHandle<Connection>,
    surface: &UseStateHandle<SurfaceData>,
    active_axis: Surface,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
    refresh: Callback<MouseEvent>,
    speech: &wire::SpeechPlayback,
    speech_status: &SpeechStatus,
    toggle_speech: Callback<MouseEvent>,
) -> Html {
    let runtime = (**surface).as_ref();
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
        <div class="workspace">
            <header class="topbar">
                <div class="mobile-brand">{"Spice Canvas"}</div>
                <div class={connection.class()}>
                    <span class="status-dot"></span>
                    <span class="status-copy">{connection.label()}</span>
                </div>
                <div class="topbar-actions">
                    {speech_control(speech, speech_status, toggle_speech)}
                    <button class="refresh" type="button" onclick={refresh}>{"Refresh"}</button>
                </div>
            </header>
            <main class="content">
                <div class="page-heading">
                    <div>
                        <h1>{title(active_axis.as_str())}</h1>
                        <p>{active_axis.question()}</p>
                    </div>
                </div>
                <section class="metrics" aria-label="Current resident metrics">
                    {metric("Tasks", task_count.to_string(), format!("{active_tasks} active"))}
                    {metric("Lanes", lane_count.to_string(), resident_state(surface))}
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
                    surface,
                    connection,
                    sender,
                    note_task,
                    note_text,
                    submit_note,
                )}
            </main>
        </div>
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
    axis: Surface,
    surface: &SurfaceData,
    connection: &Connection,
    sender: &SenderHandle,
    note_task: &UseStateHandle<String>,
    note_text: &UseStateHandle<String>,
    submit_note: Callback<SubmitEvent>,
) -> Html {
    match axis {
        Surface::View(Axis::Magnitude) => render_magnitude(surface),
        Surface::View(Axis::Flow) => render_flow(surface, note_task, note_text, submit_note),
        Surface::View(Axis::Topology) => render_topology(surface),
        Surface::View(Axis::Chronology) => render_chronology(surface, sender, connection),
        Surface::Mosaic => super::mosaic::render_mosaic(surface, connection),
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
                        <tbody>{for lanes.lanes.iter().map(|lane| html! {
                            <tr>
                                <td class="mono">{short(&lane.lane.to_string())}</td>
                                <td><span class="tag active">{lane.disposition.clone()}</span></td>
                                <td class="mono">{
                                    lane
                                        .occupant
                                        .map(|value| short(&value.to_string()))
                                        .unwrap_or_else(|| "vacant".to_owned())
                                }</td>
                                <td class="mono">{lane.worktree.clone()}</td>
                                <td>{lane.vendor.to_string()}</td>
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
