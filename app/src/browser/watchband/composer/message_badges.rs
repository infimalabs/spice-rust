use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Acknowledgement {
    Pending,
    Ack,
    Nak,
}

impl Acknowledgement {
    fn precedence(self) -> u8 {
        match self {
            Self::Ack => 0,
            Self::Pending => 1,
            Self::Nak => 2,
        }
    }
}

#[derive(Default)]
struct MessageBadges {
    acknowledgement: Option<Acknowledgement>,
    final_answer: bool,
    taste: bool,
    review: bool,
    broadcast: bool,
}

impl MessageBadges {
    fn from_message(message: &wire::AgentMessage) -> Self {
        let mut badges = Self {
            final_answer: message.final_answer,
            ..Self::default()
        };
        for classification in &message.classifications {
            match classification.kind {
                wire::MessageClassificationKind::Acknowledgement
                    if is_top_level(message, classification) =>
                {
                    badges.set_acknowledgement(Acknowledgement::Ack);
                }
                wire::MessageClassificationKind::AcknowledgementPending
                    if is_top_level(message, classification) =>
                {
                    badges.set_acknowledgement(Acknowledgement::Pending);
                }
                wire::MessageClassificationKind::Decline
                    if is_top_level(message, classification) =>
                {
                    badges.set_acknowledgement(Acknowledgement::Nak);
                }
                wire::MessageClassificationKind::Taste => badges.taste = true,
                wire::MessageClassificationKind::Review => badges.review = true,
                wire::MessageClassificationKind::Broadcast => badges.broadcast = true,
                wire::MessageClassificationKind::Acknowledgement
                | wire::MessageClassificationKind::AcknowledgementPending
                | wire::MessageClassificationKind::Decline => {}
            }
        }
        badges
    }

    fn set_acknowledgement(&mut self, acknowledgement: Acknowledgement) {
        if self
            .acknowledgement
            .is_none_or(|current| acknowledgement.precedence() > current.precedence())
        {
            self.acknowledgement = Some(acknowledgement);
        }
    }

    fn is_empty(&self) -> bool {
        self.acknowledgement.is_none()
            && !self.final_answer
            && !self.taste
            && !self.review
            && !self.broadcast
    }
}

fn is_top_level(
    message: &wire::AgentMessage,
    classification: &wire::MessageClassification,
) -> bool {
    let Ok(start) = usize::try_from(classification.start) else {
        return false;
    };
    let Some(prefix) = message.text.get(..start) else {
        return false;
    };
    !prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, line)| line)
        .trim_start()
        .starts_with('>')
}

pub(super) fn render(message: &wire::AgentMessage) -> Html {
    let badges = MessageBadges::from_message(message);
    if badges.is_empty() {
        return Html::default();
    }
    html! {
        <span
            class="message-classification-group"
            role="group"
            aria-label="Message classifications"
        >
            if let Some(acknowledgement) = badges.acknowledgement {
                {render_acknowledgement(acknowledgement)}
            }
            if badges.final_answer {
                {render_final_segment()}
            }
            if badges.taste {
                <span
                    class="message-taste-pill"
                    data-message-classification="taste"
                    aria-label="Taste classification"
                >
                    {"TASTE"}
                </span>
            }
            if badges.review {
                <span
                    class="message-review-pill"
                    data-message-classification="review"
                    aria-label="Review classification"
                >
                    {"REVIEW"}
                </span>
            }
            if badges.broadcast {
                <span
                    class="message-broadcast-pill"
                    data-message-classification="broadcast"
                    aria-label="Broadcast classification"
                >
                    {"BROADCAST"}
                </span>
            }
        </span>
    }
}

pub(super) fn render_final(message: &wire::AgentMessage) -> Html {
    if message.final_answer {
        render_final_segment()
    } else {
        Html::default()
    }
}

fn render_acknowledgement(acknowledgement: Acknowledgement) -> Html {
    let (classification, state, label, text) = match acknowledgement {
        Acknowledgement::Pending => (
            "ack-pending",
            "pending",
            "Acknowledgement awaiting retirement",
            "ACK",
        ),
        Acknowledgement::Ack => ("ack", "acked", "Acknowledgement classification", "ACK"),
        Acknowledgement::Nak => ("nak", "declined", "Declined classification", "NAK"),
    };
    html! {
        <span
            class="message-ack-pill"
            data-message-classification={classification}
            data-acknowledgement={state}
            aria-label={label}
        >
            <span class="message-ack-label">{text}</span>
        </span>
    }
}

fn render_final_segment() -> Html {
    html! {
        <span
            class="message-final-pill"
            data-message-classification="final"
            aria-label="Final answer"
        >
            {"FINAL"}
        </span>
    }
}
