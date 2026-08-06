//! Browser callbacks, activity cards, attachments, and safe Markdown for lane messages.

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag};
use web_sys::{ClipboardEvent, DragEvent, File, FileList, FileReader};

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
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            if submit_draft(lane, occupant, epoch, &lane_key, &sender, &drafts) {
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

pub(super) fn directive_source_id(directive: &wire::intent::PendingDirective) -> String {
    format!("message-directive-{}", directive.directive)
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

pub(super) fn render_agent_message_card(
    member: &crate::runtime::LaneMember,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    drafts: &UseReducerHandle<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
    layout: &CardLayout,
) -> Html {
    let drag_text = message.text.clone();
    let ondragstart = Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            let _ = transfer.set_data("text/plain", &drag_text);
            transfer.set_effect_allowed("copy");
        }
    });
    let lane_key = quote_lane.to_string();
    let quote_key = lane_key.clone();
    let quote_drafts = drafts.clone();
    let quote_text = message.text.clone();
    let quote_preview = watchband::first_nonquoted_prose(&message.text);
    let quote_identity = agent_message_quote_identity(message);
    let source_id = agent_message_source_id(message);
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.clone();
    let producer = format!(
        "{} · {}",
        watchband::leaf(&member.worktree),
        watchband::short(message.author),
    );
    let correspondent = view::chrome::Correspondent {
        name: producer.clone(),
        accent: watchband::accent(member.lane),
        speak: speak.clone(),
        quote: Callback::from(move |()| {
            quote_drafts.dispatch(ComposerAction::AddQuote {
                lane: quote_key.clone(),
                identity: quote_identity.clone(),
                order,
                quote: quote_text.clone(),
                preview: quote_preview.clone(),
            });
            focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
        }),
    };
    let locus = format!(
        "tape {}:{}+{}#{}",
        message.locus.member, message.locus.offset, message.locus.length, message.locus.event,
    );
    let reported_tasks = watchband::reported_tasks(&message.text);
    html! {
        <article
            key={format!(
                "agent-{}-{}-{}",
                message.locus.member,
                message.locus.offset,
                message.locus.event,
            )}
            id={source_id}
            class={classes!("mosaic-message-card", layout.lifecycle.as_str())}
            draggable="true"
            aria-label={format!("Agent message from {producer}")}
            data-card-kind="agent-message"
            data-agent-lane={member.lane.to_string()}
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
            data-measured-heights={layout.measured_heights.clone()}
            data-candidate-spans={layout.candidate_spans.clone()}
            data-measure-span={layout.measure_span.to_string()}
            {ondragstart}
        >
            <div class="message-body markdown-body">{render_markdown(&message.text)}</div>
            <footer class="message-meta">
                <span class="message-classifiers">
                    <span class="message-key">{"agent"}</span>
                    {for reported_tasks.into_iter().map(|task| {
                        let handle = task.to_string();
                        html! {
                            <span
                                class="message-task-chip"
                                data-task-handle={handle.clone()}
                                aria-label={format!("Reported task {handle}")}
                                title={handle.clone()}
                            >{handle}</span>
                        }
                    })}
                </span>
                <span class="message-age" title={locus.clone()}>{locus}</span>
                <span class="message-author">{producer}</span>
                {view::chrome::render_card_actions(&message.text, &correspondent)}
            </footer>
        </article>
    }
}

pub(super) fn render_directive_card(
    lane: CanonicalUuid,
    directive: &wire::intent::PendingDirective,
    lane_key: &str,
    drafts: &UseReducerHandle<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
    layout: &CardLayout,
) -> Html {
    let drag_text = directive.text.clone();
    let ondragstart = Callback::from(move |event: DragEvent| {
        if let Some(transfer) = event.data_transfer() {
            let _ = transfer.set_data("text/plain", &drag_text);
            transfer.set_effect_allowed("copy");
        }
    });
    let quote_key = lane_key.to_owned();
    let quote_drafts = drafts.clone();
    let quote_preview = directive.preview.clone();
    let quote_text = directive.text.clone();
    let quote_identity = format!("directive-{}", directive.directive);
    let source_id = directive_source_id(directive);
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.to_owned();
    let correspondent = view::chrome::Correspondent {
        name: watchband::short(lane),
        accent: watchband::accent(lane),
        speak: speak.clone(),
        quote: Callback::from(move |()| {
            quote_drafts.dispatch(ComposerAction::AddQuote {
                lane: quote_key.clone(),
                identity: quote_identity.clone(),
                order,
                quote: quote_text.clone(),
                preview: quote_preview.clone(),
            });
            focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
        }),
    };
    let age = js_sys::Date::now() as i64 - directive.authored.get().as_millisecond();
    let chronology = format!(
        "authored {}; expires {}",
        directive.authored, directive.expires
    );
    let unlinked_attachments = directive
        .attachments
        .iter()
        .filter(|attachment| !directive.text.contains(&attachment.data_url))
        .collect::<Vec<_>>();
    html! {
        <article
            key={directive.directive.to_string()}
            id={source_id}
            class={classes!("mosaic-message-card", layout.lifecycle.as_str())}
            draggable="true"
            data-directive={directive.directive.to_string()}
            data-card-kind="directive"
            data-agent-lane={lane.to_string()}
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
            data-measured-heights={layout.measured_heights.clone()}
            data-candidate-spans={layout.candidate_spans.clone()}
            data-measure-span={layout.measure_span.to_string()}
            {ondragstart}
        >
            <div class="message-body markdown-body">{render_markdown(&directive.text)}</div>
            if !unlinked_attachments.is_empty() {
                <div class="message-attachments">
                    {for unlinked_attachments.into_iter().map(|attachment| html! {
                        <img
                            src={attachment.data_url.clone()}
                            alt={attachment.name.clone()}
                            title={attachment.name.clone()}
                        />
                    })}
                </div>
            }
            <footer class="message-meta">
                <span class="message-key">{directive.key.clone()}</span>
                <span class="message-age" title={chronology}>{watchband::elapsed_label(age)}</span>
                <span class="message-author">{correspondent.name.clone()}</span>
                {view::chrome::render_card_actions(&directive.text, &correspondent)}
            </footer>
        </article>
    }
}

pub(super) fn render_quote_shards(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
    target_submit: Callback<KeyboardEvent>,
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
                                class="composer-shard-link"
                                href={format!("#{source_id}")}
                                aria-label={format!("Return to quoted message: {shown}")}
                                title="Return to this message in the stream"
                            >{"↩"}</a>
                            <strong title={shown.to_owned()}>{shown}</strong>
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
                            placeholder="Reply to this quoted message"
                            oninput={update}
                            onkeydown={target_submit.clone()}
                        ></textarea>
                    </article>
                }
            })}
        </div>
    }
}

/// Draft images ride in the shard heading as message-card thumbnails shrunk to
/// heading scale. The thumbnail is itself the remove control: a separate × would
/// be wider than the image it decorates and would crowd the prose, deep link,
/// and hamburger sharing that row. The indicator strip is always rendered so the
/// heading keeps its fixed child order; CSS hides it while it is empty.
pub(super) fn render_image_indicators(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> Html {
    html! {
        <span class="composer-shard-image-indicators">
            {for draft.attachments.iter().enumerate().map(|(index, attachment)| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveAttachment {
                        lane: remove_key.clone(),
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
            add_image_files(&drafts, &lane_key, &files);
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
            add_image_files(&drafts, &lane_key, &files);
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
                resolve: None,
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

fn add_image_files(drafts: &UseReducerHandle<ComposerDrafts>, lane_key: &str, files: &FileList) {
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
                issue: "Image is over 8 MB.".to_owned(),
            });
            continue;
        }
        read_image(drafts.clone(), lane_key.to_owned(), file);
    }
}

fn read_image(drafts: UseReducerHandle<ComposerDrafts>, lane_key: String, file: File) {
    let Ok(reader) = FileReader::new() else {
        drafts.dispatch(ComposerAction::SetIssue {
            lane: lane_key,
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
        drafts.dispatch(ComposerAction::AddAttachment {
            lane: lane_key.clone(),
            attachment: DirectiveAttachment {
                name: name.clone(),
                content_type: content_type.clone(),
                data_url,
            },
        });
    });
    reader.set_onload(Some(onload.as_ref().unchecked_ref()));
    if reader.read_as_data_url(&file).is_ok() {
        onload.forget();
    }
}

/// Render received text. Drafts are never rendered: a composer shard shows the
/// operator what they typed, so the built Markdown is only ever the message sent.
fn render_markdown(markdown: &str) -> Html {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_SMART_PUNCTUATION
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let events = Parser::new_ext(markdown, options).map(sanitize_markdown_event);
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events);
    Html::from_html_unchecked(AttrValue::from(output))
}

fn sanitize_markdown_event(event: MarkdownEvent<'_>) -> MarkdownEvent<'_> {
    match event {
        MarkdownEvent::Start(tag) => MarkdownEvent::Start(sanitize_markdown_tag(tag)),
        MarkdownEvent::Html(raw) | MarkdownEvent::InlineHtml(raw) => MarkdownEvent::Text(raw),
        other => other,
    }
}

fn sanitize_markdown_tag(tag: Tag<'_>) -> Tag<'_> {
    match tag {
        Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Link {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, false),
            title,
            id,
        },
        Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        } => Tag::Image {
            link_type,
            dest_url: sanitize_markdown_url(dest_url, true),
            title,
            id,
        },
        other => other,
    }
}

fn sanitize_markdown_url(url: CowStr<'_>, image: bool) -> CowStr<'_> {
    let candidate = url.trim().to_ascii_lowercase();
    let safe_scheme = candidate.starts_with("https://")
        || candidate.starts_with("http://")
        || candidate.starts_with("mailto:")
        || (image && candidate.starts_with("data:image/"));
    let relative = !candidate.starts_with("//")
        && !candidate
            .split(['/', '#', '?'])
            .next()
            .is_some_and(|head| head.contains(':'));
    if safe_scheme || relative {
        url
    } else {
        CowStr::Borrowed("")
    }
}
