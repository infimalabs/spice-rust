//! Working-group room publications placed in a packed plane.

use super::CardLayout;
use yew::prelude::*;

/// One room publication placed in a working-group plane. Its design channel
/// shows only through the accent, exactly as a lane's does in a team. A
/// divider spans the plane in a team compaction card's shape; its body
/// supplies that card's rules and label.
pub(super) fn render_room_card(
    accent: usize,
    design: &wire::TaskStamp,
    divider: bool,
    body: &Html,
    key: &str,
    layout: &CardLayout,
) -> Html {
    html! {
        <article
            key={key.to_owned()}
            id={key.to_owned()}
            class={classes!(
                "mosaic-message-card",
                "mosaic-room-card",
                divider.then_some("mosaic-compaction-card"),
                layout.lifecycle.as_str(),
            )}
            tabindex="-1"
            data-card-kind={if divider { "room-departure" } else { "room" }}
            data-mosaic-card-key={key.to_owned()}
            data-layout-phase={if layout.committed { "committed" } else { "measuring" }}
            data-design={design.to_string()}
            data-accent={accent.to_string()}
            data-accent-slot={accent.to_string()}
            data-state={layout.lifecycle.as_str()}
            data-track={layout.track.to_string()}
            data-span={layout.span.to_string()}
            data-base={layout.base.to_string()}
            data-row={layout.row.to_string()}
            data-rows={layout.rows.to_string()}
            data-creation-index={layout.measurement_index.to_string()}
            data-mosaic-measure-index={layout.measurement_index.to_string()}
            data-measurement-reads={layout.measurement_reads.to_string()}
            data-measured-spans={layout.measured_spans.clone()}
            data-measured-widths={layout.measured_widths.clone()}
            data-measured-heights={layout.measured_heights.clone()}
            data-candidate-spans={layout.candidate_spans.clone()}
            data-measure-span={layout.measure_span.to_string()}
        >
            {body.clone()}
        </article>
    }
}
