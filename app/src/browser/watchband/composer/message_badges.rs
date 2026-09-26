use super::*;

pub(super) fn render(message: &wire::AgentMessage) -> Html {
    if message.final_answer {
        return html! {
            <span class="message-badges" role="list" aria-label="Message classifications">
                {render_badge("FINAL", "final", None)}
            </span>
        };
    }
    if message.activity != wire::AgentActivityKind::Prose {
        return Html::default();
    }
    // A live vendor stream lands its prose before the record that proves the
    // turn ended, so this card may still be raised to FINAL by a later
    // revision. Holding the badge's footprint from the first paint keeps that
    // flip a change of color alone: the footer keeps its columns, the agent
    // identity keeps its place, and the card keeps its measured rows.
    html! {
        <span class="message-badges message-badges--reserved" aria-hidden="true">
            <span class="message-badge message-badge--reserved">
                <span class="message-badge-label">{"FINAL"}</span>
            </span>
        </span>
    }
}

fn render_badge(label: &'static str, class: &'static str, count: Option<u32>) -> Html {
    html! {
        <span
            class={classes!("message-badge", format!("message-badge--{class}"))}
            role="listitem"
            data-message-classification={class}
            aria-label={format!("{label} message classification")}
        >
            <span class="message-badge-label">{label}</span>
            if let Some(count) = count {
                <span class="message-badge-count">{count}</span>
            }
        </span>
    }
}
