//! The Magnitude answer to how much one piece of work cost.
//!
//! Every number here is a number the resident's fold produced. This module
//! chooses rows, columns and words; it never adds, averages, or converts one,
//! because a total computed in the browser would be a second reduction able to
//! disagree with the one `spice journal task-effort` prints from the same
//! bytes. What is left is placement, the exact-tag grouping handle, and the one
//! rule the fold cannot enforce from its side: a phase the fold withheld gets
//! no sizing number at all, structurally, rather than by a per-cell condition
//! that could be relaxed one cell at a time.

use super::*;
use crate::{EffortState, EffortWithheld, Pane, effort_identity_tag, effort_is_zero};
use wire::intent::{
    EffortAggregateAnswer, EffortAnswer, EffortAvailability, EffortIdentity, EffortPhaseAnswer,
    EffortSegmentAnswer, EffortUnavailable, EffortUsage,
};

/// One proven phase beside the three values its `Available` arm carries.
type Proven<'a> = (
    &'a EffortPhaseAnswer,
    &'a EffortIdentity,
    &'a EffortUsage,
    u64,
);

/// One withheld phase beside the reasons the fold gave for withholding it.
type Withheld<'a> = (&'a EffortPhaseAnswer, &'a [EffortUnavailable]);

/// The Effort section, wearing whichever of its five states currently holds.
///
/// `data-effort-state` keeps loading, refusal, fault, empty and answered apart
/// with a distinct word each, so a surface that collapsed two of them into one
/// fails a reader's expectation loudly rather than looking plausible.
pub(in crate::browser) fn render_effort(surface: &SurfaceData) -> Html {
    let surface = surface.read();
    match surface.as_ref().and_then(CanvasRuntime::effort) {
        None => section(
            "loading",
            "awaiting the fold".to_owned(),
            empty("The task-effort fold has not been read yet."),
        ),
        Some(EffortState::Withheld(withheld)) => render_unread(withheld),
        Some(EffortState::Answered(answer)) if answer.phases.is_empty() => section(
            "empty",
            "no phases".to_owned(),
            empty("No claim-to-terminal phase has been folded yet."),
        ),
        Some(EffortState::Answered(answer)) => render_answered(answer),
    }
}

fn section(state: &'static str, count: String, body: Html) -> Html {
    html! {
        <section class="section" data-pane={Pane::Effort.as_str()} data-effort-state={state}>
            <div class="section-heading">
                <h2>{"Task effort"}</h2>
                <span class="section-count">{count}</span>
            </div>
            {body}
        </section>
    }
}

/// The section as it stands when the read itself produced no fold.
///
/// The server's own words are shown rather than a house sentence about them,
/// and no counter appears beside them, because the state this renders is
/// precisely the state in which the Canvas holds no counters.
fn render_unread(withheld: &EffortWithheld) -> Html {
    section(
        withheld.class(),
        withheld.class().to_owned(),
        html! {
            <div class="empty" role="alert" data-effort-reason={withheld.reason().to_owned()}>
                <p>{withheld.reason().to_owned()}</p>
                <p class="muted">{withheld.detail().to_owned()}</p>
            </div>
        },
    )
}

fn render_answered(answer: &EffortAnswer) -> Html {
    let proven = proven(answer);
    let withheld = withheld(answer);
    section(
        "answered",
        format!("{} phases", answer.phases.len()),
        html! {
            <>
                {render_proven(&proven)}
                {render_withheld(&withheld)}
                {render_aggregates(&answer.aggregates)}
            </>
        },
    )
}

/// The phases the fold proved, each with the values its arm carries.
///
/// Reading the arm out once here is what lets the row builder take the three
/// values as arguments instead of re-matching and needing a branch for the case
/// the partition already excluded.
fn proven(answer: &EffortAnswer) -> Vec<Proven<'_>> {
    answer
        .phases
        .iter()
        .filter_map(|phase| match &phase.availability {
            EffortAvailability::Available {
                identity,
                usage,
                wall_ms,
            } => Some((phase, identity, usage, *wall_ms)),
            EffortAvailability::Unavailable { .. } => None,
        })
        .collect()
}

fn withheld(answer: &EffortAnswer) -> Vec<Withheld<'_>> {
    answer
        .phases
        .iter()
        .filter_map(|phase| match &phase.availability {
            EffortAvailability::Available { .. } => None,
            EffortAvailability::Unavailable { reasons } => Some((phase, reasons.as_slice())),
        })
        .collect()
}

fn render_proven(phases: &[Proven<'_>]) -> Html {
    if phases.is_empty() {
        return empty("No phase carries counters the fold could prove.");
    }
    html! {
        <div class="table-wrap"><table data-effort-table="proven">
            <caption class="visually-hidden">
                {"Phases whose claim-to-terminal counters the fold proved"}
            </caption>
            <thead><tr>
                <th>{"Task"}</th>
                <th>{"Phase"}</th>
                <th>{"Launch"}</th>
                <th>{"Occupant / lineage"}</th>
                <th>{"Turns"}</th>
                <th>{"Renewals"}</th>
                <th>{"Wall"}</th>
                <th>{"Input"}</th>
                <th>{"Cached"}</th>
                <th>{"Cache write"}</th>
                <th>{"Output"}</th>
                <th>{"Reasoning"}</th>
            </tr></thead>
            <tbody>
                {for phases.iter().map(|(phase, identity, usage, wall_ms)| {
                    proven_row(phase, identity, usage, *wall_ms)
                })}
            </tbody>
        </table></div>
    }
}

/// One comparable row: the launch that produced it and everything it spent.
///
/// `data-effort-magnitude` separates a phase that genuinely spent nothing from
/// one that is merely small, and `data-effort-continuation` separates a phase
/// still open at the horizon from one a terminal fact closed. Both are states
/// a reader would otherwise have to infer from a row of zeroes or a blank cell.
fn proven_row(
    phase: &EffortPhaseAnswer,
    identity: &EffortIdentity,
    usage: &EffortUsage,
    wall_ms: u64,
) -> Html {
    let magnitude = if effort_is_zero(usage, wall_ms) {
        "zero"
    } else {
        "spent"
    };
    let continuation = if phase.closing.is_some() {
        "closed"
    } else {
        "open"
    };
    html! {
        <tr
            data-effort-availability="available"
            data-effort-identity={effort_identity_tag(identity)}
            data-effort-magnitude={magnitude}
            data-effort-continuation={continuation}
        >
            <td class="mono">{phase.task.to_string()}</td>
            {phase_cell(phase)}
            {launch_cell(identity)}
            {attribution_cell(phase.segments.first())}
            {counter("turns", usage.turns.to_string())}
            {counter("renewals", phase.renewals.to_string())}
            {counter("wall", format!("{wall_ms} ms"))}
            {counter("input", usage.input_tokens.to_string())}
            {counter("cached", usage.cached_input_tokens.to_string())}
            {counter("cache-write", usage.cache_write_input_tokens.to_string())}
            {counter("output", usage.output_tokens.to_string())}
            {counter("reasoning", optional(usage.reasoning_output_tokens))}
        </tr>
    }
}

/// The phases the fold refused to size, with why, and with nothing to read.
///
/// This table has no numeric column at all. Withholding a value per cell would
/// leave the columns standing and make the absence a condition that a later
/// edit could relax one cell at a time; a table without the columns cannot
/// show a token count, a duration, an average, or a total by any route.
fn render_withheld(phases: &[Withheld<'_>]) -> Html {
    if phases.is_empty() {
        return Html::default();
    }
    html! {
        <div class="table-wrap"><table data-effort-table="withheld">
            <caption class="visually-hidden">
                {"Phases the fold could not size, and the reasons it gave"}
            </caption>
            <thead><tr>
                <th>{"Task"}</th>
                <th>{"Phase"}</th>
                <th>{"Launch"}</th>
                <th>{"Withheld because"}</th>
            </tr></thead>
            <tbody>
                {for phases.iter().map(|(phase, reasons)| withheld_row(phase, reasons))}
            </tbody>
        </table></div>
    }
}

fn withheld_row(phase: &EffortPhaseAnswer, reasons: &[EffortUnavailable]) -> Html {
    html! {
        <tr data-effort-availability="unavailable">
            <td class="mono">{phase.task.to_string()}</td>
            {phase_cell(phase)}
            {recorded_launches(&phase.segments)}
            <td>{for reasons.iter().map(|reason| html! {
                <div class="effort-reason">
                    <span class="tag" data-effort-reason={reason.code.as_str()}>
                        {reason.code.as_str()}
                    </span>
                    <span class="muted">{reason.detail.clone()}</span>
                </div>
            })}</td>
        </tr>
    }
}

/// Comparable partitions, each split into the total and the average it carries.
///
/// A partition occupies two rows rather than twenty-odd columns, so a sum can
/// never be read as a mean. Both rows wear the partition's identity tag, which
/// is what a test compares against the phases it claims to cover.
fn render_aggregates(aggregates: &[EffortAggregateAnswer]) -> Html {
    if aggregates.is_empty() {
        return empty("No partition of phases is comparable enough to total.");
    }
    html! {
        <div class="table-wrap"><table data-effort-table="aggregates">
            <caption class="visually-hidden">
                {"Totals and averages, never crossing one exact launch identity"}
            </caption>
            <thead><tr>
                <th>{"Launch"}</th>
                <th>{"Phase"}</th>
                <th>{"Phases"}</th>
                <th>{"Measure"}</th>
                <th>{"Turns"}</th>
                <th>{"Wall"}</th>
                <th>{"Input"}</th>
                <th>{"Cached"}</th>
                <th>{"Cache write"}</th>
                <th>{"Output"}</th>
                <th>{"Reasoning"}</th>
            </tr></thead>
            <tbody>{for aggregates.iter().flat_map(|aggregate| {
                [
                    aggregate_row(aggregate, "total", &aggregate.usage_sum, aggregate.wall_ms_sum),
                    aggregate_row(
                        aggregate,
                        "average",
                        &aggregate.usage_average,
                        aggregate.wall_ms_average,
                    ),
                ]
            })}</tbody>
        </table></div>
    }
}

fn aggregate_row(
    aggregate: &EffortAggregateAnswer,
    measure: &'static str,
    usage: &EffortUsage,
    wall_ms: u64,
) -> Html {
    html! {
        <tr
            data-effort-identity={effort_identity_tag(&aggregate.identity)}
            data-effort-measure={measure}
        >
            {launch_cell(&aggregate.identity)}
            <td><span class="tag">{aggregate.phase.to_string()}</span></td>
            {counter("count", aggregate.count.to_string())}
            <td>{measure}</td>
            {counter("turns", usage.turns.to_string())}
            {counter("wall", format!("{wall_ms} ms"))}
            {counter("input", usage.input_tokens.to_string())}
            {counter("cached", usage.cached_input_tokens.to_string())}
            {counter("cache-write", usage.cache_write_input_tokens.to_string())}
            {counter("output", usage.output_tokens.to_string())}
            {counter("reasoning", optional(usage.reasoning_output_tokens))}
        </tr>
    }
}

/// One cell holding a value the fold sized.
///
/// Every such cell is marked, so "this region shows no magnitude" is a question
/// about which elements exist rather than about which characters appear in the
/// text, and a row that grew a counter it should not have fails on structure.
fn counter(name: &'static str, value: String) -> Html {
    html! { <td class="mono" data-effort-counter={name}>{value}</td> }
}

/// A counter the vendor never reported, said as absence rather than as zero.
fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "unrecorded".to_owned(), |value| value.to_string())
}

fn phase_cell(phase: &EffortPhaseAnswer) -> Html {
    html! {
        <td class="title-cell">
            <div><span class="tag active">{phase.phase.to_string()}</span></div>
            <div class="mono muted">{format!("attempt {}", phase.phase_index)}</div>
        </td>
    }
}

fn launch_cell(identity: &EffortIdentity) -> Html {
    html! {
        <td class="title-cell">
            <div class="mono">{identity.vendor.to_string()}</div>
            <div class="muted">{format!("{} / {}", identity.model, identity.effort)}</div>
        </td>
    }
}

/// The launches a withheld phase's own segments recorded, and nothing more.
///
/// A phase the fold refused to size may still have recorded who ran it, and
/// that context is what makes the reason legible. Where the segments disagree
/// the disagreement is shown, because collapsing two launches into one line
/// would state an identity the fold explicitly declined to state. Repeats are
/// folded away so that agreement reads as one launch rather than as many.
fn recorded_launches(segments: &[EffortSegmentAnswer]) -> Html {
    let mut tags: Vec<String> = Vec::new();
    for segment in segments {
        let tag = segment
            .identity
            .as_ref()
            .map_or_else(|| "unrecorded".to_owned(), effort_identity_tag);
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    if tags.is_empty() {
        tags.push("unrecorded".to_owned());
    }
    html! {
        <td>{for tags.into_iter().map(|tag| html! {
            <div class="mono muted">{tag}</div>
        })}</td>
    }
}

/// Who the fold credited a proven phase to, at the locus it read.
fn attribution_cell(segment: Option<&EffortSegmentAnswer>) -> Html {
    let Some(segment) = segment else {
        return html! { <td class="muted">{"unrecorded"}</td> };
    };
    html! {
        <td class="title-cell">
            <div class="mono">{short(&segment.occupant.to_string())}</div>
            <div class="mono muted">{short(&segment.lineage.to_string())}</div>
        </td>
    }
}
