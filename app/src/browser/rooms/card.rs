//! One room card face for every surface, shaped as a team message card's: the
//! words lead with a signoff naming the age, the speaker in the accent their
//! lane wears in a team, the flow to any receivers, and the badges that
//! classify what was said; the statuses or sources the publication carries
//! follow. A surface places the face in its own card and never nests a second
//! card around it or shows a publication key.

use wire::CanonicalUuid;
use wire::rooms::RoomAuthorityStamp;
use yew::prelude::*;

use super::{contributions, conversation};
use crate::AppRuntime;
use crate::browser::clock::Age;
use crate::runtime::RoomEvent;
use crate::watchband;

/// Who said one card's words. A lane wears its team accent; the operator and
/// a lane this browser has not loaded keep the card's own accent.
pub(super) struct Speaker {
    pub(super) name: String,
    pub(super) lane: Option<CanonicalUuid>,
}

/// What classifies one card, beside its speaker.
pub(super) enum Badge {
    /// A question or its answer, worn as a quoted packet wears it.
    Packet(Packet),
    /// A room-only kind, such as a typed contribution or a recorded room
    /// event: a stable key and the words that name it.
    Kind {
        kind: &'static str,
        label: &'static str,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Packet {
    Ask,
    Ans,
}

/// One card's signoff-led words and what follows them.
pub(super) struct Face {
    pub(super) speaker: Option<Speaker>,
    /// The receivers after the flow arrow; empty when the card directs no one.
    pub(super) receivers: String,
    pub(super) badge: Option<Badge>,
    pub(super) words: String,
    pub(super) tail: Html,
}

/// Where a surface anchors its cards: the card's element id, its age link's
/// accessible label, and the corner island of card actions, which leads the
/// words so they flow around it as a team card's prose does.
pub(super) struct Anchor<'a> {
    pub(super) id: &'a str,
    pub(super) label: String,
    pub(super) island: Html,
}

/// One room event as a card in a surface's list.
pub(super) fn render_card(
    runtime: &AppRuntime,
    controls: Option<conversation::CardControls<'_>>,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
) -> Html {
    html! {
        <article class="room-card" key={publication.to_string()} tabindex="0">
            {render_face(runtime, controls, publication, event, None)}
        </article>
    }
}

/// One room event's face, for a surface that supplies its own card.
pub(super) fn render_face(
    runtime: &AppRuntime,
    controls: Option<conversation::CardControls<'_>>,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
    anchor: Option<Anchor<'_>>,
) -> Html {
    let face = conversation::face(runtime, controls, publication, event)
        .or_else(|| contributions::face(runtime, publication, event))
        .unwrap_or_else(|| super::summary_face(runtime, event));
    render(runtime, publication, face, anchor)
}

pub(super) fn render(
    runtime: &AppRuntime,
    publication: &RoomAuthorityStamp,
    face: Face,
    anchor: Option<Anchor<'_>>,
) -> Html {
    let Face {
        speaker,
        receivers,
        badge,
        words,
        tail,
    } = face;
    let (href, label, island) = anchor.map_or_else(
        || (None, AttrValue::default(), Html::default()),
        |anchor| {
            (
                Some(AttrValue::from(crate::view::card_href(anchor.id))),
                AttrValue::from(anchor.label),
                anchor.island,
            )
        },
    );
    html! {
        <div class="message-flow room-card-face">
            <div class="message-body markdown-body">
                {island}
                <p class="room-card-words">
                    <span class="message-signoff">
                        <Age authored={publication.micros() / 1_000} class="relative-age" {href} {label} />
                        {for speaker.map(|speaker| render_speaker(runtime, speaker))}
                        if !receivers.is_empty() {
                            <span class="message-quote-direction" aria-label="to">{"→"}</span>
                            <span class="message-quote-recipients">{receivers}</span>
                        }
                        if let Some(badge) = badge {
                            <span class="message-classification-group" role="group" aria-label="Message classifications">
                                {render_badge(badge)}
                            </span>
                        }
                    </span>
                    {" "}{words}
                </p>
            </div>
            {tail}
        </div>
    }
}

fn render_speaker(runtime: &AppRuntime, speaker: Speaker) -> Html {
    let accent = speaker.lane.and_then(|lane| {
        runtime
            .lanes()
            .and_then(|lanes| watchband::lane_accent(&lanes.lanes, lane))
    });
    html! {
        <span class="message-agent-name" data-accent={accent.map(|accent| accent.to_string())}>
            <span class="message-agent-label">{speaker.name}</span>
        </span>
    }
}

fn render_badge(badge: Badge) -> Html {
    match badge {
        Badge::Packet(packet) => {
            let (kind, acknowledgement, label, text) = match packet {
                Packet::Ask => ("ask", "question", "Question", "ASK"),
                Packet::Ans => ("ans", "answer", "Answer", "ANS"),
            };
            html! {
                <span
                    class="message-ack-pill"
                    data-message-classification={kind}
                    data-acknowledgement={acknowledgement}
                    data-message-packet={kind}
                    aria-label={label}
                >
                    <span class="message-ack-label">{text}</span>
                </span>
            }
        }
        Badge::Kind { kind, label } => html! {
            <span class="message-room-pill" data-message-classification={kind}>
                {label}
            </span>
        },
    }
}
