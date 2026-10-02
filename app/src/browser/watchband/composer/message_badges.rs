use super::*;

/// A card carries FINAL only once its record proves the turn ended. A live
/// prose card raised later gains the badge then; nothing holds its footprint
/// beforehand, so the controls island never widens with an invisible chip.
pub(super) fn render(message: &wire::AgentMessage) -> Html {
    if !message.final_answer {
        return Html::default();
    }
    html! {
        <span class="message-badges" role="list" aria-label="Message classifications">
            {render_badge("FINAL", "final", None)}
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
