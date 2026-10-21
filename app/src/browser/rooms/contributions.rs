//! Typed Design contributions read in place: grouped by kind, and in the
//! room history in journal order beside ordinary conversation. Each entry
//! names its lane, time, charge, the question item it weighs, text, and every
//! cited source as an exact link or the typed reason the resident cannot
//! resolve it. Contributions are evidence for the operator's Design judgment,
//! never authority; reading them writes nothing.

use wire::TaskStamp;
use wire::rooms::{RoomAuthorityStamp, RoomContributionKind, RoomSource, RoomSourceView};
use yew::prelude::*;

use super::card::{Badge, Face, Speaker};
use crate::AppRuntime;
use crate::browser::view::chrome::CopyButton;
use crate::runtime::{Contribution, ContributionRead, RoomEvent, RoomEventKind};

/// Words naming one contribution kind: a charge's signed weight and sense,
/// or a question.
pub(super) const fn kind_label(kind: RoomContributionKind) -> &'static str {
    match kind {
        RoomContributionKind::Charge(charge) => match charge.get() {
            2 => "+2 strongly for",
            1 => "+1 for",
            0 => "0 evidence",
            -1 => "-1 against",
            -2 => "-2 strongly against",
            _ => panic!("a charge lies in -2..=2"),
        },
        RoomContributionKind::Question => "Question",
    }
}

/// `design`'s contributions grouped by kind, each group oldest first.
pub(super) fn render_groups(runtime: &AppRuntime, design: &TaskStamp) -> Html {
    let groups = runtime.room_projection().contribution_groups(design);
    let total = groups
        .iter()
        .map(|group| group.entries.len())
        .sum::<usize>();
    html! {
        <section class="room-contributions" aria-label="Contributions" data-room-contributions="">
            <header>
                <strong>{"Contributions"}</strong>
                <span>{total}</span>
            </header>
            if groups.is_empty() {
                <p class="room-empty">{"No contributions."}</p>
            }
            {for groups.into_iter().map(|group| {
                let label = kind_label(group.kind);
                html! {
                    <section
                        class="room-contribution-group"
                        key={group.kind.as_str()}
                        data-contribution-group={group.kind.as_str()}
                        aria-label={format!("{label} contributions")}
                    >
                        <h4>{label}<span class="room-contribution-count">{group.entries.len()}</span></h4>
                        <ol>
                            {for group.entries.iter().map(|entry| html! {
                                <li key={entry.publication.to_string()}>{render_entry(runtime, entry)}</li>
                            })}
                        </ol>
                    </section>
                }
            })}
        </section>
    }
}

/// One contribution event's card face, or `None` for any other event.
pub(super) fn face(
    runtime: &AppRuntime,
    publication: &RoomAuthorityStamp,
    event: &RoomEvent,
) -> Option<Face> {
    let RoomEventKind::Contributed {
        lane,
        contribution,
        question,
        text,
        sources,
    } = &event.kind
    else {
        return None;
    };
    Some(entry_face(
        runtime,
        &Contribution {
            publication,
            position: event.position,
            lane: *lane,
            contribution: *contribution,
            question: *question,
            text,
            sources,
        },
    ))
}

/// The badge one contribution kind wears beside its author.
pub(super) const fn badge(kind: RoomContributionKind) -> Badge {
    Badge::Kind {
        kind: kind.as_str(),
        label: kind_label(kind),
    }
}

fn entry_face(runtime: &AppRuntime, entry: &Contribution<'_>) -> Face {
    Face {
        speaker: Some(Speaker {
            name: super::conversation::lane_label(runtime, entry.lane),
            lane: Some(entry.lane),
        }),
        receivers: String::new(),
        badge: Some(badge(entry.contribution)),
        words: entry.text.to_owned(),
        tail: html! {
            <>
                {render_question(runtime, entry)}
                {render_sources(runtime, entry.publication, entry.sources)}
            </>
        },
    }
}

fn render_entry(runtime: &AppRuntime, entry: &Contribution<'_>) -> Html {
    html! {
        <article class="room-card" key={entry.publication.to_string()} tabindex="0">
            {super::card::render(runtime, entry.publication.micros() / 1_000, entry_face(runtime, entry), None)}
        </article>
    }
}

/// The question item a contribution weighs: its text once the resident
/// reads it, its number alone while the read is pending, failed, or the
/// statement it witnessed has been revised; nothing for a question or a
/// retained Design-level post.
fn render_question(runtime: &AppRuntime, entry: &Contribution<'_>) -> Html {
    let Some(item) = entry.question else {
        return Html::default();
    };
    let line = match runtime.contribution_read(entry.publication) {
        Some(ContributionRead::Read {
            question: Some(text),
            ..
        }) => format!("Item {item}: {text}"),
        Some(ContributionRead::Read { question: None, .. }) => {
            format!("Item {item} of an earlier statement")
        }
        _ => format!("Item {item}"),
    };
    html! {
        <p class="room-contribution-question" data-question-item={item.to_string()}>{line}</p>
    }
}

/// How one source currently reads.
#[derive(Clone, Copy)]
enum SourceState<'a> {
    Available,
    Unavailable(&'a wire::rooms::RoomSourceUnavailable),
    /// The resident has not answered yet.
    Resolving,
    /// The resident refused or failed the read.
    Unresolved,
}

fn render_sources(
    runtime: &AppRuntime,
    publication: &RoomAuthorityStamp,
    cited: &[RoomSource],
) -> Html {
    if cited.is_empty() {
        return html! { <p class="room-contribution-sources-none">{"No sources cited."}</p> };
    }
    let (busy, failure, rows) = match runtime.contribution_read(publication) {
        Some(ContributionRead::Read { sources: views, .. }) => (
            false,
            None,
            views
                .iter()
                .map(|view| match view {
                    RoomSourceView::Available { source } => (source, SourceState::Available),
                    RoomSourceView::Unavailable {
                        source,
                        unavailable,
                    } => (source, SourceState::Unavailable(unavailable)),
                })
                .collect::<Vec<_>>(),
        ),
        Some(ContributionRead::Failed(reason)) => (
            false,
            Some(reason.as_str()),
            cited
                .iter()
                .map(|source| (source, SourceState::Unresolved))
                .collect(),
        ),
        Some(ContributionRead::Loading) | None => (
            true,
            None,
            cited
                .iter()
                .map(|source| (source, SourceState::Resolving))
                .collect(),
        ),
    };
    html! {
        <div class="room-contribution-sources">
            if let Some(reason) = failure {
                <p class="room-contribution-sources-failed" role="status">
                    {format!("Sources could not be resolved: {reason}")}
                </p>
            }
            <ul aria-label="Sources" aria-busy={busy.to_string()}>
                {for rows.into_iter().map(|(source, state)| render_source(runtime, source, state))}
            </ul>
        </div>
    }
}

fn render_source(runtime: &AppRuntime, source: &RoomSource, state: SourceState<'_>) -> Html {
    let (state_key, status) = match state {
        SourceState::Available => ("available", "available".to_owned()),
        SourceState::Unavailable(unavailable) => (
            "unavailable",
            format!(
                "unavailable: {}",
                super::conversation::unavailable_reason(unavailable)
            ),
        ),
        SourceState::Resolving => ("resolving", "resolving…".to_owned()),
        SourceState::Unresolved => ("unresolved", "not resolved".to_owned()),
    };
    let (kind, link) = match source {
        RoomSource::Transcript(hunk) => {
            let label = format!(
                "Transcript · {} · member {} offset {} event {} · bytes {}..{}",
                super::conversation::lane_label(runtime, hunk.head.lane),
                hunk.locus.member,
                hunk.locus.offset,
                hunk.locus.event,
                hunk.text.start,
                hunk.text.end,
            );
            let href = matches!(state, SourceState::Available)
                .then(|| super::conversation::transcript_card_href(runtime, hunk))
                .flatten();
            let link = href.map_or_else(
                || html! { <span>{label.clone()}</span> },
                |href| html! { <a href={href}>{label.clone()}</a> },
            );
            ("transcript", link)
        }
        RoomSource::Journal { fact } => (
            "journal",
            html! {
                <CopyButton
                    class="app-compact-control"
                    text={AttrValue::from(fact.to_string())}
                    subject="journal fact"
                    label={AttrValue::from(format!("Copy journal fact {fact}"))}
                >{format!("Journal fact {fact}")}</CopyButton>
            },
        ),
        RoomSource::Artifact { digest } => (
            "artifact",
            html! {
                <CopyButton
                    class="app-compact-control"
                    text={AttrValue::from(digest.to_string())}
                    subject="artifact digest"
                    label={AttrValue::from(format!("Copy artifact digest {digest}"))}
                >{"Artifact "}<code>{digest.to_string()}</code></CopyButton>
            },
        ),
        RoomSource::Contribution { publication, .. } => (
            "contribution",
            html! {
                <CopyButton
                    class="app-compact-control"
                    text={AttrValue::from(serde_json::to_string(source).unwrap_or_default())}
                    subject="contribution source"
                    label={AttrValue::from(format!("Copy contribution source {publication}"))}
                >{"Contribution "}<code>{publication.to_string()}</code></CopyButton>
            },
        ),
    };
    html! {
        <li data-source-kind={kind} data-source-state={state_key}>
            {link}
            <span class="room-contribution-source-state">{status}</span>
        </li>
    }
}
