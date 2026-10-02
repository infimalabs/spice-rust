use super::*;

fn dash_return_id(slot: &str, lane: CanonicalUuid) -> String {
    format!("team-dash-{slot}-{lane}")
}

fn render_dash_task(standing: &Standing, select: &Callback<wire::TaskStamp>) -> Html {
    let Some(handle) = standing.handle.as_deref() else {
        return html! {
            <div class="team-dash-member-slot" data-dash-slot="task" data-empty="true">
                <span class="team-dash-slot-meta">{"Task"}</span>
                <span class="team-dash-slot-copy">{"No task held"}</span>
            </div>
        };
    };
    let Some(task) = watchband::canonical_task_handle(handle).and_then(|handle| handle.born().ok())
    else {
        return html! {
            <div class="team-dash-member-slot" data-dash-slot="task" data-empty="true">
                <span class="team-dash-slot-meta">{"Task unavailable"}</span>
                <span class="team-dash-slot-copy">{handle.to_owned()}</span>
            </div>
        };
    };
    let open = select.reform(move |_| task.clone());
    let title = standing
        .task_title
        .clone()
        .unwrap_or_else(|| "Current task".to_owned());
    let phase = standing.phase.as_deref().unwrap_or("active");
    html! {
        <button
            type="button"
            class={classes!("team-dash-member-slot", "team-dash-member-action")}
            data-dash-slot="task"
            id={dash_return_id("task", standing.lane)}
            data-task-handle={handle.to_owned()}
            aria-label={format!("Open task {handle}: {title}")}
            onclick={open}
        >
            <span class="team-dash-slot-meta">
                <span>{format!("Task · {phase}")}</span>
                <span class="team-dash-slot-clocks">
                    {lease_age(standing.held_since, "team-dash-slot-age", "age unavailable")}
                    if let Some(expiry) = standing.expires_at {
                        {" · "}
                        <Countdown
                            {expiry}
                            class={classes!("team-dash-slot-countdown")}
                            title={format!("Lease expiry for {handle}")}
                        />
                    }
                </span>
            </span>
            <span class="team-dash-slot-copy">
                <strong>{handle.to_owned()}</strong>
                {" · "}
                {title}
            </span>
        </button>
    }
}

pub(super) fn dash_chat_click(
    team: CanonicalUuid,
    controls: &UseReducerHandle<TeamControls>,
) -> Callback<MouseEvent> {
    let controls = controls.clone();
    Callback::from(move |event: MouseEvent| {
        if super::card_target::remember_team_return(&event, team) {
            controls.dispatch(TeamControlAction::ShowChat { team });
        }
    })
}

fn render_dash_activity(
    runtime: &CanvasRuntime,
    standing: &Standing,
    team: CanonicalUuid,
    controls: &UseReducerHandle<TeamControls>,
) -> Html {
    let Some(activity) = super::agent_menu::latest_activity(runtime, standing) else {
        return html! {
            <div class="team-dash-member-slot" data-dash-slot="activity" data-empty="true">
                <span class="team-dash-slot-meta">{"Latest activity"}</span>
                <span class="team-dash-slot-copy">{"No retained activity"}</span>
            </div>
        };
    };
    html! {
        <a
            class={classes!("team-dash-member-slot", "team-dash-member-action")}
            data-dash-slot="activity"
            id={dash_return_id("activity", standing.lane)}
            data-dash-return=""
            data-dash-return-origin=""
            data-activity-class={activity.class}
            href={crate::view::card_href(&activity.target)}
            aria-label={format!("Open latest {} activity in Chat", standing.label)}
            onclick={dash_chat_click(team, controls)}
        >
            <span class="team-dash-slot-meta">
                <span>{"Latest activity"}</span>
                <Age authored={activity.authored} class={classes!("team-dash-slot-age")} />
            </span>
            <span class="team-dash-slot-copy">
                <strong>{activity.prefix}</strong>
                if !activity.detail.is_empty() {
                    {" "}
                    {activity.detail}
                }
            </span>
        </a>
    }
}

fn render_dash_direction(
    standing: &Standing,
    team: CanonicalUuid,
    controls: &UseReducerHandle<TeamControls>,
) -> Html {
    let Some(preview) = super::agent_menu::waiting_direction(standing) else {
        return html! {
            <div class="team-dash-member-slot" data-dash-slot="direction" data-empty="true">
                <span class="team-dash-slot-meta">{"Waiting direction"}</span>
                <span class="team-dash-slot-copy">{"No pending direction"}</span>
            </div>
        };
    };
    let target = primary_textarea_id(&standing.lane.to_string());
    html! {
        <a
            class={classes!("team-dash-member-slot", "team-dash-member-action")}
            data-dash-slot="direction"
            id={dash_return_id("direction", standing.lane)}
            data-dash-return=""
            data-dash-return-origin=""
            data-pending={standing.pending.to_string()}
            href={crate::view::card_href(&target)}
            aria-label={format!("Open {}'s composer in Chat", standing.label)}
            onclick={dash_chat_click(team, controls)}
        >
            <span class="team-dash-slot-meta">
                <span>{"Waiting direction"}</span>
                <span>{format!("{} pending", standing.pending)}</span>
            </span>
            <span class="team-dash-slot-copy">{preview}</span>
        </a>
    }
}

pub(super) fn render_agent_details(
    runtime: &CanvasRuntime,
    member: &crate::runtime::LaneMember,
    standing: &Standing,
    team: CanonicalUuid,
    select: &Callback<wire::TaskStamp>,
    controls: &UseReducerHandle<TeamControls>,
    member_command: &Callback<(CanonicalUuid, crate::runtime::MemberCommand)>,
) -> Html {
    debug_assert_eq!(
        member.lane, standing.lane,
        "standings retain canonical member order"
    );
    let occupant = member
        .occupant
        .map(|value| value.to_string())
        .unwrap_or_else(|| "vacant".to_owned());
    let session = standing
        .session
        .clone()
        .unwrap_or_else(|| "unbound".to_owned());
    let status = member.status(standing.pending);
    let presence = watchband::member_presence(runtime, member, standing);
    let presence_label = if presence.state == watchband::MemberPresenceState::Working {
        standing.handle.as_ref().map_or_else(
            || presence.state.label().to_owned(),
            |handle| format!("working on {handle}"),
        )
    } else {
        presence.state.label().to_owned()
    };
    let presence_age = lease_age(
        presence.since_millis,
        "team-presence-age",
        "age unavailable",
    );
    html! {
        <article
            key={standing.lane.to_string()}
            class="team-dash-member"
            data-agent-lane={standing.lane.to_string()}
            data-lane-status={status.key()}
            data-accent={standing.accent.to_string()}
            data-agent-presence={presence.state.key()}
            title={watchband::standing_title(standing)}
        >
            <header class="team-dash-member-header">
                {view::render_member_presence(status)}
                <strong>{standing.label.clone()}</strong>
                {render_dash_vendor_icon(standing)}
            </header>
            <details class="team-dash-member-disclosure">
                <summary title="Seat details">
                    <span class="team-presence-state">{presence_label}</span>
                    {" · "}
                    {presence_age}
                </summary>
                <div class="team-dash-member-seat">
                {super::member_controls::render(runtime, member, standing, member_command, "dash")}
                <dl>
                    <div>
                        <dt>{"Agent channel"}</dt>
                        <dd><code>{standing.lane.to_string()}</code></dd>
                    </div>
                    <div>
                        <dt>{"Occupant"}</dt>
                        <dd><code>{occupant}</code></dd>
                    </div>
                    <div>
                        <dt>{"Worktree"}</dt>
                        <dd><code>{standing.worktree.clone()}</code></dd>
                    </div>
                    <div>
                        <dt>{"Pending"}</dt>
                        <dd>{format!("{} pending", standing.pending)}</dd>
                    </div>
                    <div>
                        <dt>{"Model / residency policy"}</dt>
                        <dd>{format!(
                            "{} / {}",
                            watchband::model_display(standing.model.as_deref()),
                            standing.life
                        )}</dd>
                    </div>
                    <div>
                        <dt>{"Session"}</dt>
                        <dd><code>{session}</code></dd>
                    </div>
                    {for standing.jobs.iter().map(|job| html! {
                        <div>
                            <dt>
                                {"Job "}
                                <code>{job.id.clone()}</code>
                                {format!(" ({})", job.standing.as_str())}
                            </dt>
                            <dd>{job.command.clone()}</dd>
                        </div>
                    })}
                </dl>
                </div>
            </details>
            {render_dash_task(standing, select)}
            {render_dash_activity(runtime, standing, team, controls)}
            {render_dash_direction(standing, team, controls)}
        </article>
    }
}
