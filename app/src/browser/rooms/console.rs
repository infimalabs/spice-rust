//! Working-group consoles: a team watchband with tasks and agents swapped.
//!
//! A group's designs are its composers and its accented channels; its toolbar
//! is the team toolbar read from those designs. Every other piece is the team
//! console's own: the packed plane, its composer slot, the quote shards, and
//! the composer issue toast.

mod panel;
mod toolbar;

pub(super) use panel::use_arrangement_focus;

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
use crate::runtime::{
    RoomAnchorAuthority, RoomAnchorSettlement, RoomEvent, RoomEventKind, RoomProjectionStatus,
};
use crate::watchband;
use crate::work::WorkProjection;

/// Everything one render of the opened group consoles reads.
pub(in crate::browser) struct GroupRender<'a, 'w> {
    pub(in crate::browser) runtime: &'a AppRuntime,
    pub(in crate::browser) work: Option<&'a WorkProjection<'w>>,
    pub(in crate::browser) controls: &'a Controls,
    pub(in crate::browser) sender: &'a SenderHandle,
    pub(in crate::browser) drafts: &'a UseReducerHandle<ComposerDrafts>,
    pub(in crate::browser) open_message_task: &'a Callback<(TaskStamp, web_sys::Element)>,
    pub(in crate::browser) navigate: &'a Callback<crate::browser::navigation::Intent>,
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
    let authoritative = projection
        .partition()
        .groups
        .iter()
        .find(|group| group.group == view.group);
    // Absence is shown only once a completed snapshot lacks the group; while
    // authority reloads, the console keeps its last name and its geometry.
    let absent = loaded && authoritative.is_none();
    let name = group_name(render.runtime, render.controls, view, authoritative, absent);
    let anchors = authoritative.map_or(&[][..], |group| group.anchors.as_slice());
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
    // A settled design takes no post, so it has no place on the post control;
    // every other design's composer posts through its own entry there.
    let mut postables = Vec::new();
    let mut composers = Vec::with_capacity(channels.len());
    for channel in &channels {
        composers.push(
            match projection.anchor_authority(&channel.stamp, render.runtime.board()) {
                RoomAnchorAuthority::Settled { standing, .. } => {
                    render_settled_design(render, channel, standing)
                }
                _ => {
                    let postable = design_post(render, &view.group, channel);
                    let composer =
                        render_design_composer(render, view, channel, postable.post.clone());
                    postables.push(postable);
                    composer
                }
            },
        );
    }
    let posting = render
        .controls
        .posting
        .borrow()
        .iter()
        .any(|(group, _)| *group == view.group);
    let composer = html! {
        <form class="team-composer" onsubmit={panel::submit(&postables)}>
            <div class="team-agents">{for composers}</div>
            {panel::render(render.controls, &view.group, projection.partition().revision, &postables, posting)}
        </form>
    };
    html! {
        <section
            key={view.group.to_string()}
            class="team-surface group-surface"
            data-group={view.group.to_string()}
            data-team-weight={watchband::team_visual_weight(channels.len()).to_string()}
            onclick={panel::dismissal(render.controls, &view.group)}
        >
            {toolbar::render(render, view, &name, &channels, loaded)}
            {panel::render_arrangement(render.controls, view, index, &name)}
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
    let report = toast(render);
    let controls = super::conversation::CardControls {
        report: &report,
        sender: render.sender,
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
            {render_event(render.runtime, Some(controls), publication, event)}
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

/// What a design composer's post does: it records and asks no lane.
const POST_CUE: &str = "Posts are recorded in this room for its listeners; they ask no lane for anything. To ask a lane, use Coordinate in this design's menu.";

/// One open design's composer: its heading and menu, the textarea posting to
/// exactly that design's room, and the quote shards drafted beneath it.
/// Command or Control and Enter posts this composer alone, as it sends one
/// member's draft in a team.
fn render_design_composer(
    render: &GroupRender<'_, '_>,
    view: &GroupView,
    channel: &Channel,
    post: Callback<()>,
) -> Html {
    let key = design_key(&view.group, &channel.stamp);
    let draft = render.drafts.get(&key).cloned().unwrap_or_default();
    let drawn = render.controls.drawn(view, &channel.stamp);
    let onkeydown = Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            if !event.repeat() {
                post.emit(());
            }
        }
    });
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
                    {render_design_heading(render, channel)}
                    <p class="composer-shard-cue" data-room-post-cue="">{POST_CUE}</p>
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

/// A settled design's room takes no new posts, so its composer names that
/// standing in place of the draft; the room's history stays in the plane.
fn render_settled_design(
    render: &GroupRender<'_, '_>,
    channel: &Channel,
    standing: RoomAnchorSettlement,
) -> Html {
    let standing = standing.as_str();
    html! {
        <section
            key={channel.stamp.to_string()}
            class="team-agent team-design"
            data-design={channel.stamp.to_string()}
            data-accent={channel.accent.to_string()}
            data-room-settled={standing}
            title={channel.label.clone()}
        >
            <div class="agent-composer-stack" data-has-quote-shards="false">
                <div class="mosaic-composer composer-shard composer-shard--primary" data-shard-kind="primary">
                    {render_design_heading(render, channel)}
                    <p class="composer-shard-settled" role="status">
                        {format!("This design is {standing}; its room stays readable and takes no new posts.")}
                    </p>
                </div>
            </div>
        </section>
    }
}

/// A design's heading, as a team member's resolves from its newest prose:
/// the room's newest publication whose words this browser holds, aged and
/// linked to its card, or the design's title until any is held.
fn render_design_heading(render: &GroupRender<'_, '_>, channel: &Channel) -> Html {
    let newest = newest_words(render.runtime, &channel.stamp);
    let age = match &newest {
        Some((publication, _)) => {
            let source_id = format!("{ROOM_CARD_PREFIX}{publication}");
            html! {
                <a
                    class="composer-shard-link composer-shard-age"
                    href={crate::view::card_href(&source_id)}
                    aria-label={format!("Jump to latest {} publication", channel.label)}
                >
                    <Age authored={publication.micros() / 1_000} class="composer-shard-age-label" />
                </a>
            }
        }
        None => html! {
            <span
                class="composer-shard-link composer-shard-age composer-shard-age-label"
                data-age-slot={watchband::AGE_SLOT_WIDTH.to_string()}
            >{"—"}</span>
        },
    };
    let prose = newest.map_or_else(
        || design_title(render.runtime, &channel.stamp).unwrap_or_else(|| channel.label.clone()),
        |(_, prose)| prose,
    );
    html! {
        <header class="app-glass-control-surface composer-shard-heading">
            {age}
            <strong class="composer-shard-prose" title={prose.clone()}>{prose}</strong>
            {render_design_menu(render, channel)}
        </header>
    }
}

/// A design's title as plain text, wherever the board still carries its row.
fn design_title(runtime: &AppRuntime, design: &TaskStamp) -> Option<String> {
    let board = runtime.board()?;
    board
        .tasks
        .iter()
        .chain(&board.room_anchors)
        .find(|task| task.statement.body.born == *design)
        .map(|task| crate::composer_markdown::inline::plain_text(&task.presentation_title))
}

/// The newest room event, in journal order, whose words this browser holds,
/// as its publication and first authored line.
fn newest_words(runtime: &AppRuntime, design: &TaskStamp) -> Option<(RoomAuthorityStamp, String)> {
    let mut events = runtime
        .room_projection()
        .room(design)?
        .events()
        .iter()
        .collect::<Vec<_>>();
    events.sort_by_key(|(_, event)| Reverse(event.position));
    events.into_iter().find_map(|(publication, event)| {
        let text = super::conversation::spoken(runtime, publication, event)?.text?;
        let line = watchband::first_nonquoted_prose(&text)?;
        Some((publication.clone(), line))
    })
}

/// A design's menu carries what its room holds besides conversation: its
/// standing, the coordination that asks a lane for a contribution, its
/// preparation authority and, for an OOPS batch, the grooming form. Its group
/// rail mirrors an agent's team rail.
fn render_design_menu(render: &GroupRender<'_, '_>, channel: &Channel) -> Html {
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
                {render_group_actions(render, channel)}
                {super::standing::render_standing(render.runtime, &channel.stamp, render.runtime.board().and_then(|board| board.row(&channel.stamp)))}
                {super::coordination::coordination_form(render.runtime, &toast(render), render.sender, &channel.stamp)}
                {render_preparation(render.runtime, &channel.stamp)}
                {oops_grooming_form(render.runtime, &toast(render), render.sender, &channel.stamp)}
            </div>
        </details>
    }
}

/// The design-side twin of an agent's **Create new team** and **Leave all
/// teams**: begin a group holding only this design, or take it out of its
/// group, which ends once its last design leaves.
fn render_group_actions(render: &GroupRender<'_, '_>, channel: &Channel) -> Html {
    let runtime = render.runtime;
    let partition = runtime.room_projection().partition();
    let tasks = runtime
        .board()
        .map_or(&[][..], |board| board.tasks.as_slice());
    let enabled = runtime.offers(wire::CommandKind::WorkingGroupMove);
    let menu = crate::roster::working_groups(partition, tasks).menu(&channel.stamp);
    let revision = partition.revision;
    let action = |change: Option<crate::roster::Change<RoomAuthorityStamp, wire::TaskStamp>>| {
        let change = change.filter(|_| enabled);
        let disabled = change.is_none();
        let controls = render.controls.clone();
        let sender = render.sender.clone();
        let activate = Callback::from(move |event: MouseEvent| {
            if let Some(change) = change.clone() {
                super::submit_change(&sender, &controls, change, revision);
                close_menu(&event);
            }
        });
        (disabled, activate)
    };
    let (split_disabled, split) = action(menu.new_group);
    let (leave_disabled, leave) = action(menu.leave);
    html! {
        <div class="app-control-rail" role="group" aria-label="Working group">
            <button
                class="agent-menu-action"
                type="button"
                disabled={split_disabled}
                aria-label={format!("New working group for {}", channel.label)}
                title={format!("Move only {} to a new working group", channel.label)}
                data-group-split-design={channel.stamp.to_string()}
                onclick={split}
            >{"New working group"}</button>
            <button
                class="agent-menu-action"
                type="button"
                disabled={leave_disabled}
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
/// that lands clears the composer only while it still holds what was posted;
/// one that is refused leaves that composer's draft in place. Each report
/// names its design, since one press may post to several rooms, and a
/// composer whose post awaits its answer is not posted again.
fn design_post(
    render: &GroupRender<'_, '_>,
    group: &RoomAuthorityStamp,
    channel: &Channel,
) -> panel::Postable {
    let runtime = render.runtime;
    let key = design_key(group, &channel.stamp);
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
    let attached = !render
        .drafts
        .get(&key)
        .is_none_or(|draft| draft.quotes.is_empty());
    let controls = render.controls.clone();
    let sender = render.sender.clone();
    let drafts = render.drafts.clone();
    let group = group.clone();
    let design = channel.stamp.clone();
    let label = channel.label.clone();
    let reachable = task.is_some();
    let post = Callback::from(move |()| {
        let report = {
            let drafts = drafts.clone();
            let label = label.clone();
            move |issue: &str| {
                drafts.dispatch(ComposerAction::Report {
                    issue: format!("{label}: {issue}"),
                });
            }
        };
        let flight = (group.clone(), design.clone());
        if controls.posting.borrow().contains(&flight) {
            return;
        }
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
            let flight = flight.clone();
            let key = key.clone();
            move |result: Result<wire::rooms::RoomPostAnswer, String>| {
                controls.posting.borrow_mut().remove(&flight);
                match result {
                    Ok(answer) => {
                        if composer_holds(&key, &snapshot) {
                            clear_composer(&key, &drafts);
                            controls
                                .drawn
                                .borrow_mut()
                                .insert(flight.clone(), String::new());
                            controls.update(RoomViewAction::Posted {
                                group: flight.0.clone(),
                                design: flight.1.clone(),
                                text: snapshot.body.trim().to_owned(),
                            });
                        }
                        report(&room_post_notice(&answer.outcome));
                    }
                    Err(issue) => report(&issue),
                }
            }
        };
        controls.posting.borrow_mut().insert(flight.clone());
        if !admit_action(
            &sender,
            ClientAction::RoomPost(RoomPostAction {
                room,
                head,
                text: prepared.message.trim().to_owned(),
                grooming: None,
                report_heads: Vec::new(),
                respond: Rc::new(landed),
            }),
        ) {
            controls.posting.borrow_mut().remove(&flight);
        }
    });
    panel::Postable {
        design: channel.stamp.clone(),
        accent: channel.accent,
        available: reachable,
        attached,
        post,
    }
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
