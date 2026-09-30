//! The Dash attention band: what this team needs from the operator now.
//!
//! The band lives in the flow header, whose height is fixed, so items enter
//! and leave without moving the flow below it. With nothing due it renders no
//! items and the header shows only the flow heading.

use gloo_timers::callback::Timeout;

use super::super::clock::now_millis;
use super::*;
use crate::runtime::ConsentRuling;
use crate::watchband::{AttentionItem, AttentionSubject, TaskAttention, attention_at};

#[derive(Properties, PartialEq)]
pub(super) struct AttentionBandProps {
    /// Every candidate item, including ones not yet due.
    pub(super) items: Rc<Vec<AttentionItem>>,
    /// Render one due item with its control.
    pub(super) view: Callback<AttentionItem, Html>,
}

/// The due items, re-rendered when the next time-dependent item falls due.
#[function_component(AttentionBand)]
pub(super) fn attention_band(props: &AttentionBandProps) -> Html {
    let tick = use_state(|| 0_u64);
    let now = now_millis();
    let (due, next) = attention_at(&props.items, now);
    {
        let tick = tick.clone();
        use_effect_with((next, *tick), move |(next, generation)| {
            let generation = *generation;
            let timeout = next.map(|next| {
                // One frame past the deadline, so the re-render finds it due.
                let delay = next.saturating_sub(now_millis()).saturating_add(16);
                Timeout::new(u32::try_from(delay).unwrap_or(u32::MAX), move || {
                    tick.set(generation.wrapping_add(1));
                })
            });
            move || drop(timeout)
        });
    }
    if due.is_empty() {
        return Html::default();
    }
    html! {
        <ul class="team-dash-attention" aria-label="Needs you">
            {for due.into_iter().map(|item| props.view.emit(item.clone()))}
        </ul>
    }
}

/// What one team's band needs to render its items' controls.
pub(super) struct AttentionControls<'a> {
    pub(super) team: CanonicalUuid,
    pub(super) runtime: &'a CanvasRuntime,
    pub(super) sender: &'a SenderHandle,
    pub(super) select: &'a Callback<wire::TaskStamp>,
    pub(super) controls: &'a UseReducerHandle<TeamControls>,
}

/// This team's candidate items, dropping consent requests already ruled.
pub(super) fn team_items(
    runtime: &CanvasRuntime,
    standings: &[Standing],
    work: Option<&crate::work::WorkProjection<'_>>,
) -> Rc<Vec<AttentionItem>> {
    let mut items =
        watchband::team_attention(standings, work, runtime.inbox(), runtime.consent_queue());
    items.retain(|item| match &item.subject {
        AttentionSubject::Consent(pending) => !matches!(
            runtime.consent_ruling(pending.request),
            Some(ConsentRuling::Ruled(_))
        ),
        _ => true,
    });
    Rc::new(items)
}

/// One item renderer that owns every handle its controls need.
pub(super) fn item_view(controls: &AttentionControls<'_>) -> Callback<AttentionItem, Html> {
    let team = controls.team;
    let decide = controls.runtime.offers(CommandKind::ConsentDecide);
    let rulings = controls
        .runtime
        .consent_queue()
        .into_iter()
        .flat_map(|queue| &queue.pending)
        .filter_map(|pending| {
            controls
                .runtime
                .consent_ruling(pending.request)
                .map(|ruling| (pending.request, ruling.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let sender = controls.sender.clone();
    let select = controls.select.clone();
    let team_controls = controls.controls.clone();
    Callback::from(move |item: AttentionItem| {
        let body = match &item.subject {
            AttentionSubject::Consent(pending) => render_consent(
                &item,
                pending,
                decide.then_some(&sender),
                rulings.get(&pending.request),
            ),
            AttentionSubject::Task {
                task,
                handle,
                title,
                reason,
            } => render_task(&item, task, handle, title, *reason, &select),
            AttentionSubject::Lease {
                task,
                handle,
                expires_at,
            } => render_lease(&item, task.as_ref(), handle, *expires_at, &select),
            AttentionSubject::Direction {
                pending,
                oldest,
                preview,
            } => render_direction(&item, team, *pending, *oldest, preview, &team_controls),
        };
        html! {
            <li
                key={item.key()}
                class="team-dash-attention-item"
                data-attention={attention_kind(&item.subject)}
                data-agent-lane={item.lane.to_string()}
                data-accent={item.accent.to_string()}
            >{body}</li>
        }
    })
}

const fn attention_kind(subject: &AttentionSubject) -> &'static str {
    match subject {
        AttentionSubject::Consent(_) => "consent",
        AttentionSubject::Task { .. } => "task",
        AttentionSubject::Lease { .. } => "lease",
        AttentionSubject::Direction { .. } => "direction",
    }
}

fn render_consent(
    item: &AttentionItem,
    pending: &wire::intent::ConsentPendingAnswer,
    sender: Option<&SenderHandle>,
    ruling: Option<&ConsentRuling>,
) -> Html {
    let sending = matches!(ruling, Some(ConsentRuling::Sending(_)));
    // The ruling's progress and outcome report through the root toast.
    let rule = |answer: wire::Answer, label: &'static str| {
        let sender = sender.cloned();
        let request = pending.request;
        let onclick = Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            if let Some(sender) = &sender {
                let _admitted =
                    admit_action(sender, ClientAction::ConsentRuling { request, answer });
            }
        });
        html! {
            <button
                type="button"
                class="team-dash-attention-rule"
                data-answer={label.to_lowercase()}
                disabled={sending}
                {onclick}
            >{label}</button>
        }
    };
    html! {
        <span
            class="team-dash-attention-chip"
            title={format!(
                "{} asks for {} under rule {}",
                item.label, pending.operation, pending.rule.as_str()
            )}
        >
            <span class="team-dash-flow-member">{item.label.clone()}</span>
            <span class="team-dash-attention-copy">
                <strong>{"Consent"}</strong>
                {" · "}
                {pending.operation.clone()}
            </span>
            if sender.is_some() {
                {rule(wire::Answer::Granted, "Allow")}
                {rule(wire::Answer::Denied, "Deny")}
            }
        </span>
    }
}

fn render_task(
    item: &AttentionItem,
    task: &wire::TaskStamp,
    handle: &str,
    title: &str,
    reason: TaskAttention,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let label = reason.label();
    html! {
        <button
            type="button"
            class="team-dash-attention-chip"
            data-task-attention={reason.key()}
            aria-label={format!("Open task {handle}: {title}")}
            title={format!("{label}: {handle} · {title}")}
            onclick={open_task(task, select)}
        >
            <span class="team-dash-flow-member">{item.label.clone()}</span>
            <span class="team-dash-attention-copy">
                <strong>{label}</strong>
                {" · "}
                {handle.to_owned()}
            </span>
        </button>
    }
}

fn render_lease(
    item: &AttentionItem,
    task: Option<&wire::TaskStamp>,
    handle: &str,
    expiry: i64,
    select: &Callback<wire::TaskStamp>,
) -> Html {
    let copy = html! {
        <>
            <span class="team-dash-flow-member">{item.label.clone()}</span>
            <span class="team-dash-attention-copy">
                <strong>{"Lease"}</strong>
                {" · "}
                {handle.to_owned()}
            </span>
            <Countdown
                {expiry}
                class={classes!("team-dash-flow-age")}
                title={format!("Lease expiry for {handle}")}
            />
        </>
    };
    match task {
        Some(task) => html! {
            <button
                type="button"
                class="team-dash-attention-chip"
                aria-label={format!("Open task {handle}, whose lease is expiring")}
                onclick={open_task(task, select)}
            >{copy}</button>
        },
        None => html! { <span class="team-dash-attention-chip">{copy}</span> },
    }
}

fn open_task(task: &wire::TaskStamp, select: &Callback<wire::TaskStamp>) -> Callback<MouseEvent> {
    let task = task.clone();
    select.reform(move |_| task.clone())
}

fn render_direction(
    item: &AttentionItem,
    team: CanonicalUuid,
    pending: usize,
    oldest: i64,
    preview: &str,
    controls: &UseReducerHandle<TeamControls>,
) -> Html {
    let target = primary_textarea_id(&item.lane.to_string());
    let waiting = if pending == 1 {
        "1 waiting".to_owned()
    } else {
        format!("{pending} waiting")
    };
    html! {
        <a
            class="team-dash-attention-chip"
            href={crate::view::card_href(&target)}
            aria-label={format!("Open {}'s composer in Chat", item.label)}
            title={preview.to_owned()}
            onclick={dash::dash_chat_click(team, controls)}
        >
            <span class="team-dash-flow-member">{item.label.clone()}</span>
            <span class="team-dash-attention-copy">
                <strong>{"Direction"}</strong>
                {" · "}
                {waiting}
            </span>
            <Age authored={oldest} class={classes!("team-dash-flow-age")} />
        </a>
    }
}
