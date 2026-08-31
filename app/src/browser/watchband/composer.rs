//! Browser callbacks, activity cards, attachments, and safe Markdown for lane messages.

use pulldown_cmark::{CowStr, Event as MarkdownEvent, Options, Parser, Tag, TagEnd};
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
    pub(super) member: crate::runtime::LaneMember,
    pub(super) quote_lane: CanonicalUuid,
    pub(super) message: Rc<wire::AgentMessage>,
    pub(super) quote_dispatcher: UseReducerDispatcher<ComposerDrafts>,
    pub(super) speech_queue: Callback<SpeechRequest>,
    pub(super) speech: LineageSpeech,
    pub(super) lane: CanonicalUuid,
    pub(super) order: u64,
    pub(super) layout: CardLayout,
}

impl PartialEq for AgentMessageCardProps {
    fn eq(&self, other: &Self) -> bool {
        self.member.lane == other.member.lane
            && self.member.name == other.member.name
            && self.member.worktree == other.member.worktree
            && self.quote_lane == other.quote_lane
            && (Rc::ptr_eq(&self.message, &other.message) || self.message == other.message)
            && self.quote_dispatcher == other.quote_dispatcher
            && self.speech_queue == other.speech_queue
            && self.speech == other.speech
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
        let speech_queue = props.speech_queue.clone();
        Callback::from(move |text: String| {
            speech_queue.emit(SpeechRequest::manual(Some(lane.clone()), text));
        })
    };
    render_agent_message_card(
        &props.member,
        props.quote_lane,
        &props.message,
        &props.quote_dispatcher,
        &speak,
        props.order,
        &props.layout,
    )
}

fn render_agent_message_card(
    member: &crate::runtime::LaneMember,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    quote_dispatcher: &UseReducerDispatcher<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
    layout: &CardLayout,
) -> Html {
    let source_id = agent_message_source_id(message);
    let attribution = watchband::agent_message_attribution(
        message.author,
        member.lane,
        &member.worktree,
        member.name.as_deref(),
    );
    let correspondent =
        agent_message_correspondent(member, quote_lane, message, quote_dispatcher, speak, order);
    let is_compaction = message.activity == wire::AgentActivityKind::Compaction;
    let card_label = if is_compaction {
        format!("Context compacted by {attribution}")
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
    let image_only = markdown_is_image_only(&message.text)
        || (message.text.trim().is_empty() && !unlinked_media.is_empty());
    html! {
        <article
            key={source_id.clone()}
            id={source_id.clone()}
            class={classes!(
                "mosaic-message-card",
                is_compaction.then_some("mosaic-compaction-card"),
                image_only.then_some("image-only"),
                layout.lifecycle.as_str(),
            )}
            aria-label={card_label}
            data-card-kind={agent_card_kind(is_compaction)}
            data-message-role="agent"
            data-mosaic-card-key={source_id}
            data-layout-phase={if layout.committed { "committed" } else { "measuring" }}
            data-agent-lane={member.lane.to_string()}
            data-agent-activity={agent_activity_name(message.activity)}
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
        >
            if is_compaction {
                {render_compaction_boundary(message, &attribution)}
            } else {
                <div class="message-body markdown-body">{render_markdown(&message.text)}</div>
                {render_agent_message_media(unlinked_media)}
                {render_agent_message_footer(
                    message,
                    &correspondent,
                    crate::runtime::agent_message_display_millis(message),
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
        </>
    }
}

fn agent_message_correspondent(
    member: &crate::runtime::LaneMember,
    quote_lane: CanonicalUuid,
    message: &wire::AgentMessage,
    quote_dispatcher: &UseReducerDispatcher<ComposerDrafts>,
    speak: &Callback<String>,
    order: u64,
) -> view::chrome::Correspondent {
    let lane_key = quote_lane.to_string();
    let quote_text = message.text.clone();
    let quote_preview = watchband::first_nonquoted_prose(&message.text);
    let quote_identity = agent_message_quote_identity(message);
    let focus_identity = quote_identity.clone();
    let focus_lane = lane_key.clone();
    let quote_dispatcher = quote_dispatcher.clone();
    view::chrome::Correspondent {
        accent: watchband::accent(member.lane),
        speak: speak.clone(),
        quote: Callback::from(move |()| {
            quote_dispatcher.dispatch(ComposerAction::AddQuote {
                lane: lane_key.clone(),
                identity: quote_identity.clone(),
                order,
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
            {for unlinked_media.into_iter().map(|media| html! {
                <span class="message-image message-attachment" title={format!("Embedded image {}", media.name)}>
                    <img
                        src={media.data_url.clone()}
                        alt={media.name.clone()}
                        decoding="async"
                        data-media-identity={media.identity().expect("admitted message media has identity").to_string()}
                    />
                </span>
            })}
        </div>
    }
}

fn render_agent_message_footer(
    message: &wire::AgentMessage,
    correspondent: &view::chrome::Correspondent,
    display_millis: i64,
    producer: &str,
) -> Html {
    let locus = format!(
        "tape {}:{}+{}#{}",
        message.locus.member, message.locus.offset, message.locus.length, message.locus.event,
    );
    let card_actions = view::chrome::render_card_actions(&message.text, correspondent);
    let producer = producer.to_owned();
    let author = message.author.to_string();
    let author_title = format!("Copy agent id\n{author}");
    let copy_author = Callback::from(move |_| view::chrome::copy_text(&author));
    let producer_identity = html! {
        <>
            <button
                class="message-agent-name"
                type="button"
                title={author_title}
                aria-label={format!("Copy agent id for {producer}")}
                onclick={copy_author}
            >{producer}</button>
            <span class="message-footer-separator" aria-hidden="true">{"\u{b7}"}</span>
        </>
    };
    html! {
        <footer class="message-footer">
            <div class="message-footer-left">
                <Age authored={display_millis} class="message-age" title={locus} />
            </div>
            <div class="message-footer-right">
                {producer_identity}
                {card_actions}
            </div>
        </footer>
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
/// and hamburger sharing that row. The strip is rendered even with no images so
/// the heading keeps its fixed child order and its grid columns, which is what
/// holds the hamburger still as the first image arrives.
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
        match DirectiveAttachment::admit(name.clone(), content_type.clone(), data_url) {
            Ok(attachment) => drafts.dispatch(ComposerAction::AddAttachment {
                lane: lane_key.clone(),
                attachment,
            }),
            Err(error) => drafts.dispatch(ComposerAction::SetIssue {
                lane: lane_key.clone(),
                issue: format!("Image was refused: {error}."),
            }),
        }
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
    let mut rendered_tasks = Vec::new();
    let mut image = None;
    let mut code_block = false;
    let mut events = Vec::new();
    for event in Parser::new_ext(markdown, options) {
        let event = sanitize_markdown_event(event);
        if image.is_some() {
            match event {
                MarkdownEvent::End(TagEnd::Image) => {
                    let image = image.take().expect("an open Markdown image closes once");
                    events.push(MarkdownEvent::InlineHtml(
                        render_markdown_image(image).into(),
                    ));
                }
                MarkdownEvent::Text(text) | MarkdownEvent::Code(text) => {
                    image
                        .as_mut()
                        .expect("Markdown image alt text has an open image")
                        .alt
                        .push_str(&text);
                }
                MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => image
                    .as_mut()
                    .expect("Markdown image break has an open image")
                    .alt
                    .push(' '),
                _ => {}
            }
            continue;
        }
        if let MarkdownEvent::Start(Tag::Image {
            dest_url, title, ..
        }) = event
        {
            image = Some(MarkdownImage {
                source: dest_url,
                title,
                alt: String::new(),
            });
            continue;
        }
        match &event {
            MarkdownEvent::Start(Tag::CodeBlock(_)) => code_block = true,
            MarkdownEvent::End(TagEnd::CodeBlock) => code_block = false,
            _ => {}
        }
        if code_block {
            events.push(event);
        } else {
            events.extend(inline_task_chips(event, &mut rendered_tasks));
        }
    }
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, events.into_iter());
    Html::from_html_unchecked(AttrValue::from(output))
}

struct MarkdownImage<'a> {
    source: CowStr<'a>,
    title: CowStr<'a>,
    alt: String,
}

fn render_markdown_image(image: MarkdownImage<'_>) -> String {
    let source = image.source.trim();
    let alt = image.alt.trim();
    let alt = if alt.is_empty() { "image" } else { alt };
    let escaped_alt = escaped_html(alt);
    if source.is_empty() {
        return format!(
            "<span class=\"message-image message-image--unsupported\" role=\"img\" \
             aria-label=\"Image unavailable: {escaped_alt}\">[image: {escaped_alt}]</span>"
        );
    }

    let escaped_source = escaped_href(source);
    let lower_source = source.to_ascii_lowercase();
    if lower_source.starts_with("data:image/") {
        return format!(
            "<span class=\"message-image\" title=\"Embedded image\">\
             <img src=\"{escaped_source}\" alt=\"{escaped_alt}\" decoding=\"async\"></span>"
        );
    }

    let label = escaped_html(&format!("Open {alt} full size"));
    let title = if image.title.trim().is_empty() {
        label.clone()
    } else {
        escaped_html(image.title.trim())
    };
    format!(
        "<a class=\"message-image\" href=\"{escaped_source}\" title=\"{title}\" \
         aria-label=\"{label}\" target=\"_blank\" rel=\"noopener\">\
         <img src=\"{escaped_source}\" alt=\"{escaped_alt}\" decoding=\"async\"></a>"
    )
}

fn escaped_href(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'#'..=b'%' | b'('..=b';' | b'=' | b'?' | b'@' | b'^' | b'_' | b'~'
            )
        {
            escaped.push(char::from(byte));
        } else if byte == b'&' {
            escaped.push_str("&amp;");
        } else if byte == b'\'' {
            escaped.push_str("&#x27;");
        } else {
            escaped.push('%');
            escaped.push(char::from(HEX[usize::from(byte >> 4)]));
            escaped.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    escaped
}

fn escaped_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn markdown_is_image_only(markdown: &str) -> bool {
    let mut depth = 0_u32;
    let mut images = 0_u32;
    for event in Parser::new_ext(markdown, Options::ENABLE_GFM) {
        match event {
            MarkdownEvent::Start(Tag::Image { .. }) => {
                depth = depth.saturating_add(1);
                images = images.saturating_add(1);
            }
            MarkdownEvent::End(TagEnd::Image) => depth = depth.saturating_sub(1),
            MarkdownEvent::Start(Tag::Paragraph) | MarkdownEvent::End(TagEnd::Paragraph) => {}
            MarkdownEvent::Text(text) if depth > 0 || text.trim().is_empty() => {}
            MarkdownEvent::Code(_) if depth > 0 => {}
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => {}
            MarkdownEvent::Start(_) | MarkdownEvent::End(_) if depth > 0 => {}
            _ => return false,
        }
    }
    images > 0 && depth == 0
}

fn inline_task_chips<'a>(
    event: MarkdownEvent<'a>,
    rendered: &mut Vec<wire::TaskHandle>,
) -> Vec<MarkdownEvent<'a>> {
    match event {
        MarkdownEvent::Text(text) => inline_task_text(text, rendered),
        MarkdownEvent::Code(code) => {
            let Some(task) = watchband::canonical_task_handle(&code) else {
                return vec![MarkdownEvent::Code(code)];
            };
            if rendered.contains(&task) {
                vec![MarkdownEvent::Code(code)]
            } else {
                rendered.push(task.clone());
                vec![MarkdownEvent::InlineHtml(task_chip_html(&task).into())]
            }
        }
        other => vec![other],
    }
}

fn inline_task_text<'a>(
    text: CowStr<'a>,
    rendered: &mut Vec<wire::TaskHandle>,
) -> Vec<MarkdownEvent<'a>> {
    let mentions = watchband::task_mentions(&text);
    if mentions.is_empty() {
        return vec![MarkdownEvent::Text(text)];
    }
    let mut events = Vec::new();
    let mut cursor = 0;
    for (range, task) in mentions {
        if rendered.contains(&task) {
            continue;
        }
        if cursor < range.start {
            events.push(MarkdownEvent::Text(
                text[cursor..range.start].to_owned().into(),
            ));
        }
        events.push(MarkdownEvent::InlineHtml(task_chip_html(&task).into()));
        rendered.push(task);
        cursor = range.end;
    }
    if cursor < text.len() {
        events.push(MarkdownEvent::Text(text[cursor..].to_owned().into()));
    }
    events
}

fn task_chip_html(task: &wire::TaskHandle) -> String {
    // TaskHandle parsing admits only the canonical ASCII handle grammar, so
    // these fixed attributes cannot acquire markup from authored prose.
    let handle = task.to_string();
    format!(
        "<span class=\"message-task-chip\" data-task-handle=\"{handle}\" \
         aria-label=\"Reported task {handle}\" title=\"{handle}\">{handle}</span>"
    )
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
        || (!image && candidate.starts_with("mailto:"))
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
