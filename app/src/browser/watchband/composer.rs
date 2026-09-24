//! Browser callbacks, activity cards, attachments, and safe Markdown for lane messages.

use web_sys::{ClipboardEvent, DragEvent, File, FileList, FileReader};
use yew::functional::UseReducerDispatcher;

use super::*;

pub(super) fn update_body(
    lane_key: String,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
        drafts.dispatch(ComposerAction::SetBody {
            lane: lane_key.clone(),
            body: input.value(),
        });
    })
}

pub(super) fn keyboard_submit(
    lane: CanonicalUuid,
    target: Option<SubmissionTarget>,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
    submissions: SubmissionHandle,
    broadcast_targets: Vec<BroadcastTarget>,
) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            if event.shift_key() {
                if event.repeat() {
                    return;
                }
                let source_is_primary = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .is_some_and(|target| target.has_attribute("data-primary-composer-target-id"));
                if source_is_primary
                    && submit_broadcast(
                        lane,
                        &lane_key,
                        &broadcast_targets,
                        &sender,
                        &drafts,
                        &submissions,
                    )
                {
                    focus_after_render(primary_textarea_id(&lane_key));
                }
                return;
            }
            let Ok(fanout) = random_uuid() else {
                return;
            };
            if submit_draft(
                fanout,
                lane,
                target,
                &lane_key,
                &sender,
                &drafts,
                &submissions,
            ) {
                focus_after_render(primary_textarea_id(&lane_key));
            }
        }
    })
}

pub(super) fn primary_textarea_id(lane_key: &str) -> String {
    format!("composer-primary-{lane_key}")
}

fn quote_textarea_id(lane_key: &str, identity: &str) -> String {
    format!("composer-quote-{lane_key}-{identity}")
}

fn agent_message_quote_identity(message: &wire::AgentMessage) -> String {
    format!(
        "agent-{}-{}-{}-{}-{}",
        message.lane,
        message.locus.member,
        message.locus.offset,
        message.locus.length,
        message.locus.event,
    )
}

pub(super) fn agent_message_source_id(message: &wire::AgentMessage) -> String {
    format!("message-{}", agent_message_quote_identity(message))
}

pub(super) fn focus_after_render(id: String) {
    spawn_local(async move {
        TimeoutFuture::new(0).await;
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(textarea) = document
            .get_element_by_id(&id)
            .and_then(|element| element.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        else {
            return;
        };
        let _ = textarea.focus();
    });
}

#[derive(Properties)]
pub(super) struct AgentMessageCardProps {
    pub(super) accent: usize,
    pub(super) member: crate::runtime::LaneMember,
    pub(super) quote_lane: CanonicalUuid,
    pub(super) message: Rc<wire::AgentMessage>,
    pub(super) quote_dispatcher: UseReducerDispatcher<ComposerDrafts>,
    pub(super) speech_queue: Callback<SpeechRequest>,
    pub(super) speech: TeamSpeech,
    pub(super) speaking: bool,
    pub(super) lane: CanonicalUuid,
    pub(super) order: u64,
    pub(super) layout: CardLayout,
}

impl PartialEq for AgentMessageCardProps {
    fn eq(&self, other: &Self) -> bool {
        self.accent == other.accent
            && self.member.lane == other.member.lane
            && self.member.name == other.member.name
            && self.member.worktree == other.member.worktree
            && self.quote_lane == other.quote_lane
            && (Rc::ptr_eq(&self.message, &other.message) || self.message == other.message)
            && self.quote_dispatcher == other.quote_dispatcher
            && self.speech_queue == other.speech_queue
            && self.speech == other.speech
            && self.speaking == other.speaking
            && self.lane == other.lane
            && self.order == other.order
            && self.layout == other.layout
    }
}

/// Keep stable message DOM and parsed Markdown out of unrelated surface updates.
#[function_component(AgentMessageCard)]
pub(super) fn agent_message_card(props: &AgentMessageCardProps) -> Html {
    let lane = props.lane.to_string();
    let message_key = agent_message_source_id(&props.message);
    let speech_queue = props.speech_queue.clone();
    let response = props
        .message
        .automatic_speech
        .as_ref()
        .and_then(|speech| crate::narration::prepared_utterance(&speech.text))
        .or_else(|| crate::narration::card_utterance(props.message.activity, &props.message.text));
    let speak = Callback::from(move |_: String| {
        if let Some(text) = response.clone() {
            speech_queue.emit(SpeechRequest::message(
                lane.clone(),
                message_key.clone(),
                text,
            ));
        }
    });
    let correspondent = agent_message_correspondent(
        props.accent,
        props.quote_lane,
        &props.message,
        &props.quote_dispatcher,
        &speak,
        props.speaking,
        props.order,
    );
    render_agent_message_card(
        &props.member,
        props.quote_lane,
        &props.message,
        &correspondent,
        &props.layout,
    )
}

fn render_agent_message_card(
    member: &crate::runtime::LaneMember,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    correspondent: &view::chrome::Correspondent,
    layout: &CardLayout,
) -> Html {
    let AgentMessagePresentation {
        source_id,
        attribution,
        footer_label,
        is_boundary,
        shown_text,
        rendered_html,
        unlinked_media,
        media_description,
        image_only,
        card_label,
        badges,
    } = agent_message_presentation(member, message);
    html! {
        <article
            key={source_id.clone()}
            id={source_id.clone()}
            class={classes!(
                "mosaic-message-card",
                is_boundary.then_some("mosaic-compaction-card"),
                (!is_boundary && image_only).then_some("mosaic-message-card--image-only"),
                message.final_answer.then_some("final"),
                correspondent.speaking.then_some("now-playing"),
                layout.lifecycle.as_str(),
            )}
            aria-label={card_label}
            data-card-kind={if is_boundary { "compaction" } else { "agent-message" }}
            data-message-role="agent"
            data-mosaic-card-key={source_id}
            data-layout-phase={if layout.committed { "committed" } else { "measuring" }}
            data-agent-lane={member.lane.to_string()}
            data-agent-activity={watchband::agent_activity_name(message.activity)}
            data-message-ack-count={badges.acknowledgements.to_string()}
            data-message-taste-count={badges.tastes.to_string()}
            data-message-broadcast-count={badges.broadcasts.to_string()}
            data-message-review-count={badges.reviews.to_string()}
            data-message-pending-count={badges.pending.to_string()}
            data-message-final={message.final_answer.to_string()}
            data-speech-active={correspondent.speaking.to_string()}
            data-producer-target-id={quote_lane.to_string()}
            data-accent={correspondent.accent.to_string()}
            data-accent-slot={correspondent.accent.to_string()}
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
            if is_boundary {
                {render_agent_boundary(message, &footer_label)}
            } else {
                {rendered_html}
                {render_agent_message_media(unlinked_media, media_description.as_deref())}
                {render_agent_message_footer(
                    message,
                    &shown_text,
                    correspondent,
                    crate::runtime::agent_message_display_millis(message),
                    &footer_label,
                    &attribution,
                )}
            }
        </article>
    }
}

struct AgentMessagePresentation<'a> {
    source_id: String,
    attribution: String,
    footer_label: String,
    is_boundary: bool,
    /// The text the card shows; a tool card shows none.
    shown_text: std::borrow::Cow<'a, str>,
    rendered_html: Html,
    unlinked_media: Vec<&'a wire::Media>,
    /// What a tool card says about its images in place of its preview line.
    media_description: Option<String>,
    image_only: bool,
    card_label: String,
    badges: wire::AgentMessageBadges,
}

fn agent_message_presentation<'a>(
    member: &crate::runtime::LaneMember,
    message: &'a wire::AgentMessage,
) -> AgentMessagePresentation<'a> {
    let attribution = watchband::agent_message_attribution(
        message.author,
        member.lane,
        &member.worktree,
        member.name.as_deref(),
    );
    let footer_label =
        watchband::agent_message_label(&member.worktree, member.name.as_deref()).to_owned();
    let is_boundary = message.activity.is_boundary();
    let badges = message.badges;
    // A tool card is its image. The resident's preview line stays on the wire
    // for the ticker and the hook, but here it becomes the image's description
    // rather than visible words, and the resident withholds it altogether when
    // a result carried only images.
    let is_tool = message.activity == wire::AgentActivityKind::Tool;
    let media_description = (is_tool && !message.text.trim().is_empty())
        .then(|| watchband::tool_media_description(&message.text));
    let shown_text = if is_tool { "" } else { message.text.as_str() };
    let shown_text = wire::media::with_media_payloads(shown_text, &message.media)
        .expect("message media passed admission before browser rendering");
    let rendered_markdown =
        crate::composer_markdown::render_received_with_badges(&shown_text, badges);
    let card_label = match message.activity {
        wire::AgentActivityKind::Turn => unreachable!("turn events are not message cards"),
        wire::AgentActivityKind::Compaction => format!("Context compacted by {footer_label}"),
        wire::AgentActivityKind::Tool => media_description.as_ref().map_or_else(
            || format!("Tool result from {attribution}"),
            |description| format!("Tool result from {attribution}: {description}"),
        ),
        wire::AgentActivityKind::Reasoning => format!("Thinking from {attribution}"),
        wire::AgentActivityKind::Prose => format!("Agent message from {attribution}"),
    };
    let unlinked_media = message
        .media
        .iter()
        .filter(|media| {
            !media
                .is_embedded_in(&shown_text)
                .expect("message media passed admission before browser rendering")
        })
        .collect::<Vec<_>>();
    let image_only = rendered_markdown.image_only
        || (!unlinked_media.is_empty() && shown_text.trim().is_empty());
    AgentMessagePresentation {
        source_id: agent_message_source_id(message),
        attribution,
        footer_label,
        is_boundary,
        shown_text,
        rendered_html: html! {
            <super::super::mermaid::MarkdownBody
                html={AttrValue::from(rendered_markdown.html)}
                sources={Rc::new(rendered_markdown.diagrams.into_iter().map(AttrValue::from).collect::<Vec<_>>())}
                acknowledgements={Rc::new(rendered_markdown.acknowledgements)}
                final_answer={message.final_answer}
            />
        },
        unlinked_media,
        media_description,
        image_only,
        card_label,
        badges,
    }
}

fn render_agent_boundary(message: &wire::AgentMessage, attribution: &str) -> Html {
    let attribution = attribution.to_owned();
    let display_millis = crate::runtime::agent_message_display_millis(message);
    html! {
        <>
            <span class="mosaic-compaction-rule" aria-hidden="true"></span>
            <span class="mosaic-compaction-label">
                <Age authored={display_millis} class="mosaic-compaction-age" />
                <span class="mosaic-compaction-context">
                    <span class="mosaic-compaction-agent" title={attribution.clone()}>{attribution}</span>
                    <span class="message-key">{"compacted context"}</span>
                </span>
            </span>
            <span class="mosaic-compaction-rule" aria-hidden="true"></span>
        </>
    }
}

fn agent_message_correspondent(
    accent: usize,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    quote_dispatcher: &UseReducerDispatcher<ComposerDrafts>,
    speak: &Callback<String>,
    speaking: bool,
    order: u64,
) -> view::chrome::Correspondent {
    let lane_key = quote_lane.to_string();
    let quote_text = message.text.clone();
    let quote_preview = watchband::first_nonquoted_prose(&message.text);
    let quote_identity = agent_message_quote_identity(message);
    let display_millis = crate::runtime::agent_message_display_millis(message);
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.clone();
    let quote_dispatcher = quote_dispatcher.clone();
    view::chrome::Correspondent {
        accent,
        speak: speak.clone(),
        speaking,
        quote: Callback::from(move |()| {
            quote_dispatcher.dispatch(ComposerAction::AddQuote {
                lane: lane_key.clone(),
                identity: quote_identity.clone(),
                order,
                display_millis,
                quote: quote_text.clone(),
                preview: quote_preview.clone(),
            });
            focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
        }),
    }
}

fn render_agent_message_media(
    unlinked_media: Vec<&wire::Media>,
    description: Option<&str>,
) -> Html {
    if unlinked_media.is_empty() {
        return Html::default();
    }
    html! {
        <div class="message-attachments">
            {for unlinked_media.into_iter().map(|media| {
                let description = description.unwrap_or(&media.name).to_owned();
                let action = format!("Open {description} full size");
                html! {
                    <button
                        type="button"
                        class="message-image message-image--openable"
                        title={action.clone()}
                        aria-label={action}
                        data-image-preview=""
                        data-image-name={media.name.clone()}
                    >
                        <img
                            src={media.data_url.clone()}
                            alt={description}
                            data-media-identity={media.identity().expect("admitted message media has identity").to_string()}
                            data-mosaic-image-state="pending"
                        />
                        <span
                            class="message-image-status"
                            role="status"
                            data-mosaic-image-status="pending"
                        >
                            {"Loading image"}
                        </span>
                    </button>
                }
            })}
        </div>
    }
}

fn render_agent_message_footer(
    message: &wire::AgentMessage,
    shown_text: &str,
    correspondent: &view::chrome::Correspondent,
    display_millis: i64,
    producer_label: &str,
    attribution: &str,
) -> Html {
    let locus = format!(
        "tape {}:{}+{}#{}",
        message.locus.member, message.locus.offset, message.locus.length, message.locus.event,
    );
    let has_actions = !shown_text.trim().is_empty();
    let producer_label = producer_label.to_owned();
    let attribution = attribution.to_owned();
    let author = message.author.to_string();
    let author_title = format!("Copy agent id\n{author}");
    let copy_author = Callback::from(move |_| view::chrome::copy_text(&author));
    let producer_identity = html! {
        <span class="message-footer-identity">
            <button
                class="message-agent-name"
                type="button"
                title={author_title}
                aria-label={format!("Copy agent id for {attribution}")}
                onclick={copy_author}
            ><span class="message-agent-label">{producer_label}</span></button>
        </span>
    };
    html! {
        <footer class="message-footer">
            <div class="message-footer-left">
                <Age authored={display_millis} class="message-age" title={locus} />
                {render_agent_message_badges(message)}
            </div>
            <div class="message-footer-right">
                {producer_identity}
                if has_actions {
                    {view::chrome::render_card_actions(shown_text, correspondent)}
                }
            </div>
        </footer>
    }
}

fn render_agent_message_badges(message: &wire::AgentMessage) -> Html {
    if message.final_answer {
        return html! {
            <span class="message-badges" role="list" aria-label="Message classifications">
                {render_agent_message_badge("FINAL", "final", None)}
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

fn render_agent_message_badge(
    label: &'static str,
    class: &'static str,
    count: Option<u32>,
) -> Html {
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

pub(super) fn render_quote_shards(
    lane_key: &str,
    draft: &ComposerDraft,
    status: crate::runtime::LaneStatus,
    drafts: &UseReducerHandle<ComposerDrafts>,
    target_submit: Callback<KeyboardEvent>,
    placeholder: &str,
) -> Html {
    html! {
        <div class="composer-quote-stack" data-composer-quote-stack-target-id={lane_key.to_owned()}>
            {for draft.quotes.iter().map(|quote| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove_identity = quote.identity.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveQuote {
                        lane: remove_key.clone(),
                        identity: remove_identity.clone(),
                    });
                });
                let update_key = lane_key.to_owned();
                let update_drafts = drafts.clone();
                let update_identity = quote.identity.clone();
                let update = Callback::from(move |event: InputEvent| {
                    let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
                    update_drafts.dispatch(ComposerAction::SetQuoteBody {
                        lane: update_key.clone(),
                        identity: update_identity.clone(),
                        body: input.value(),
                    });
                });
                let shown = watchband::quote_label(quote.preview.as_deref());
                let textarea_id = quote_textarea_id(lane_key, &quote.identity);
                let source_id = format!("message-{}", quote.identity);
                let onpaste = paste_images(
                    lane_key.to_owned(),
                    Some(quote.identity.clone()),
                    drafts.clone(),
                );
                html! {
                    <article
                        key={quote.identity.clone()}
                        class="mosaic-composer composer-shard composer-shard--quote"
                        title={quote.quote.clone()}
                        data-shard-kind="quote"
                        data-shard-target-id={lane_key.to_owned()}
                        data-quote-identity={quote.identity.clone()}
                        data-quote-target-id={lane_key.to_owned()}
                    >
                        <header class="composer-shard-heading">
                            <a
                                class={classes!(
                                    "composer-shard-link",
                                    "composer-shard-age",
                                    status.key(),
                                )}
                                href={format!("#{source_id}")}
                                aria-label={format!("Jump to quoted message: {shown}")}
                                data-lane-status={status.key()}
                            >{lease_age(
                                Some(quote.display_millis),
                                "composer-shard-age-label",
                                "—",
                            )}</a>
                            <strong class="composer-shard-prose" title={shown.to_owned()}>{shown}</strong>
                            {render_image_indicators(
                                lane_key,
                                &quote.attachments,
                                Some(&quote.identity),
                                drafts,
                            )}
                            <button
                                type="button"
                                aria-label="Remove quoted composer shard"
                                onclick={remove}
                            >{"×"}</button>
                        </header>
                        <textarea
                            id={textarea_id}
                            aria-label="Reply to quoted message"
                            data-quote-draft-id={quote.identity.clone()}
                            rows="2"
                            value={quote.body.clone()}
                            placeholder={placeholder.to_owned()}
                            oninput={update}
                            onkeydown={target_submit.clone()}
                            {onpaste}
                        ></textarea>
                        if let Some(issue) = &quote.issue {
                            <p class="composer-issue" role="status">{issue.clone()}</p>
                        }
                    </article>
                }
            })}
        </div>
    }
}

/// Draft images ride in the shard heading as compact thumbnail chips. Removal is
/// an overlaid control, matching the predecessor: hovering the image describes
/// the attachment itself instead of raising a wide native "Remove <name>"
/// tooltip across the header. The strip is rendered even with no images so the
/// heading keeps its fixed child order and its grid columns, which holds that
/// final control still as the first image arrives.
pub(super) fn render_image_indicators(
    lane_key: &str,
    attachments: &[DirectiveAttachment],
    quote_identity: Option<&str>,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> Html {
    html! {
        <span class="composer-shard-image-indicators">
            {for attachments.iter().enumerate().map(|(index, attachment)| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove_quote = quote_identity.map(str::to_owned);
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveAttachment {
                        lane: remove_key.clone(),
                        quote: remove_quote.clone(),
                        index,
                    });
                });
                html! {
                    <span
                        class="composer-shard-image-indicator"
                        title={attachment.name.clone()}
                    >
                        <button type="button" class="composer-shard-image-open" data-image-preview="" data-image-name={attachment.name.clone()} aria-label={format!("Open {} full size", attachment.name)}>
                            <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                        </button>
                        <button
                            type="button"
                            class="composer-shard-image-remove"
                            title="Remove image"
                            aria-label={format!("Remove image {}", attachment.name)}
                            onclick={remove}
                        >{"×"}</button>
                    </span>
                }
            })}
        </span>
    }
}

pub(super) fn paste_images(
    lane_key: String,
    quote_identity: Option<String>,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let clipboard: ClipboardEvent = event.clone().unchecked_into();
        let Some(transfer) = clipboard.clipboard_data() else {
            return;
        };
        let Some(files) = transfer.files() else {
            return;
        };
        if has_image(&files) {
            clipboard.prevent_default();
            add_image_files(&drafts, &lane_key, quote_identity.as_deref(), &files);
        }
    })
}

pub(super) fn drop_on_lane(
    lane: CanonicalUuid,
    target: Option<SubmissionTarget>,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<DragEvent> {
    Callback::from(move |event: DragEvent| {
        event.prevent_default();
        let Some(transfer) = event.data_transfer() else {
            return;
        };
        if let Some(files) = transfer.files()
            && has_image(&files)
        {
            add_image_files(&drafts, &lane_key, None, &files);
            return;
        }
        let Ok(text) = transfer.get_data("text/plain") else {
            return;
        };
        let Some(SubmissionTarget::Resident { occupant, epoch }) = target else {
            return;
        };
        let message = crate::composer_markdown::block_quote(&text);
        if message.is_empty() {
            return;
        }
        let (Some(id), Some(fanout)) = (action_identity(&sender), action_identity(&sender)) else {
            return;
        };
        if !admit_action(
            &sender,
            ClientAction::Steer {
                id,
                fanout,
                lane,
                occupant,
                epoch,
                message,
                attachments: Vec::new(),
            },
        ) {
            transfer.set_drop_effect("none");
        }
    })
}

fn has_image(files: &FileList) -> bool {
    (0..files.length()).any(|index| {
        files
            .get(index)
            .is_some_and(|file| file.type_().starts_with("image/"))
    })
}

fn add_image_files(
    drafts: &UseReducerHandle<ComposerDrafts>,
    lane_key: &str,
    quote_identity: Option<&str>,
    files: &FileList,
) {
    for index in 0..files.length() {
        let Some(file) = files.get(index) else {
            continue;
        };
        if !file.type_().starts_with("image/") {
            continue;
        }
        if file.size() > MAX_ATTACHMENT_BYTES {
            drafts.dispatch(ComposerAction::SetIssue {
                lane: lane_key.to_owned(),
                quote: quote_identity.map(str::to_owned),
                issue: "Image is over 8 MB.".to_owned(),
            });
            continue;
        }
        read_image(
            drafts.clone(),
            lane_key.to_owned(),
            quote_identity.map(str::to_owned),
            file,
        );
    }
}

fn read_image(
    drafts: UseReducerHandle<ComposerDrafts>,
    lane_key: String,
    quote_identity: Option<String>,
    file: File,
) {
    let Ok(reader) = FileReader::new() else {
        drafts.dispatch(ComposerAction::SetIssue {
            lane: lane_key,
            quote: quote_identity,
            issue: "Image reader is unavailable.".to_owned(),
        });
        return;
    };
    let result_reader = reader.clone();
    let name = if file.name().is_empty() {
        "pasted-image.png".to_owned()
    } else {
        file.name()
    };
    let content_type = if file.type_().is_empty() {
        "image/png".to_owned()
    } else {
        file.type_()
    };
    let onload = Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |_| {
        let Ok(value) = result_reader.result() else {
            return;
        };
        let Some(data_url) = value.as_string() else {
            return;
        };
        match DirectiveAttachment::admit(name.clone(), content_type.clone(), data_url) {
            Ok(attachment) => drafts.dispatch(ComposerAction::AddAttachment {
                lane: lane_key.clone(),
                quote: quote_identity.clone(),
                attachment,
            }),
            Err(error) => drafts.dispatch(ComposerAction::SetIssue {
                lane: lane_key.clone(),
                quote: quote_identity.clone(),
                issue: format!("Image was refused: {error}."),
            }),
        }
    });
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    if reader.read_as_data_url(&file).is_ok() {
        onload.forget();
    }
}
struct ComposerPointerState {
    pointer_id: i32,
    start_x: i32,
    start_y: i32,
    dragging: bool,
}

fn composer_document() -> Option<web_sys::Document> {
    web_sys::window()?.document()
}

fn composer_drag_element(
    document: &web_sys::Document,
    lane: CanonicalUuid,
) -> Option<web_sys::Element> {
    document
        .query_selector(&format!("[data-agent-lane=\"{lane}\"]"))
        .ok()
        .flatten()
}

fn clear_composer_swap_preview(document: &web_sys::Document) {
    let Ok(nodes) = document.query_selector_all("[data-composer-drag-left]") else {
        return;
    };
    for index in 0..nodes.length() {
        let Some(element) = nodes
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let _ = element.remove_attribute("data-composer-drag-target");
        if let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() {
            let _ = element.style().remove_property("transform");
        }
    }
}

fn clear_composer_drag_presentation() {
    let Some(document) = composer_document() else {
        return;
    };
    clear_composer_swap_preview(&document);
    if let Ok(nodes) = document.query_selector_all("[data-composer-drag-left]") {
        for index in 0..nodes.length() {
            let Some(element) = nodes
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            else {
                continue;
            };
            let _ = element.remove_attribute("data-composer-drag-left");
            let _ = element.remove_attribute("data-composer-drag-right");
            let _ = element.remove_attribute("data-composer-drag-top");
            let _ = element.remove_attribute("data-composer-drag-bottom");
            let _ = element.remove_attribute("data-composer-drag-source");
        }
    }
    if let Ok(nodes) = document.query_selector_all(".composer-drag-ghost") {
        for index in 0..nodes.length() {
            if let Some(element) = nodes
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            {
                element.remove();
            }
        }
    }
}

fn update_composer_drag_ghost(client_x: i32, client_y: i32) {
    let Some(ghost) = composer_document()
        .and_then(|document| {
            document
                .query_selector(".composer-drag-ghost")
                .ok()
                .flatten()
        })
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let offset_x = ghost
        .get_attribute("data-composer-drag-offset-x")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_default();
    let offset_y = ghost
        .get_attribute("data-composer-drag-offset-y")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or_default();
    let _ = ghost
        .style()
        .set_property("left", &format!("{}px", f64::from(client_x) - offset_x));
    let _ = ghost
        .style()
        .set_property("top", &format!("{}px", f64::from(client_y) - offset_y));
}

fn begin_composer_drag_presentation(
    lane: CanonicalUuid,
    client_x: i32,
    client_y: i32,
) -> Option<()> {
    clear_composer_drag_presentation();
    let document = composer_document()?;
    let source = composer_drag_element(&document, lane)?;
    let rect = source.get_bounding_client_rect();
    let siblings = source
        .parent_element()?
        .query_selector_all(":scope > .team-agent")
        .ok()?;
    for index in 0..siblings.length() {
        let Some(element) = siblings
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let rect = element.get_bounding_client_rect();
        for (attribute, value) in [
            ("data-composer-drag-left", rect.left()),
            ("data-composer-drag-right", rect.right()),
            ("data-composer-drag-top", rect.top()),
            ("data-composer-drag-bottom", rect.bottom()),
        ] {
            let _ = element.set_attribute(attribute, &value.to_string());
        }
    }
    source
        .set_attribute("data-composer-drag-source", "true")
        .ok()?;

    let ghost = source
        .clone_node_with_deep(true)
        .ok()?
        .dyn_into::<web_sys::HtmlElement>()
        .ok()?;
    ghost.set_class_name(&format!("{} composer-drag-ghost", ghost.class_name()));
    for attribute in [
        "data-composer-drag-source",
        "data-composer-drag-target",
        "data-composer-drag-left",
        "data-composer-drag-right",
        "data-composer-drag-top",
        "data-composer-drag-bottom",
    ] {
        let _ = ghost.remove_attribute(attribute);
    }
    let _ = ghost.set_attribute("aria-hidden", "true");
    let _ = ghost.set_attribute("inert", "");
    let _ = ghost.set_attribute(
        "data-composer-drag-offset-x",
        &(f64::from(client_x) - rect.left()).to_string(),
    );
    let _ = ghost.set_attribute(
        "data-composer-drag-offset-y",
        &(f64::from(client_y) - rect.top()).to_string(),
    );
    let _ = ghost
        .style()
        .set_property("width", &format!("{}px", rect.width()));
    let _ = ghost
        .style()
        .set_property("height", &format!("{}px", rect.height()));
    if let Ok(ids) = ghost.query_selector_all("[id]") {
        for index in 0..ids.length() {
            if let Some(element) = ids
                .item(index)
                .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
            {
                let _ = element.remove_attribute("id");
            }
        }
    }
    document.body()?.append_child(&ghost).ok()?;
    update_composer_drag_ghost(client_x, client_y);
    Some(())
}

fn preview_composer_swap(source: CanonicalUuid, target: CanonicalUuid) {
    let Some(document) = composer_document() else {
        return;
    };
    clear_composer_swap_preview(&document);
    if source == target {
        return;
    }
    let Some(source) = composer_drag_element(&document, source)
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let Some(target) = composer_drag_element(&document, target)
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    let source_left = source
        .get_attribute("data-composer-drag-left")
        .and_then(|value| value.parse::<f64>().ok());
    let target_left = target
        .get_attribute("data-composer-drag-left")
        .and_then(|value| value.parse::<f64>().ok());
    let (Some(source_left), Some(target_left)) = (source_left, target_left) else {
        return;
    };
    let offset = target_left - source_left;
    let _ = source
        .style()
        .set_property("transform", &format!("translateX({offset}px)"));
    let _ = target
        .style()
        .set_property("transform", &format!("translateX({}px)", -offset));
    let _ = target.set_attribute("data-composer-drag-target", "true");
}

fn composer_at_point(client_x: i32, client_y: i32) -> Option<CanonicalUuid> {
    let document = composer_document()?;
    let nodes = document
        .query_selector_all("[data-composer-drag-left]")
        .ok()?;
    let x = f64::from(client_x);
    let y = f64::from(client_y);
    for index in 0..nodes.length() {
        let Some(element) = nodes
            .item(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let boundary = |attribute| {
            element
                .get_attribute(attribute)
                .and_then(|value| value.parse::<f64>().ok())
        };
        if boundary("data-composer-drag-left").is_some_and(|left| x >= left)
            && boundary("data-composer-drag-right").is_some_and(|right| x <= right)
            && boundary("data-composer-drag-top").is_some_and(|top| y >= top)
            && boundary("data-composer-drag-bottom").is_some_and(|bottom| y <= bottom)
        {
            return element
                .get_attribute("data-agent-lane")
                .and_then(|lane| lane.parse().ok());
        }
    }
    None
}

fn composer_swap_target() -> Option<CanonicalUuid> {
    composer_document()?
        .query_selector("[data-composer-drag-target]")
        .ok()
        .flatten()?
        .get_attribute("data-agent-lane")?
        .parse()
        .ok()
}

fn release_composer_pointer(event: &PointerEvent) {
    if let Some(element) = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
    {
        let _ = element.release_pointer_capture(event.pointer_id());
    }
}

type ComposerPointerStateHandle = std::rc::Rc<std::cell::RefCell<Option<ComposerPointerState>>>;

fn composer_drag_start(
    enabled: bool,
    state: &ComposerPointerStateHandle,
) -> Callback<PointerEvent> {
    let state = std::rc::Rc::clone(state);
    Callback::from(move |event: PointerEvent| {
        if !enabled || event.button() != 0 || state.borrow().is_some() {
            return;
        }
        let Some(element) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if element.set_pointer_capture(event.pointer_id()).is_err() {
            return;
        }
        clear_composer_drag_presentation();
        state.replace(Some(ComposerPointerState {
            pointer_id: event.pointer_id(),
            start_x: event.client_x(),
            start_y: event.client_y(),
            dragging: false,
        }));
        event.prevent_default();
        event.stop_propagation();
    })
}

fn composer_drag_movement(
    lane: CanonicalUuid,
    members: Vec<CanonicalUuid>,
    state: &ComposerPointerStateHandle,
) -> Callback<PointerEvent> {
    let state = std::rc::Rc::clone(state);
    Callback::from(move |event: PointerEvent| {
        let mut state = state.borrow_mut();
        let Some(active) = state
            .as_mut()
            .filter(|active| active.pointer_id == event.pointer_id())
        else {
            return;
        };
        if !active.dragging
            && event.client_x().abs_diff(active.start_x) < 6
            && event.client_y().abs_diff(active.start_y) < 6
        {
            return;
        }
        if !active.dragging {
            if begin_composer_drag_presentation(lane, event.client_x(), event.client_y()).is_none()
            {
                *state = None;
                return;
            }
            active.dragging = true;
        }
        drop(state);
        update_composer_drag_ghost(event.client_x(), event.client_y());
        if let Some(target) =
            composer_at_point(event.client_x(), event.client_y()).filter(|id| members.contains(id))
        {
            preview_composer_swap(lane, target);
        } else if let Some(document) = composer_document() {
            clear_composer_swap_preview(&document);
        }
        event.prevent_default();
        event.stop_propagation();
    })
}

fn composer_drag_finish(
    lane: CanonicalUuid,
    topology_revision: u64,
    topology: Vec<(CanonicalUuid, Vec<CanonicalUuid>)>,
    sender: SenderHandle,
    state: &ComposerPointerStateHandle,
) -> Callback<PointerEvent> {
    let state = std::rc::Rc::clone(state);
    Callback::from(move |event: PointerEvent| {
        let active = {
            let mut state = state.borrow_mut();
            if !state
                .as_ref()
                .is_some_and(|active| active.pointer_id == event.pointer_id())
            {
                return;
            }
            state.take().expect("the matching pointer state exists")
        };
        release_composer_pointer(&event);
        let arranged = active
            .dragging
            .then(composer_swap_target)
            .flatten()
            .and_then(|target| watchband::swap_members(&topology, lane, target));
        clear_composer_drag_presentation();
        let Some(teams) = arranged else {
            return;
        };
        let _ = admit_action(
            &sender,
            ClientAction::ArrangeTeams {
                topology_revision,
                teams: teams
                    .into_iter()
                    .map(|(team, lanes)| TeamArrangement { team, lanes })
                    .collect(),
            },
        );
        event.prevent_default();
        event.stop_propagation();
    })
}

fn composer_drag_cancel(state: ComposerPointerStateHandle) -> Callback<PointerEvent> {
    Callback::from(move |event: PointerEvent| {
        let active = {
            let mut state = state.borrow_mut();
            if !state
                .as_ref()
                .is_some_and(|active| active.pointer_id == event.pointer_id())
            {
                return;
            }
            state.take()
        };
        if active.is_some() {
            release_composer_pointer(&event);
            clear_composer_drag_presentation();
        }
    })
}

pub(super) fn primary_composer_dragging(
    lane: CanonicalUuid,
    enabled: bool,
    topology_revision: u64,
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    sender: &SenderHandle,
) -> PrimaryComposerDragging {
    let state = std::rc::Rc::new(std::cell::RefCell::new(None));
    let members = topology
        .iter()
        .find_map(|(_, members)| members.contains(&lane).then(|| members.clone()))
        .unwrap_or_default();
    PrimaryComposerDragging {
        enabled,
        start: composer_drag_start(enabled, &state),
        movement: composer_drag_movement(lane, members, &state),
        finish: composer_drag_finish(
            lane,
            topology_revision,
            topology.to_vec(),
            sender.clone(),
            &state,
        ),
        cancel: composer_drag_cancel(state),
    }
}
