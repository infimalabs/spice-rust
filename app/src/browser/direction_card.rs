//! One operator direction card, shared by team and global Work Directions.
//!
//! Each surface resolves its own read model (the team ledger or the resident
//! history page) into a [`Card`]; this module alone decides the markup, so the
//! two surfaces cannot drift apart.

use std::collections::BTreeSet;
use std::rc::Rc;

use yew::prelude::*;

/// Who authored a direction.
pub(super) enum Author {
    /// The operator, shown by the badge its published identity carries.
    Operator,
    /// A peer lane, by its label and accent slot.
    Lane {
        label: String,
        accent: Option<String>,
    },
}

impl Author {
    fn accent(&self) -> Option<String> {
        match self {
            Self::Operator => None,
            Self::Lane { accent, .. } => accent.clone(),
        }
    }

    fn render(&self) -> Html {
        match self {
            Self::Operator => html! {
                <strong class="team-direction-operator" role="img" aria-label="Operator"></strong>
            },
            Self::Lane { label, .. } => html! { <strong>{label.clone()}</strong> },
        }
    }
}

/// One recipient badge with its independently evolving state.
pub(super) struct Recipient {
    /// Stable key within the card: the lane in a team ledger, the directive
    /// across teams.
    pub key: String,
    pub lane: wire::CanonicalUuid,
    pub label: String,
    pub accent: Option<String>,
    pub state_key: &'static str,
    pub state_label: &'static str,
    /// Exact retained Chat card this recipient's reply resolves to.
    pub target: Option<String>,
    /// Journal position of the recipient's retirement, when history has one.
    pub retirement: Option<wire::Seq>,
}

/// Recipients within one team, or grouped under team labels when the view
/// spans teams.
pub(super) enum Recipients {
    Team(Vec<Recipient>),
    Teams(Vec<(String, Vec<Recipient>)>),
}

impl Recipients {
    fn len(&self) -> usize {
        match self {
            Self::Team(recipients) => recipients.len(),
            Self::Teams(groups) => groups.iter().map(|(_, recipients)| recipients.len()).sum(),
        }
    }
}

pub(super) struct Card<'a> {
    /// Directive or fanout identity keying the card.
    pub identity: wire::CanonicalUuid,
    /// Element id a durable route or a return anchor names.
    pub id: String,
    pub body: &'a str,
    /// False when only a retained excerpt of the direction is available.
    pub complete: bool,
    pub attachments: &'a [wire::body::DirectiveAttachment],
    pub author: Author,
    /// Steering keys whose caused tasks the card offers.
    pub keys: BTreeSet<&'a str>,
    pub recipients: Recipients,
    pub authored_millis: i64,
}

pub(super) struct Controls<'a> {
    pub board: Option<&'a crate::runtime::BoardView>,
    pub select: &'a Callback<wire::TaskStamp>,
    /// Records where a followed badge link leaves this card.
    pub follow: &'a Callback<MouseEvent>,
}

pub(super) fn render(card: &Card<'_>, controls: &Controls<'_>) -> Html {
    let rendered = crate::composer_markdown::render_received_with_classifications(card.body, &[]);
    let diagrams = Rc::new(
        rendered
            .diagrams
            .into_iter()
            .map(AttrValue::from)
            .collect::<Vec<_>>(),
    );
    html! {
        <li
            key={card.identity.to_string()}
            id={card.id.clone()}
            class="team-direction-ledger-row"
            role="article"
            aria-label={format!("Direction to {} recipients", card.recipients.len())}
            tabindex="-1"
            data-card-return-origin="work"
            data-direction={card.identity.to_string()}
            data-accent={card.author.accent()}
        >
            <super::mermaid::MarkdownBody
                html={AttrValue::from(rendered.html)}
                sources={diagrams}
                final_answer={false}
                quote_tasks={Rc::default()}
            />
            {render_attachments(card.attachments)}
            {render_tasks(&card.keys, controls)}
            <footer class="team-direction-footer">
                <span class="team-direction-attribution" data-accent={card.author.accent()}>
                    <span class="agent-presence" aria-hidden="true"></span>
                    {card.author.render()}
                </span>
                if !card.complete {
                    <span class="team-direction-excerpt">{"retained excerpt"}</span>
                }
                {render_recipients(&card.recipients, controls.follow)}
                <super::clock::Age authored={card.authored_millis} class="team-direction-age" />
            </footer>
        </li>
    }
}

fn render_attachments(attachments: &[wire::body::DirectiveAttachment]) -> Html {
    if attachments.is_empty() {
        return Html::default();
    }
    html! {
        <div class="message-attachments" aria-label="Direction attachments">
            {for attachments.iter().map(|attachment| {
                let action = format!("View {} full size", attachment.name);
                html! {
                    <button
                        key={attachment.name.clone()}
                        type="button"
                        class="message-image message-image--openable"
                        title={action.clone()}
                        aria-label={action}
                        data-image-preview=""
                        data-image-name={attachment.name.clone()}
                    >
                        <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                    </button>
                }
            })}
        </div>
    }
}

fn render_tasks(keys: &BTreeSet<&str>, controls: &Controls<'_>) -> Html {
    let handles = controls.board.map_or_else(BTreeSet::new, |board| {
        keys.iter()
            .filter_map(|key| board.quote_tasks.get(*key))
            .flatten()
            .collect::<BTreeSet<_>>()
    });
    let tasks = controls.board.into_iter().flat_map(|board| {
        board.tasks.iter().filter_map(|task| {
            handles
                .contains(&task.statement.body.handle)
                .then_some((&task.statement.body.handle, &task.statement.body.born))
        })
    });
    html! {
        <div class="team-direction-tasks">
            {for tasks.map(|(handle, stamp)| {
                let open = {
                    let select = controls.select.clone();
                    let stamp = stamp.clone();
                    Callback::from(move |_| select.emit(stamp.clone()))
                };
                html! {
                    <button
                        key={handle.to_string()}
                        type="button"
                        class="team-direction-task-chip"
                        data-task-handle={handle.to_string()}
                        aria-label={format!("Open {handle} details")}
                        onclick={open}
                    >
                        <span class="team-direction-task-chip-label">{handle.to_string()}</span>
                    </button>
                }
            })}
        </div>
    }
}

fn render_recipients(recipients: &Recipients, follow: &Callback<MouseEvent>) -> Html {
    match recipients {
        Recipients::Team(recipients) => html! {
            <ul class="team-direction-recipients" aria-label="Recipients">
                {for recipients.iter().map(|recipient| render_recipient(recipient, follow))}
            </ul>
        },
        Recipients::Teams(groups) => html! {
            <div class="team-direction-recipient-groups" aria-label="Recipients grouped by team">
                {for groups.iter().map(|(team, recipients)| html! {
                    <ul class="team-direction-recipients" aria-label={format!("Recipients in {team}")}>
                        {for recipients.iter().map(|recipient| render_recipient(recipient, follow))}
                    </ul>
                })}
            </div>
        },
    }
}

fn render_recipient(recipient: &Recipient, follow: &Callback<MouseEvent>) -> Html {
    let badge = html! {
        <>
            <span class="team-task-member">
                <span class="agent-presence" aria-hidden="true"></span>
                <strong class="team-task-member-name">{recipient.label.clone()}</strong>
            </span>
            <span class="team-direction-state">{recipient.state_label}</span>
        </>
    };
    html! {
        <li
            key={recipient.key.clone()}
            data-agent-lane={recipient.lane.to_string()}
            data-accent={recipient.accent.clone()}
            data-direction-state={recipient.state_key}
            data-direction-retirement-position={recipient
                .retirement
                .map(|position| position.get().to_string())}
        >
            if let Some(target) = recipient.target.as_deref() {
                <a
                    class="team-direction-recipient-link"
                    href={crate::view::card_href(target)}
                    aria-label={format!(
                        "Open {} {} message in Chat",
                        recipient.label,
                        recipient.state_label.to_lowercase(),
                    )}
                    onclick={follow.clone()}
                >{badge}</a>
            } else {
                <span class="team-direction-recipient-label">{badge}</span>
            }
        </li>
    }
}
