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
) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
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
    pub(super) speech: LineageSpeech,
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
    let speak = if props.speech == LineageSpeech::Quiet {
        Callback::from(|_: String| {})
    } else {
        let lane = props.lane.to_string();
        let message_key = agent_message_source_id(&props.message);
        let speech_queue = props.speech_queue.clone();
        Callback::from(move |text: String| {
            speech_queue.emit(SpeechRequest::message(
                lane.clone(),
                message_key.clone(),
                text,
            ));
        })
    };
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
    let source_id = agent_message_source_id(message);
    let attribution = watchband::agent_message_attribution(
        message.author,
        member.lane,
        &member.worktree,
        member.name.as_deref(),
    );
    let footer_label = watchband::agent_message_label(&member.worktree, member.name.as_deref())
        .expect("admitted agent message has a configured name or worktree basename");
    let is_compaction = message.activity == wire::AgentActivityKind::Compaction;
    let rendered_markdown = crate::composer_markdown::render_received(&message.text);
    let card_label = if is_compaction {
        format!("Context compacted by {footer_label}")
    } else {
        format!("Agent message from {attribution}")
    };
    let unlinked_media = message
        .media
        .iter()
        .filter(|media| {
            !media
                .is_embedded_in(&message.text)
                .expect("message media passed admission before browser rendering")
        })
        .collect::<Vec<_>>();
    let image_only = rendered_markdown.image_only
        || (!unlinked_media.is_empty() && message.text.trim().is_empty());
    let rendered_html = Html::from_html_unchecked(AttrValue::from(rendered_markdown.html));
    html! {
        <article
            key={source_id.clone()}
            id={source_id.clone()}
            class={classes!(
                "mosaic-message-card",
                is_compaction.then_some("mosaic-compaction-card"),
                (!is_compaction && image_only).then_some("mosaic-message-card--image-only"),
                message.final_answer.then_some("final"),
                correspondent.speaking.then_some("now-playing"),
                layout.lifecycle.as_str(),
            )}
            aria-label={card_label}
            data-card-kind={agent_card_kind(is_compaction)}
            data-message-role="agent"
            data-mosaic-card-key={source_id}
            data-layout-phase={if layout.committed { "committed" } else { "measuring" }}
            data-agent-lane={member.lane.to_string()}
            data-agent-activity={agent_activity_name(message.activity)}
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
            if is_compaction {
                {render_compaction_boundary(message, footer_label)}
            } else {
                <div class="message-body markdown-body">{rendered_html}</div>
                {render_agent_message_media(unlinked_media)}
                {render_agent_message_footer(
                    message,
                    correspondent,
                    crate::runtime::agent_message_display_millis(message),
                    footer_label,
                    &attribution,
                )}
            }
        </article>
    }
}

fn render_compaction_boundary(message: &wire::AgentMessage, attribution: &str) -> Html {
    let attribution = attribution.to_owned();
    let display_millis = crate::runtime::agent_message_display_millis(message);
    html! {
        <>
            <span class="mosaic-compaction-rule" aria-hidden="true"></span>
            <span class="mosaic-compaction-label">
                <Age authored={display_millis} class="mosaic-compaction-age" />
                <span aria-hidden="true">{"\u{b7}"}</span>
                <span class="mosaic-compaction-context">
                    <span class="mosaic-compaction-agent" title={attribution.clone()}>{attribution}</span>
                    <span class="message-key">{"compacted context"}</span>
                </span>
            </span>
            <span class="mosaic-compaction-rule" aria-hidden="true"></span>
            if !message.unavailable_tool_terminals.is_empty() {
                <ul
                    class="mosaic-compaction-terminals"
                    aria-label="Tool terminal results unavailable after context compaction"
                >
                    {for message.unavailable_tool_terminals.iter().map(|preview| html! {
                        <li
                            class="mosaic-compaction-terminal"
                            data-terminal-status="unavailable-after-compaction"
                        >
                            <span class="mosaic-compaction-tool" title={preview.clone()}>{preview}</span>
                            <span class="mosaic-compaction-status">
                                {"terminal result unavailable after context compaction"}
                            </span>
                        </li>
                    })}
                </ul>
            }
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

fn agent_card_kind(is_compaction: bool) -> &'static str {
    if is_compaction {
        "compaction"
    } else {
        "agent-message"
    }
}

fn agent_activity_name(activity: wire::AgentActivityKind) -> &'static str {
    match activity {
        wire::AgentActivityKind::Prose => "prose",
        wire::AgentActivityKind::Tool => "tool",
        wire::AgentActivityKind::Compaction => "compaction",
    }
}

fn render_agent_message_media(unlinked_media: Vec<&wire::Media>) -> Html {
    if unlinked_media.is_empty() {
        return Html::default();
    }
    html! {
        <div class="message-attachments">
            {for unlinked_media.into_iter().map(|media| {
                let action = format!("Open {} full size", media.name);
                html! {
                    <a
                        class="message-image message-image--openable"
                        href="about:blank"
                        target="_blank"
                        rel="noopener"
                        title={action.clone()}
                        aria-label={action}
                        data-full-size-data-image="true"
                    >
                        <img
                            src={media.data_url.clone()}
                            alt={media.name.clone()}
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
                    </a>
                }
            })}
        </div>
    }
}

fn render_agent_message_footer(
    message: &wire::AgentMessage,
    correspondent: &view::chrome::Correspondent,
    display_millis: i64,
    producer_label: &str,
    attribution: &str,
) -> Html {
    let locus = format!(
        "tape {}:{}+{}#{}",
        message.locus.member, message.locus.offset, message.locus.length, message.locus.event,
    );
    let has_actions = !message.text.trim().is_empty();
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
            if has_actions {
                <span class="message-footer-separator" aria-hidden="true">{"\u{b7}"}</span>
            }
        </span>
    };
    html! {
        <footer class="message-footer">
            <div class="message-footer-left">
                <Age authored={display_millis} class="message-age" title={locus} />
            </div>
            <div class="message-footer-right">
                {producer_identity}
                if has_actions {
                    {view::chrome::render_card_actions(&message.text, correspondent)}
                }
            </div>
        </footer>
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
                let shown = crate::composer_markdown::quote_label(quote.preview.as_deref());
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

/// Draft images ride in the shard heading as message-card thumbnails shrunk to
/// heading scale. The thumbnail is itself the remove control: a separate × would
/// be wider than the image it decorates and would crowd the prose, deep link,
/// and final control sharing that row. The strip is rendered even with no images
/// so the heading keeps its fixed child order and its grid columns, which holds
/// that final control still as the first image arrives.
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
                    <button
                        type="button"
                        class="composer-shard-image-indicator"
                        title={format!("Remove {}", attachment.name)}
                        aria-label={format!("Remove image {}", attachment.name)}
                        onclick={remove}
                    >
                        <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                    </button>
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
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
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
        let Some(occupant) = occupant else {
            return;
        };
        let message = crate::composer_markdown::block_quote(&text);
        if message.is_empty() {
            return;
        }
        send_action(
            &sender,
            ClientAction::Steer {
                lane,
                occupant,
                epoch,
                message,
                attachments: Vec::new(),
            },
        );
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
