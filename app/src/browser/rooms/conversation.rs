//! One sender-to-destination card family for room MSG publications.
//!
//! A card keeps its publication identity through every echo and move. Being
//! recorded in the room and each route's receipt are separate statuses; neither
//! is review or authorization.

use std::rc::Rc;

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, HtmlInputElement};
use wire::intent::{RoomControl, RoomControlAnswer, RoomConversationContent};
use wire::rooms::{
    RoomAuthorityStamp, RoomDestination, RoomPublicationContent, RoomReceiptDisposition,
    RoomReceiptRef, RoomReceiptState, RoomSource, RoomSourceUnavailable, TranscriptHunk,
};
use wire::{CanonicalUuid, CommandKind, Disposition};
use yew::prelude::*;

use super::card::{Badge, Face, Speaker};
use crate::AppRuntime;
use crate::browser::clock::{Age, Countdown, local_full};
use crate::browser::sender::{SenderHandle, admit_action};
use crate::client_action::{ClientAction, RoomControlRequest};
use crate::runtime::{
    Conversation, ConversationText, Delivery, ItemConcern, OperatorItem, RoomEvent, RoomEventKind,
};

/// What a card reads, and what its controls act through. A card without
/// controls renders its statuses and nothing that writes.
#[derive(Clone, Copy)]
struct Act<'a> {
    runtime: &'a AppRuntime,
    controls: Option<CardControls<'a>>,
}

/// Where a card's controls send and report their outcomes.
#[derive(Clone, Copy)]
pub(super) struct CardControls<'a> {
    pub(super) report: &'a Callback<String>,
    pub(super) sender: &'a SenderHandle,
}

/// Who spoke one conversation-family event, the words it carried when this
/// browser holds them, and the authority its card's routes read. A lane
/// speaker names its lane; the operator names none.
pub(super) struct Spoken<'a> {
    pub(super) author: String,
    pub(super) lane: Option<CanonicalUuid>,
    pub(super) text: Option<String>,
    conversation: Option<&'a Conversation>,
    packet: Option<&'a [CanonicalUuid]>,
}

/// The speaker and words of one MSG or plain post, or `None` for an event
/// outside this family.
pub(super) fn spoken<'a>(
    runtime: &'a AppRuntime,
    publication: &RoomAuthorityStamp,
    event: &'a RoomEvent,
) -> Option<Spoken<'a>> {
    let projection = runtime.room_projection().conversation();
    let (author, lane, echoed, conversation, packet) = match &event.kind {
        RoomEventKind::Messaged {
            source,
            directed,
            conversation,
            ..
        } => (
            lane_label(runtime, source.head.lane),
            Some(source.head.lane),
            echo(runtime, source),
            conversation
                .as_ref()
                .and_then(|stamp| projection.conversation(stamp)),
            Some(directed.as_slice()),
        ),
        RoomEventKind::Posted { author } => {
            let conversation = projection.conversation(publication)?;
            (
                crate::work::owner_label(author, None, runtime.principals()),
                None,
                conversation.text.clone(),
                Some(conversation),
                None,
            )
        }
        // A plain post is recorded in the room and directs no one.
        RoomEventKind::Published {
            author,
            content: RoomPublicationContent::Authored { text },
        } => (
            crate::work::owner_label(author, None, runtime.principals()),
            None,
            Some(text.clone()),
            None,
            None,
        ),
        _ => return None,
    };
    // A lane's bytes stay in its transcript; absent its echo, the text its
    // own deliveries carried, then the resident's read of that transcript,
    // stands in.
    let text = echoed.or_else(|| {
        packet
            .unwrap_or_default()
            .iter()
            .chain(conversation.map_or(&[][..], |conversation| &conversation.directed))
            .find_map(|directive| projection.delivery(directive))
            .map(|delivery| delivery.value.text.clone())
            .or_else(|| {
                conversation.and_then(|conversation| {
                    match runtime.conversation_text(conversation.publication.publication()) {
                        Some(ConversationText::Read {
                            content: RoomConversationContent::Text { text },
                            ..
                        }) => Some(text.clone()),
                        _ => None,
                    }
                })
            })
    });
    Some(Spoken {
        author,
        lane,
        text,
        conversation,
        packet,
    })
}

/// One room publication's card face, or `None` for an event outside this
/// family. Its controls' outcomes and refusals go to their `report`; without
/// controls the face only reads.
pub(super) fn face(
    runtime: &AppRuntime,
    controls: Option<CardControls<'_>>,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
    unheard: bool,
) -> Option<Face> {
    let act = Act { runtime, controls };
    let Spoken {
        author,
        lane,
        text,
        conversation,
        packet,
    } = spoken(runtime, publication, event)?;
    let routes = conversation.map_or_else(
        || {
            packet
                .unwrap_or_default()
                .iter()
                .map(|directive| directive_route(runtime, *directive, None, ""))
                .collect()
        },
        |conversation| routes(act, conversation),
    );
    let receivers = routes
        .iter()
        .map(|route| route.destination.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(Face {
        speaker: Some(Speaker { name: author, lane }),
        receivers,
        badge: unheard.then_some(Badge::Unheard),
        words: text.unwrap_or_else(|| "Source remains in the author's lane transcript.".to_owned()),
        tail: html! {
            <ul class="room-flow-statuses" aria-label="Delivery status">
                <li class="room-chip" data-status="recorded">{"Recorded in room"}</li>
                {for routes.into_iter().map(|route| route.html)}
            </ul>
        },
    })
}

/// This principal's operator items, each with the words it carries and its
/// exact controls, as the open count and the inbox body.
pub(super) fn render_inbox(
    runtime: &AppRuntime,
    report: &Callback<String>,
    sender: &SenderHandle,
) -> (usize, Html) {
    let act = Act {
        runtime,
        controls: Some(CardControls { report, sender }),
    };
    let projection = runtime.room_projection().conversation();
    let open = runtime.operator_inbox();
    let count = open.len();
    let body = html! {
        <>
            if open.is_empty() {
                <p class="room-empty">{"No room item awaits action."}</p>
            }
            {for open.into_iter().map(|entry| {
                let publication = entry.item.value.record.publication();
                let conversation = projection.conversation(publication);
                let summary = conversation.map_or_else(String::new, |conversation| {
                    format!("MSG from {}", party(runtime, conversation.publication.author()))
                });
                let sender_card = conversation.and_then(|conversation| {
                    match conversation.publication.source() {
                        RoomSource::Transcript(hunk) => transcript_card_href(runtime, hunk),
                        _ => None,
                    }
                });
                let words = conversation.map_or_else(
                    || "The publication this item stands for is not loaded.".to_owned(),
                    |conversation| inbox_words(runtime, conversation),
                );
                let (concern, direction) = match runtime.item_concern(&entry.item.value.record) {
                    ItemConcern::Direction { key, text } => {
                        (format!("About your direction {key}"), text)
                    }
                    ItemConcern::Task(task) => {
                        (format!("About {}", super::anchor_label(runtime, &task)), None)
                    }
                    ItemConcern::Unnamed => ("Retained item · no recorded concern".to_owned(), None),
                };
                html! {
                    <article
                        class="room-card room-flow-card"
                        key={entry.key.to_string()}
                        data-item={entry.key.to_string()}
                        data-admitted={entry.item.value.admitted.to_string()}
                        tabindex="0"
                    >
                        <header>
                            <strong class="room-flow-route">
                                if let Some(href) = sender_card {
                                    <a href={href}>{summary}</a>
                                } else {
                                    {summary}
                                }
                            </strong>
                            <span>{concern}</span>
                            <code>{publication.to_string()}</code>
                        </header>
                        if let Some(direction) = direction {
                            <blockquote class="room-flow-concern">
                                {super::card::render_words(runtime, &direction, None, None)}
                            </blockquote>
                        }
                        {super::card::render_words(runtime, &words, None, None)}
                        <ul class="room-flow-statuses" aria-label="Receipt">
                            {item_status(act, entry.key, &entry.item.value, "You")}
                        </ul>
                        <p class="room-flow-guide">
                            {"Reply sends operator direction and settles the item; ACK or NAK settles it without a reply."}
                        </p>
                    </article>
                }
            })}
        </>
    };
    (count, body)
}

/// What one inbox publication says: the operator's own text, else the
/// resident's read of the authoring lane's transcript, else that lane's echo
/// while the read runs.
fn inbox_words(runtime: &AppRuntime, conversation: &Conversation) -> String {
    if let Some(text) = &conversation.text {
        return text.clone();
    }
    match runtime.conversation_text(conversation.publication.publication()) {
        Some(ConversationText::Read {
            content: RoomConversationContent::Text { text },
            ..
        }) => text.clone(),
        Some(ConversationText::Read {
            content: RoomConversationContent::Linked { source },
            ..
        }) => format!("Linked source: {source:?}"),
        Some(ConversationText::Read {
            content: RoomConversationContent::Unavailable { unavailable },
            ..
        }) => format!("Text unavailable: {}", unavailable_reason(unavailable)),
        Some(ConversationText::Failed(reason)) => format!("Text could not be read: {reason}"),
        Some(ConversationText::Loading) | None => match conversation.publication.source() {
            RoomSource::Transcript(hunk) => echo(runtime, hunk),
            _ => None,
        }
        .unwrap_or_else(|| "Reading the author's transcript…".to_owned()),
    }
}

pub(super) fn unavailable_reason(unavailable: &RoomSourceUnavailable) -> String {
    match unavailable {
        RoomSourceUnavailable::Missing {} => "the source is missing".to_owned(),
        RoomSourceUnavailable::Hidden {} => "the source is hidden".to_owned(),
        RoomSourceUnavailable::Changed {} => "the source no longer holds these bytes".to_owned(),
        RoomSourceUnavailable::Unreadable {} => "the source cannot be read now".to_owned(),
        RoomSourceUnavailable::UnsupportedRetained { release } => {
            format!("retained by release {release}, which this release cannot decode")
        }
    }
}

/// One route's destination and its receipt status.
struct Route {
    destination: String,
    html: Html,
}

fn routes(act: Act<'_>, conversation: &Conversation) -> Vec<Route> {
    let mut directed = conversation.directed.iter();
    conversation
        .publication
        .routes()
        .iter()
        .map(|route| match &route.destination {
            RoomDestination::Lane { lane } => match directed.next() {
                Some(directive) => directive_route(act.runtime, *directive, Some(*lane), ""),
                None => unavailable_route(lane_label(act.runtime, *lane)),
            },
            RoomDestination::Operator {} => {
                let destination = operator_destination(act.runtime, conversation);
                match conversation.publication.operator_item() {
                    Some((item, _)) => item_route(act, &item, &destination, ""),
                    None => unavailable_route(destination),
                }
            }
            RoomDestination::Task { task } => {
                let destination = format!("#{}", super::anchor_label(act.runtime, task));
                Route {
                    html: html! {
                        <li class="room-chip" data-status="room">
                            {format!("{destination}: recorded there; no directed recipient")}
                        </li>
                    },
                    destination,
                }
            }
        })
        .collect()
}

/// The principal an operator route reaches, or its concern while that
/// principal's read is unavailable. Never present the retired context-free
/// `operator` selector as though the author had named it.
fn operator_destination(runtime: &AppRuntime, conversation: &Conversation) -> String {
    let recipient = match runtime.conversation_text(conversation.publication.publication()) {
        Some(ConversationText::Read {
            recipient: Some(recipient),
            ..
        }) => Some(crate::work::owner_label(
            recipient,
            None,
            runtime.principals(),
        )),
        _ => None,
    };
    let direction = conversation.publication.routes().iter().find_map(|route| {
        matches!(route.destination, RoomDestination::Operator {})
            .then_some(route.direction.as_deref())
            .flatten()
    });
    crate::runtime::operator_destination_label(recipient, direction)
}

fn unavailable_route(destination: String) -> Route {
    Route {
        html: html! {
            <li class="room-chip" data-status="unavailable">
                {format!("{destination}: receipt unavailable")}
            </li>
        },
        destination,
    }
}

/// A lane receipt is the native steering directive delivering it.
fn directive_route(
    runtime: &AppRuntime,
    directive: CanonicalUuid,
    lane: Option<CanonicalUuid>,
    lead: &str,
) -> Route {
    let delivery = runtime
        .room_projection()
        .conversation()
        .delivery(&directive)
        .map(|delivery| &delivery.value);
    let Some(lane) = lane.or_else(|| delivery.map(|delivery| delivery.lane)) else {
        return unavailable_route("Lane".to_owned());
    };
    let destination = lane_label(runtime, lane);
    let Some(delivery) = delivery else {
        return unavailable_route(destination);
    };
    let (status, label) = delivery_status(runtime, delivery);
    Route {
        html: {
            let text = format!("{lead}{destination}: {label}");
            html! {
                <li class="room-chip" data-status={status} data-receipt-key={delivery.key.clone()} title={text.clone()}>
                    {text}
                </li>
            }
        },
        destination,
    }
}

fn delivery_status(runtime: &AppRuntime, delivery: &Delivery) -> (&'static str, &'static str) {
    let member = runtime
        .lanes()
        .and_then(|lanes| lanes.member(delivery.lane));
    match delivery.disposition {
        Disposition::Live if delivery.queued => ("unheard", "queued for vacant lane · unheard"),
        Disposition::Live if member.is_none() => {
            ("unavailable", "lane unavailable · receipt pending")
        }
        Disposition::Live if member.is_some_and(|member| member.occupant.is_none()) => {
            ("vacant", "lane vacant · receipt pending")
        }
        Disposition::Live => ("pending", "receipt pending"),
        Disposition::Acked => ("acked", "acked"),
        Disposition::Declined => ("declined", "declined"),
        Disposition::Lapsed => ("expired", "receipt expired"),
        Disposition::DeadLettered => ("unavailable", "undeliverable · unavailable"),
        Disposition::Canceled => ("cancelled", "cancelled"),
    }
}

fn item_route(act: Act<'_>, key: &RoomReceiptRef, destination: &str, lead: &str) -> Route {
    let html = match act.runtime.room_projection().conversation().item(key) {
        Some(item) => item_status(act, key, &item.value, &format!("{lead}{destination}")),
        None => return unavailable_route(destination.to_owned()),
    };
    Route {
        destination: destination.to_owned(),
        html,
    }
}

/// The operator's own receipt, with reply and ACK/NAK while pending and
/// recovery once lapsed. Its control slot keeps one height in every state, so
/// a receipt settling never moves the cards after it.
fn item_status(act: Act<'_>, key: &RoomReceiptRef, item: &OperatorItem, destination: &str) -> Html {
    let none = || (Html::default(), "", None);
    let (status, label, (clock, suffix, exact)) = match &item.state {
        RoomReceiptState::Pending { expires } => {
            let exact = local_full(expires);
            (
                "pending",
                "receipt pending · expires in ",
                (
                    html! {
                        <Countdown
                            expiry={expires.get().as_millisecond()}
                            class={classes!("room-receipt-time")}
                            title={exact.clone()}
                        />
                    },
                    "",
                    Some(format!("receipt pending · expires {exact}")),
                ),
            )
        }
        RoomReceiptState::Acked { .. } => ("acked", "acked", none()),
        RoomReceiptState::Declined { .. } => ("declined", "declined", none()),
        RoomReceiptState::Expired { at } => {
            let exact = local_full(at);
            (
                "expired",
                "receipt expired ",
                (
                    html! {
                        <Age
                            authored={at.get().as_millisecond()}
                            class={classes!("room-receipt-time")}
                            title={exact.clone()}
                        />
                    },
                    " ago",
                    Some(format!("receipt expired {exact}")),
                ),
            )
        }
        RoomReceiptState::Cancelled { .. } => ("cancelled", "cancelled", none()),
    };
    let control = match (&item.state, act.controls) {
        (RoomReceiptState::Pending { .. }, Some(controls)) => {
            dispose_controls(act.runtime, controls, key)
        }
        (RoomReceiptState::Expired { .. }, Some(controls)) => {
            recover_control(act.runtime, controls, key)
        }
        _ => Html::default(),
    };
    let text = format!("{destination}: {label}");
    let hover = format!("{destination}: {}", exact.as_deref().unwrap_or(label));
    html! {
        <li class="room-chip" data-status={status} data-item-key={key.to_string()}>
            <span title={hover}>{text}{clock}{suffix}</span>
            <div class="room-flow-slot">{control}</div>
        </li>
    }
}

/// A reply is operator direction to the item's lane and settles it; ACK and
/// NAK settle silently, their clause reaching no lane.
fn dispose_controls(
    runtime: &AppRuntime,
    controls: CardControls<'_>,
    key: &RoomReceiptRef,
) -> Html {
    let text = NodeRef::default();
    let enabled = runtime.offers(CommandKind::RoomControl);
    let control = |make: fn(&RoomReceiptRef, String) -> RoomControl| {
        let act = Owned::from(controls);
        let text = text.clone();
        let key = key.clone();
        move |event: &Event| {
            act.send(
                event,
                RoomControlRequest::Control(make(&key, input_value(&text))),
            );
        }
    };
    let reply: fn(&RoomReceiptRef, String) -> RoomControl = |key, text| RoomControl::Reply {
        item: key.clone(),
        text,
    };
    let ack: fn(&RoomReceiptRef, String) -> RoomControl = |key, clause| RoomControl::Dispose {
        disposition: RoomReceiptDisposition::Ack,
        keys: vec![key.clone()],
        clause,
    };
    let nak: fn(&RoomReceiptRef, String) -> RoomControl = |key, clause| RoomControl::Dispose {
        disposition: RoomReceiptDisposition::Nak,
        keys: vec![key.clone()],
        clause,
    };
    let submit = control(reply);
    let ack = control(ack);
    let nak = control(nak);
    let prompt = "Reply, or a settling clause";
    html! {
        <form class="room-flow-control" onsubmit={Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            submit(&event);
        })}>
            <input ref={text} aria-label={prompt} placeholder={prompt} disabled={!enabled} />
            <button type="submit" disabled={!enabled}>{"Reply"}</button>
            <button type="button" disabled={!enabled} onclick={Callback::from(move |event: MouseEvent| ack(&event))}>{"ACK"}</button>
            <button type="button" disabled={!enabled} onclick={Callback::from(move |event: MouseEvent| nak(&event))}>{"NAK"}</button>
        </form>
    }
}

fn recover_control(runtime: &AppRuntime, controls: CardControls<'_>, key: &RoomReceiptRef) -> Html {
    let enabled = runtime.offers(CommandKind::RoomControl);
    let act = Owned::from(controls);
    let key = key.clone();
    let onclick = Callback::from(move |event: MouseEvent| {
        act.send(
            &event,
            RoomControlRequest::Control(RoomControl::Recover { item: key.clone() }),
        );
    });
    html! {
        <button type="button" class="room-flow-control" disabled={!enabled} {onclick}>{"Recover"}</button>
    }
}

/// Control context a callback can own.
#[derive(Clone)]
struct Owned {
    report: Callback<String>,
    sender: SenderHandle,
}

impl From<CardControls<'_>> for Owned {
    fn from(controls: CardControls<'_>) -> Self {
        Self {
            report: controls.report.clone(),
            sender: controls.sender.clone(),
        }
    }
}

impl Owned {
    /// Send one control and report its outcome where its surface reports.
    /// Keyboard focus moves to the acting card first, so the control
    /// retiring never strands it.
    fn send(&self, event: &Event, request: RoomControlRequest) {
        if let Some(card) = acting_card(event) {
            let _ = card.focus();
        }
        let report = self.report.clone();
        let respond = Rc::new(move |answer: Result<RoomControlAnswer, String>| {
            report.emit(match answer {
                Ok(answer) => control_notice(&answer),
                Err(error) => error,
            });
        });
        let _ = admit_action(&self.sender, ClientAction::RoomControl { request, respond });
    }
}

fn control_notice(answer: &RoomControlAnswer) -> String {
    let mut moves = answer
        .items
        .iter()
        .map(|moved| format!("item {} {}", moved.item, moved.state))
        .collect::<Vec<_>>();
    if let Some(publication) = &answer.publication {
        moves.push(format!("reply {publication} recorded"));
    }
    if let Some(returned) = &answer.returned {
        moves.push(format!("returned as {returned}"));
    }
    if let Some((publication, verdict)) = &answer.assessed {
        moves.push(format!(
            "publication {publication} assessed {}",
            verdict.name()
        ));
    }
    if moves.is_empty() {
        "Room control recorded.".to_owned()
    } else {
        format!("Room control recorded: {}.", moves.join("; "))
    }
}

/// The card a control acts on, or the inbox holding it, since a settled
/// item leaves the inbox.
fn acting_card(event: &Event) -> Option<HtmlElement> {
    let target = event.target()?.dyn_into::<Element>().ok()?;
    let inbox = target.closest("[data-room-inbox]").ok().flatten();
    inbox
        .or_else(|| {
            target
                .closest(".room-card, .mosaic-room-card")
                .ok()
                .flatten()
        })?
        .dyn_into::<HtmlElement>()
        .ok()
}

fn input_value(node: &NodeRef) -> String {
    node.cast::<HtmlInputElement>()
        .map(|input| input.value())
        .unwrap_or_default()
}

fn party(runtime: &AppRuntime, destination: &RoomDestination) -> String {
    match destination {
        RoomDestination::Operator {} => "Operator".to_owned(),
        RoomDestination::Lane { lane } => lane_label(runtime, *lane),
        RoomDestination::Task { task } => format!("#{}", super::anchor_label(runtime, task)),
    }
}

pub(super) fn lane_label(runtime: &AppRuntime, lane: CanonicalUuid) -> String {
    runtime
        .lanes()
        .and_then(|lanes| lanes.member(lane))
        .map_or_else(
            || format!("lane {}", &lane.to_string()[..8]),
            |member| {
                crate::watchband::agent_message_label(&member.worktree, member.name.as_deref())
                    .to_owned()
            },
        )
}

/// The loaded agent message one transcript hunk was contributed from.
fn transcript_message<'a>(
    runtime: &'a AppRuntime,
    hunk: &TranscriptHunk,
) -> Option<&'a wire::AgentMessage> {
    runtime
        .messages()
        .iter()
        .find(|message| message.lane == hunk.head.lane && message.locus == hunk.locus)
        .map(Rc::as_ref)
}

/// The card route of the loaded message one transcript hunk came from.
pub(super) fn transcript_card_href(runtime: &AppRuntime, hunk: &TranscriptHunk) -> Option<String> {
    transcript_message(runtime, hunk).map(|message| {
        crate::view::card_href(&crate::browser::watchband::agent_message_source_id(message))
    })
}

/// The exact contributed bytes when the author's transcript echo is loaded.
fn echo(runtime: &AppRuntime, hunk: &TranscriptHunk) -> Option<String> {
    let start = usize::try_from(hunk.text.start).ok()?;
    let end = usize::try_from(hunk.text.end).ok()?;
    transcript_message(runtime, hunk)
        .and_then(|message| message.text.get(start..end))
        .map(str::to_owned)
}
