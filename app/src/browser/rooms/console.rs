//! Working-group consoles: a team watchband with tasks and agents swapped.
//!
//! A group's designs are its composers and its accented channels; the lanes
//! working any task associated with those designs are its tabs. Every other
//! piece is the team console's own: the packed plane, its composer slot, the
//! quote shards, and the composer issue toast.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::HtmlTextAreaElement;
use wire::rooms::{RoomAuthorityStamp, RoomPublicationContent};
use wire::{QuoteTarget, TaskStamp};
use yew::prelude::*;

use super::{
    ComposerFocus, Controls, anchor_label, oops_grooming_form, render_event, render_preparation,
    room_post_notice,
};
use crate::AppRuntime;
use crate::browser::clock::Age;
use crate::browser::sender::{SenderHandle, admit_action};
use crate::browser::watchband::{
    ActivityCard, ComposerAction, ComposerDrafts, ComposerTextarea, HistorySender, MosaicPlane,
    PlaneOwner, ROOM_CARD_PREFIX, TeamSpeech, clear_composer, focus_after_render, prepare_composer,
    primary_textarea_id, quote_textarea_id, render_quote_shards,
};
use crate::client_action::{ClientAction, RoomPostAction};
use crate::room_views::{GroupView, RoomViewAction};
use crate::runtime::{RoomAnchorAuthority, RoomEvent, RoomEventKind, RoomProjectionStatus};
use crate::watchband;
use crate::work::{GroupLane, WorkProjection};

/// Everything one render of the opened group consoles reads.
pub(in crate::browser) struct GroupRender<'a, 'w> {
    pub(in crate::browser) runtime: &'a AppRuntime,
    pub(in crate::browser) work: Option<&'a WorkProjection<'w>>,
    pub(in crate::browser) controls: &'a Controls,
    pub(in crate::browser) sender: &'a SenderHandle,
    pub(in crate::browser) drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub(in crate::browser) open_message_task: &'a Callback<(TaskStamp, web_sys::Element)>,
    pub(in crate::browser) select_task: &'a Callback<TaskStamp>,
}

/// One design as its group shows it: a composer target and an accented channel.
struct Channel {
    stamp: TaskStamp,
    accent: usize,
    label: String,
}

/// Every opened group console, in this browser's order, as fleet surfaces.
pub(in crate::browser) fn render_group_surfaces(render: &GroupRender<'_, '_>) -> Html {
    let groups = render.controls.views.groups();
    render
        .controls
        .slots
        .retain(|group| groups.iter().any(|view| view.group == *group));
    groups
        .iter()
        .enumerate()
        .map(|(index, view)| render_surface(render, view, index))
        .collect()
}

fn render_surface(render: &GroupRender<'_, '_>, view: &GroupView, index: usize) -> Html {
    let projection = render.runtime.room_projection();
    let loaded = projection.status() == RoomProjectionStatus::Available;
    let authoritative = projection.groups().get(&view.group);
    // Absence is shown only once a completed snapshot lacks the group; while
    // authority reloads, the console keeps its last name and its geometry.
    let absent = loaded && authoritative.is_none();
    let name = group_name(
        render.runtime,
        render.controls,
        view,
        authoritative.map(|group| &group.value),
        absent,
    );
    let anchors = authoritative.map_or(&[][..], |group| group.value.anchors.as_slice());
    let channels = anchors
        .iter()
        .enumerate()
        .map(|(position, stamp)| Channel {
            stamp: stamp.clone(),
            accent: watchband::accent(anchors.len(), position),
            label: anchor_label(render.runtime, stamp),
        })
        .collect::<Vec<_>>();
    let slot = render.controls.slots.slot(view.group.clone());
    let cards = plane_cards(render, &view.group, &channels);
    let empty = cards.is_empty();
    let composer = html! {
        <form class="team-composer" onsubmit={Callback::from(|event: SubmitEvent| event.prevent_default())}>
            <div class="team-agents">
                {for channels.iter().map(|channel| render_design_composer(render, view, channel))}
            </div>
        </form>
    };
    html! {
        <section
            key={view.group.to_string()}
            class="team-surface group-surface"
            data-group={view.group.to_string()}
            data-team-weight={watchband::team_visual_weight(channels.len()).to_string()}
        >
            <header class="team-toolbar">
                {render_lane_rail(render, &channels)}
                {render_status(render.controls, view, index, &name, &channels, loaded)}
            </header>
            if absent {
                <p class="room-unavailable">{"Shared group authority is unavailable. This local view remains in place."}</p>
            }
            <div class="team-pane">
                <div class="team-compose">
                    // The composer renders into its slot in the plane's scroll content.
                    {yew::create_portal(composer, slot.clone().into())}
                    <div
                        tabindex="-1"
                        class="team-activity-grid"
                        aria-live="polite"
                        aria-label="Live design activity"
                        data-empty={empty.to_string()}
                    >
                        if empty {
                            <div class="mosaic-message-empty">{"No room publications."}</div>
                        }
                        <MosaicPlane
                            key={view.group.to_string()}
                            owner={PlaneOwner::Group(view.group.clone())}
                            members={channels.len().max(1)}
                            {cards}
                            quote_dispatcher={render.drafts.dispatcher()}
                            open_message_task={render.open_message_task.clone()}
                            speak={Callback::noop()}
                            narrate={Callback::noop()}
                            speech={TeamSpeech::Quiet}
                            speech_owner={None}
                            history_targets={BTreeMap::new()}
                            history_revision={0}
                            history={HistorySender::new(render.sender.clone())}
                            composer_slot={slot}
                        />
                    </div>
                </div>
            </div>
        </section>
    }
}

fn group_name(
    runtime: &AppRuntime,
    controls: &Controls,
    view: &GroupView,
    authoritative: Option<&wire::rooms::WorkingGroup>,
    absent: bool,
) -> String {
    match authoritative {
        Some(group) => {
            let label = super::group_label(runtime, group);
            controls
                .names
                .borrow_mut()
                .insert(view.group.clone(), label.clone());
            label
        }
        None if absent => "Unavailable working group".to_owned(),
        None => controls
            .names
            .borrow()
            .get(&view.group)
            .cloned()
            .unwrap_or_else(|| "Loading working group…".to_owned()),
    }
}

/// The group's status band: one accented light per design channel, the
/// group's name, and this browser's own placement of the console.
fn render_status(
    controls: &Controls,
    view: &GroupView,
    index: usize,
    name: &str,
    channels: &[Channel],
    loaded: bool,
) -> Html {
    let close = {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |_| controls.close_group(group.clone()))
    };
    let move_group = |delta| {
        let controls = controls.clone();
        let group = view.group.clone();
        Callback::from(move |_| controls.move_group(group.clone(), delta))
    };
    html! {
        <div class="app-glass-control-surface app-status-band team-status-row">
            <span class="team-lights" role="group" aria-label={format!("{} designs", channels.len())}>
                {for channels.iter().map(|channel| html! {
                    <span
                        key={channel.stamp.to_string()}
                        class="app-presence-pip app-presence-pip--status group-design-pip"
                        role="img"
                        data-design={channel.stamp.to_string()}
                        data-accent={channel.accent.to_string()}
                        aria-label={channel.label.clone()}
                        title={channel.label.clone()}
                    ></span>
                })}
            </span>
            <strong class="group-surface-name" aria-busy={(!loaded).to_string()}>{name.to_owned()}</strong>
            <span class="room-inline-actions">
                <button type="button" disabled={index == 0} onclick={move_group(-1)} aria-label="Move group left">{"←"}</button>
                <button type="button" disabled={index + 1 == controls.views.groups().len()} onclick={move_group(1)} aria-label="Move group right">{"→"}</button>
                <button type="button" onclick={close} aria-label="Close group">{"×"}</button>
            </span>
        </div>
    }
}

/// The lanes working any task associated with the group's designs, each
/// followed by the handles of the tasks it is working there.
fn render_lane_rail(render: &GroupRender<'_, '_>, channels: &[Channel]) -> Html {
    let designs = channels
        .iter()
        .map(|channel| channel.stamp.clone())
        .collect::<Vec<_>>();
    let lanes = render
        .work
        .map(|work| work.group_lanes(&designs))
        .unwrap_or_default();
    html! {
        <div class="team-task-rail">
            if !lanes.is_empty() {
                <nav class="app-horn-tabs team-tasks" aria-label="Lanes working this group's designs">
                    {for lanes.iter().map(|lane| render_lane_tab(render, lane, channels))}
                </nav>
            }
        </div>
    }
}

/// One lane tab: the lane opens its own Chat composer, where its history
/// plays, and stays inert while it holds no Chat seat.
fn render_lane_tab(
    render: &GroupRender<'_, '_>,
    lane: &GroupLane<'_, '_>,
    channels: &[Channel],
) -> Html {
    let lanes = render.runtime.lanes();
    let member = lanes.and_then(|lanes| {
        lanes
            .lanes
            .iter()
            .flat_map(|team| &team.members)
            .find(|member| member.lane == lane.lane())
    });
    let label = member.map_or_else(
        || lane.lane().to_string(),
        |member| {
            watchband::agent_message_label(&member.worktree, member.name.as_deref()).to_owned()
        },
    );
    let chat = lanes.and_then(|lanes| lanes.chat_target_for_lane(lane.lane()));
    let target = match chat {
        Some((_team, member)) => html! {
            <a
                class="app-horn-tab__target"
                href={crate::view::card_href(&primary_textarea_id(&member.to_string()))}
                title={format!("Open {label} in Chat")}
                aria-label={format!("Open {label} in Chat")}
                data-group-lane-link={lane.lane().to_string()}
            >
                <span class="app-horn-tab__label">{label.clone()}</span>
            </a>
        },
        None => html! {
            <span class="app-horn-tab__target" title={label.clone()}>
                <span class="app-horn-tab__label">{label.clone()}</span>
            </span>
        },
    };
    html! {
        <span
            key={lane.lane().to_string()}
            class="app-horn-tab app-horn-tab--up app-horn-tab--group"
            data-group-lane={lane.lane().to_string()}
        >
            {target}
            {for lane.tasks().iter().map(|task| render_lane_task(render, task, channels))}
        </span>
    }
}

/// One task handle under its lane, in the accent of the design it serves.
fn render_lane_task(
    render: &GroupRender<'_, '_>,
    task: &crate::work::WorkTask<'_>,
    channels: &[Channel],
) -> Html {
    let statement = &task.row().statement.body;
    let accent = channels
        .iter()
        .find(|channel| crate::runtime::associated(statement, &channel.stamp))
        .map(|channel| channel.accent);
    let label = format!(
        "Open {}: {}",
        statement.handle,
        crate::composer_markdown::inline::plain_text(task.row().presentation_title()),
    );
    let open_task = {
        let select = render.select_task.clone();
        let born = statement.born.clone();
        Callback::from(move |_| select.emit(born.clone()))
    };
    html! {
        <button
            key={statement.born.to_string()}
            class="app-horn-tab app-horn-tab--up app-horn-tab__handle"
            type="button"
            title={label.clone()}
            aria-label={label}
            data-availability={task.availability().key()}
            data-accent={accent.map(|accent| accent.to_string())}
            onclick={open_task}
        >{statement.handle.to_string()}</button>
    }
}

/// The composer and quote-draft key of one design within one group.
fn design_key(group: &RoomAuthorityStamp, design: &TaskStamp) -> String {
    format!("design-{group}-{design}")
}

/// The shared composer toast, where every console surface reports its
/// outcomes and refusals.
fn toast(render: &GroupRender<'_, '_>) -> Callback<String> {
    let drafts = render.drafts.clone();
    Callback::from(move |issue| drafts.dispatch(ComposerAction::Report { issue }))
}

/// Every room publication of every design, interleaved newest first exactly
/// as a team plane interleaves its lanes.
fn plane_cards(
    render: &GroupRender<'_, '_>,
    group: &RoomAuthorityStamp,
    channels: &[Channel],
) -> Vec<ActivityCard> {
    let projection = render.runtime.room_projection();
    let mut cards = channels
        .iter()
        .flat_map(|channel| {
            projection
                .room(&channel.stamp)
                .into_iter()
                .flat_map(|room| room.events())
                .map(move |(publication, event)| {
                    room_card(render, group, channel, publication, event)
                })
        })
        .collect::<Vec<_>>();
    cards.sort_by_key(|card| Reverse(card.chronology_key()));
    cards
}

fn room_card(
    render: &GroupRender<'_, '_>,
    group: &RoomAuthorityStamp,
    channel: &Channel,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
) -> ActivityCard {
    let source_id = format!("{ROOM_CARD_PREFIX}{publication}");
    let display_millis = publication.micros() / 1_000;
    // Only authored words quote; the plane has no text for a lane's packet
    // until its transcript is read, and other events carry no words at all.
    let quote = match &event.kind {
        RoomEventKind::Published(RoomPublicationContent::Authored { text }) => {
            let target = QuoteTarget::Room {
                room: channel.stamp.clone(),
                publication: publication.clone(),
                digest: QuoteTarget::text_digest(text),
            };
            let lane = design_key(group, &channel.stamp);
            let identity = target.to_string();
            let dispatcher = render.drafts.dispatcher();
            let source_id = source_id.clone();
            let order = event.position.get();
            let preview = watchband::quote_label(watchband::first_nonquoted_prose(text).as_deref())
                .to_owned();
            Some(Callback::from(move |_: MouseEvent| {
                dispatcher.dispatch(ComposerAction::AddQuote {
                    lane: lane.clone(),
                    target: target.clone(),
                    source_id: source_id.clone(),
                    order,
                    display_millis,
                    preview: preview.clone(),
                });
                focus_after_render(quote_textarea_id(&lane, &identity));
            }))
        }
        _ => None,
    };
    let body = html! {
        <>
            <div class="message-controls" role="group" aria-label={format!("Publication controls for {}", channel.label)}>
                <div class="message-controls-meta">
                    <Age
                        authored={display_millis}
                        class="relative-age"
                        title={publication.to_string()}
                        href={Some(AttrValue::from(crate::view::card_href(&source_id)))}
                        label={format!("Link to publication in {}", channel.label)}
                    />
                </div>
                <div class="message-controls-actions">
                    if let Some(quote) = quote {
                        <span class="message-actions app-control-rail">
                            <button
                                class="app-card-action app-card-action--quote"
                                type="button"
                                title="Quote this message"
                                aria-label="Quote this message"
                                onclick={quote}
                            ><span class="app-card-action__icon" aria-hidden="true">{"❝"}</span></button>
                        </span>
                    }
                </div>
            </div>
            {render_event(render.runtime, &toast(render), render.sender, publication, event)}
        </>
    };
    ActivityCard::Room {
        accent: channel.accent,
        design: channel.stamp.clone(),
        publication: publication.clone(),
        position: event.position,
        shape: Rc::from(format!("{:?}", event.kind)),
        body,
    }
}

/// One design's composer: its heading and menu, the textarea posting to
/// exactly that design's room, and the quote shards drafted beneath it.
fn render_design_composer(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    channel: &Channel,
) -> Html {
    let key = design_key(&view.group, &channel.stamp);
    let draft = render.drafts.get(&key).cloned().unwrap_or_default();
    let drawn = render.controls.drawn(view, &channel.stamp);
    let post = post_callback(render, &view.group, channel, &key);
    let onkeydown = {
        let post = post.clone();
        Callback::from(move |event: KeyboardEvent| {
            if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
                event.prevent_default();
                if !event.repeat() {
                    post.emit(());
                }
            }
        })
    };
    let oninput = {
        let controls = render.controls.clone();
        let group = view.group.clone();
        let design = channel.stamp.clone();
        Callback::from(move |event: InputEvent| {
            let Some(textarea) = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlTextAreaElement>().ok())
                .filter(|textarea| textarea.has_attribute("data-primary-composer-target-id"))
            else {
                return;
            };
            controls.draft(group.clone(), design.clone(), textarea.value());
        })
    };
    let focused = view.focused(&channel.stamp);
    let onfocusin = {
        let controls = render.controls.clone();
        let group = view.group.clone();
        let design = channel.stamp.clone();
        Callback::from(move |_: FocusEvent| {
            if !focused {
                controls.focus(group.clone(), design.clone());
            }
        })
    };
    let requested = render
        .controls
        .requested
        .borrow()
        .as_ref()
        .is_some_and(|(group, design)| *group == view.group && *design == channel.stamp);
    let settle = {
        let requested = render.controls.requested.clone();
        Callback::from(move |()| *requested.borrow_mut() = None)
    };
    let placeholder = format!("Post to {}", channel.label);
    let draft_unsafe = !drawn.is_empty() || !draft.quotes.is_empty();
    let textarea_id = primary_textarea_id(&key);
    html! {
        <section
            key={channel.stamp.to_string()}
            class="team-agent team-design"
            data-design={channel.stamp.to_string()}
            data-accent={channel.accent.to_string()}
            title={channel.label.clone()}
            {oninput}
            {onfocusin}
        >
            <div
                class="agent-composer-stack"
                data-has-quote-shards={(!draft.quotes.is_empty()).to_string()}
            >
                <div
                    class="mosaic-composer composer-shard composer-shard--primary"
                    data-shard-kind="primary"
                    data-shard-target-id={key.clone()}
                    data-draft-unsafe={draft_unsafe.to_string()}
                >
                    <header class="app-glass-control-surface composer-shard-heading">
                        <strong class="composer-shard-prose" title={channel.label.clone()}>{channel.label.clone()}</strong>
                        {render_design_menu(render, view, channel)}
                        <button
                            type="button"
                            class="composer-shard-post"
                            aria-label={format!("Post to {}", channel.label)}
                            onclick={Callback::from(move |_| post.emit(()))}
                        >{"Post"}</button>
                    </header>
                    <ComposerTextarea
                        key={drawn.clone()}
                        id={textarea_id.clone()}
                        lane={key.clone()}
                        drafts={render.drafts.dispatcher()}
                        label={format!("Message for design {}", channel.label)}
                        body={drawn.clone()}
                        placeholder={placeholder.clone()}
                        rows="4"
                        primary_target={Some(key.clone())}
                        quote_identity={None::<String>}
                        broadcast_help={None::<String>}
                        onkeydown={onkeydown.clone()}
                        onpaste={Callback::noop()}
                    />
                    <ComposerFocus id={textarea_id} active={focused} {requested} {settle} />
                </div>
                {render_quote_shards(&key, &draft, None, None, render.drafts, onkeydown, &placeholder)}
            </div>
        </section>
    }
}

/// A design's menu carries what its room holds besides conversation: its
/// preparation authority and, for an OOPS batch, the grooming form. Its group
/// rail mirrors an agent's team rail.
fn render_design_menu(render: &GroupRender<'_, '_>, view: &GroupView, channel: &Channel) -> Html {
    let label = format!("Menu for {}", channel.label);
    html! {
        <details
            class="agent-menu"
            data-app-overlay-disclosure=""
            onkeydown={Callback::from(crate::browser::overlay::dismiss_disclosure_on_escape)}
        >
            <summary class="agent-menu-button" aria-label={label.clone()}>
                <span class="composer-menu-icon" aria-hidden="true">
                    <span></span><span></span><span></span>
                </span>
            </summary>
            <div
                class="agent-menu-panel app-overlay-panel app-overlay-panel--seated"
                role="group"
                aria-label={label}
                data-design-menu={channel.stamp.to_string()}
            >
                <header class="agent-menu-head">
                    <strong>{channel.label.clone()}</strong>
                </header>
                {render_group_actions(render, view, channel)}
                {render_preparation(render.runtime, &channel.stamp)}
                {oops_grooming_form(render.runtime, &toast(render), render.sender, &channel.stamp)}
            </div>
        </details>
    }
}

/// The design-side twin of an agent's **Create new team** and **Leave all
/// teams**: begin a group holding only this design, or take it out of its
/// group, which ends once its last design leaves.
fn render_group_actions(render: &GroupRender<'_, '_>, view: &GroupView, channel: &Channel) -> Html {
    let Some(held) = render.runtime.room_projection().groups().get(&view.group) else {
        return Html::default();
    };
    let (group, head) = (held.value.clone(), held.position);
    let can_put = render.runtime.offers(wire::CommandKind::WorkingGroupPut)
        && group.anchors.contains(&channel.stamp);
    let can_split = can_put && group.anchors.len() > 1;
    let split = {
        let controls = render.controls.clone();
        let sender = render.sender.clone();
        let design = channel.stamp.clone();
        Callback::from(move |event: MouseEvent| {
            let Some(created) = super::mint_group() else {
                controls.report("A working-group identity could not be created.".into());
                return;
            };
            let alone = wire::rooms::WorkingGroup {
                group: created,
                anchors: vec![design.clone()],
            };
            super::submit_group(&sender, &controls, alone, None);
            close_menu(&event);
        })
    };
    let leave = {
        let controls = render.controls.clone();
        let sender = render.sender.clone();
        let design = channel.stamp.clone();
        Callback::from(move |event: MouseEvent| {
            let mut remaining = group.clone();
            remaining.anchors.retain(|held| *held != design);
            super::submit_group(&sender, &controls, remaining, Some(head));
            close_menu(&event);
        })
    };
    html! {
        <div class="app-control-rail" role="group" aria-label="Working group">
            <button
                class="agent-menu-action"
                type="button"
                disabled={!can_split}
                aria-label={format!("Create new group for {}", channel.label)}
                title={format!("Move only {} to a new group", channel.label)}
                data-group-split-design={channel.stamp.to_string()}
                onclick={split}
            >{"Create new group"}</button>
            <button
                class="agent-menu-action"
                type="button"
                disabled={!can_put}
                aria-label={format!("Leave group for {}", channel.label)}
                title={format!("Remove {} from its group", channel.label)}
                data-group-leave-design={channel.stamp.to_string()}
                onclick={leave}
            >{"Leave group"}</button>
        </div>
    }
}

/// Close the menu holding the clicked control.
fn close_menu(event: &MouseEvent) {
    if let Some(menu) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        .and_then(|element| element.closest("details").ok().flatten())
    {
        let _ = menu.remove_attribute("open");
    }
}

/// Post one design composer's message to exactly that design's room. A post
/// that lands clears the composer only while it still holds what was posted.
fn post_callback(
    render: &GroupRender<'_, '_>,
    group: &RoomAuthorityStamp,
    channel: &Channel,
    key: &str,
) -> Callback<()> {
    let runtime = render.runtime;
    let available = matches!(
        runtime
            .room_projection()
            .anchor_authority(&channel.stamp, runtime.board()),
        RoomAnchorAuthority::Available(_)
    ) && runtime.offers(wire::CommandKind::RoomPost);
    let task = runtime
        .board()
        .and_then(|board| {
            board
                .tasks
                .iter()
                .find(|task| task.statement.body.born == channel.stamp)
        })
        .map(|task| (task.statement.body.handle.clone(), task.statement.position))
        .filter(|_| available);
    let controls = render.controls.clone();
    let sender = render.sender.clone();
    let drafts = render.drafts.clone();
    let group = group.clone();
    let design = channel.stamp.clone();
    let key = key.to_owned();
    Callback::from(move |()| {
        let report = {
            let drafts = drafts.clone();
            move |issue: &str| {
                drafts.dispatch(ComposerAction::Report {
                    issue: issue.to_owned(),
                });
            }
        };
        let Some((room, head)) = task.clone() else {
            report("This design's room is unavailable.");
            return;
        };
        let Some(prepared) = prepare_composer(&key, &drafts) else {
            report("A room post cannot be empty.");
            return;
        };
        if !prepared.attachments.is_empty() {
            report("Room posts carry no images.");
            return;
        }
        let snapshot = prepared.snapshot;
        let landed = {
            let report = report.clone();
            let controls = controls.clone();
            let drafts = drafts.clone();
            let group = group.clone();
            let design = design.clone();
            let key = key.clone();
            move |result: Result<wire::rooms::RoomPostAnswer, String>| match result {
                Ok(answer) => {
                    if composer_holds(&key, &snapshot) {
                        clear_composer(&key, &drafts);
                        controls
                            .drawn
                            .borrow_mut()
                            .insert((group.clone(), design.clone()), String::new());
                        controls.update(RoomViewAction::Posted {
                            group: group.clone(),
                            design: design.clone(),
                            text: snapshot.body.trim().to_owned(),
                        });
                    }
                    report(&room_post_notice(&answer.outcome));
                }
                Err(issue) => report(&issue),
            }
        };
        let _ = admit_action(
            &sender,
            ClientAction::RoomPost(RoomPostAction {
                room,
                head,
                text: prepared.message.trim().to_owned(),
                grooming: None,
                report_heads: Vec::new(),
                respond: Rc::new(landed),
            }),
        );
    })
}

/// Whether a composer's live elements still hold exactly the posted draft:
/// the same primary text and the same quote shards with the same replies.
fn composer_holds(key: &str, posted: &crate::composer_drafts::ComposerDraft) -> bool {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return false;
    };
    let value = |id: &str| {
        document
            .get_element_by_id(id)
            .and_then(|element| element.dyn_into::<HtmlTextAreaElement>().ok())
            .map(|textarea| textarea.value())
    };
    let identities = document
        .query_selector_all(&format!(
            "[data-composer-quote-stack-target-id=\"{key}\"] [data-quote-identity]"
        ))
        .ok()
        .map(|nodes| {
            (0..nodes.length())
                .filter_map(|index| nodes.item(index))
                .filter_map(|node| node.dyn_into::<web_sys::Element>().ok())
                .filter_map(|element| element.get_attribute("data-quote-identity"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    value(&primary_textarea_id(key)).as_deref() == Some(posted.body.as_str())
        && identities.len() == posted.quotes.len()
        && posted
            .quotes
            .iter()
            .zip(&identities)
            .all(|(quote, identity)| {
                quote.identity == *identity
                    && value(&quote_textarea_id(key, &quote.identity)).as_deref()
                        == Some(quote.body.as_str())
            })
}
