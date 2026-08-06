//! Predecessor-style lane composer rendered inside the additive Mosaic tab.

use std::{collections::BTreeMap, rc::Rc};

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{ClipboardEvent, DragEvent, File, FileList, FileReader, HtmlInputElement};

use super::*;

const MAX_ATTACHMENTS: usize = 8;
const MAX_ATTACHMENT_BYTES: f64 = 8.0 * 1_024.0 * 1_024.0;

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComposerDraft {
    body: String,
    quotes: Vec<String>,
    attachments: Vec<DirectiveAttachment>,
    issue: Option<String>,
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComposerDrafts(BTreeMap<String, ComposerDraft>);

pub(super) enum ComposerAction {
    SetBody {
        lane: String,
        body: String,
    },
    AddQuote {
        lane: String,
        quote: String,
    },
    RemoveQuote {
        lane: String,
        index: usize,
    },
    AddAttachment {
        lane: String,
        attachment: DirectiveAttachment,
    },
    RemoveAttachment {
        lane: String,
        index: usize,
    },
    SetIssue {
        lane: String,
        issue: String,
    },
    Clear {
        lane: String,
    },
}

impl Reducible for ComposerDrafts {
    type Action = ComposerAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            ComposerAction::SetBody { lane, body } => next.draft_mut(lane).body = body,
            ComposerAction::AddQuote { lane, quote } => {
                let draft = next.draft_mut(lane);
                if !draft.quotes.contains(&quote) {
                    draft.quotes.push(quote);
                }
            }
            ComposerAction::RemoveQuote { lane, index } => {
                let draft = next.draft_mut(lane);
                if index < draft.quotes.len() {
                    draft.quotes.remove(index);
                }
            }
            ComposerAction::AddAttachment { lane, attachment } => {
                let draft = next.draft_mut(lane);
                if draft.attachments.len() >= MAX_ATTACHMENTS {
                    draft.issue = Some("Maximum images attached.".to_owned());
                } else {
                    draft.attachments.push(attachment);
                    draft.issue = None;
                }
            }
            ComposerAction::RemoveAttachment { lane, index } => {
                let draft = next.draft_mut(lane);
                if index < draft.attachments.len() {
                    draft.attachments.remove(index);
                }
            }
            ComposerAction::SetIssue { lane, issue } => {
                next.draft_mut(lane).issue = Some(issue);
            }
            ComposerAction::Clear { lane } => {
                next.0.remove(&lane);
            }
        }
        Rc::new(next)
    }
}

impl ComposerDrafts {
    fn draft_mut(&mut self, lane: String) -> &mut ComposerDraft {
        self.0.entry(lane).or_default()
    }

    fn get(&self, lane: &str) -> Option<&ComposerDraft> {
        self.0.get(lane)
    }
}

/// Renders the packed inbox belonging to one lane.
///
/// The mosaic owns which lanes exist and what each composer does with a quote;
/// the engine that packs a lane's cards lives beside the chrome that styles
/// them. Passing the feed in keeps that split intact, so this module never
/// reaches across it to place a card itself.
type LaneFeed<'a> = &'a dyn Fn(&crate::runtime::LaneRow, &[wire::intent::PendingDirective]) -> Html;

pub(super) fn render_mosaic(
    surface: &SurfaceData,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    feed: LaneFeed<'_>,
) -> Html {
    let Some(runtime) = surface.as_ref() else {
        return empty("No admitted mosaic is available.");
    };
    let Some(lanes) = runtime.lanes() else {
        return empty("No lane snapshot is available.");
    };
    let can_send = runtime.offers(CommandKind::SteerSend);
    html! {
        <div class="mosaic" data-view="mosaic">
            <div class="mosaic-heading">
                <div>
                    <h1>{"Mosaic"}</h1>
                    <p>{"Live lanes, quoted Markdown, and image-bearing steering."}</p>
                </div>
                <span class="section-count">{format!("{} lanes", lanes.lanes.len())}</span>
            </div>
            if lanes.lanes.is_empty() {
                {empty("No lanes are seated.")}
            } else {
                <div class="mosaic-lanes">
                    {for lanes.lanes.iter().map(|lane| {
                        let directives = runtime
                            .inbox()
                            .and_then(|inbox| inbox.lanes.iter().find(|row| row.lane == lane.lane))
                            .map(|row| row.pending.as_slice())
                            .unwrap_or(&[]);
                        render_lane(lane, lanes.epoch, directives, can_send, sender, drafts, feed)
                    })}
                </div>
            }
        </div>
    }
}

fn render_lane(
    lane: &crate::runtime::LaneRow,
    epoch: CanonicalUuid,
    directives: &[wire::intent::PendingDirective],
    can_send: bool,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
    feed: LaneFeed<'_>,
) -> Html {
    let lane_id = lane.lane;
    let lane_key = lane_id.to_string();
    let draft = drafts.get(&lane_key).cloned().unwrap_or_default();
    let occupant = lane.occupant;
    let send_enabled = can_send && occupant.is_some();
    let markdown = compose_markdown(&draft);
    let submit = submit_callback(
        lane_id,
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let oninput = update_body(lane_key.clone(), drafts.clone());
    let onkeydown = keyboard_submit(submit.clone());
    let onpaste = paste_images(lane_key.clone(), drafts.clone());
    let ondragover = Callback::from(move |event: DragEvent| {
        event.prevent_default();
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
    });
    let ondrop = drop_on_lane(
        lane_id,
        occupant,
        epoch,
        lane_key.clone(),
        sender.clone(),
        drafts.clone(),
    );
    let input_id = format!("mosaic-images-{lane_id}");
    let onchange = choose_images(lane_key.clone(), drafts.clone());
    html! {
        <section
            class={classes!("mosaic-lane", (!send_enabled).then_some("mosaic-lane--vacant"))}
            data-lane-id={lane_id.to_string()}
            {ondragover}
            {ondrop}
        >
            <header class="mosaic-lane-header">
                <div>
                    <strong>{format!("{} / {}", lane.vendor, short(&lane_id.to_string()))}</strong>
                    <span>{lane.worktree.clone()}</span>
                </div>
                <span class="tag active">{lane.disposition.clone()}</span>
            </header>
            <form class="mosaic-composer" onsubmit={submit}>
                {render_quote_bands(&lane_key, &draft, drafts)}
                <textarea
                    aria-label={format!("Message for {lane_id}")}
                    rows="4"
                    value={draft.body.clone()}
                    placeholder={composer_placeholder(lane, directives.len())}
                    {oninput}
                    {onkeydown}
                    {onpaste}
                ></textarea>
                {render_attachments(&lane_key, &draft, drafts)}
                if let Some(issue) = &draft.issue {
                    <p class="composer-issue" role="status">{issue.clone()}</p>
                }
                if !markdown.is_empty() {
                    <div class="composer-markdown" aria-label="Markdown preview">
                        {render_markdown(&markdown)}
                    </div>
                }
                <div class="mosaic-composer-actions">
                    <input
                        id={input_id.clone()}
                        class="visually-hidden"
                        type="file"
                        accept="image/*"
                        multiple=true
                        {onchange}
                    />
                    <label class="image-picker" for={input_id}>{"Images"}</label>
                    <span>{format!("{}/{}", draft.attachments.len(), MAX_ATTACHMENTS)}</span>
                    <button
                        class="composer-send"
                        type="submit"
                        disabled={!send_enabled || markdown.trim().is_empty()}
                    >{"Send"}</button>
                </div>
            </form>
            {feed(lane, directives)}
        </section>
    }
}

fn composer_placeholder(lane: &crate::runtime::LaneRow, pending: usize) -> String {
    let occupant = lane
        .occupant
        .map(|value| short(&value.to_string()))
        .unwrap_or_else(|| "vacant".to_owned());
    format!(
        "{} / {occupant}\n{pending} pending, {}",
        lane.vendor, lane.disposition
    )
}

fn submit_callback(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: String,
    sender: SenderHandle,
    drafts: UseReducerHandle<ComposerDrafts>,
) -> Callback<SubmitEvent> {
    Callback::from(move |event: SubmitEvent| {
        event.prevent_default();
        submit_draft(lane, occupant, epoch, &lane_key, &sender, &drafts);
    })
}

fn submit_draft(
    lane: CanonicalUuid,
    occupant: Option<CanonicalUuid>,
    epoch: CanonicalUuid,
    lane_key: &str,
    sender: &SenderHandle,
    drafts: &UseReducerHandle<ComposerDrafts>,
) {
    let Some(occupant) = occupant else {
        return;
    };
    let Some(draft) = drafts.get(lane_key).cloned() else {
        return;
    };
    let message = compose_markdown(&draft);
    if message.trim().is_empty() {
        return;
    }
    send_action(
        sender,
        ClientAction::Steer {
            lane,
            occupant,
            epoch,
            message,
            attachments: draft.attachments,
        },
    );
    drafts.dispatch(ComposerAction::Clear {
        lane: lane_key.to_owned(),
    });
}

fn update_body(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: web_sys::HtmlTextAreaElement = event.target_unchecked_into();
        drafts.dispatch(ComposerAction::SetBody {
            lane: lane_key.clone(),
            body: input.value(),
        });
    })
}

fn keyboard_submit(submit: Callback<SubmitEvent>) -> Callback<KeyboardEvent> {
    Callback::from(move |event: KeyboardEvent| {
        if event.key() == "Enter" && (event.meta_key() || event.ctrl_key()) {
            event.prevent_default();
            let event = SubmitEvent::new("submit").expect("submit event construction succeeds");
            submit.emit(event);
        }
    })
}

fn render_quote_bands(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> Html {
    html! {
        <div class="composer-quote-stack">
            {for draft.quotes.iter().enumerate().map(|(index, quote)| {
                let remove_key = lane_key.to_owned();
                let remove_drafts = drafts.clone();
                let remove = Callback::from(move |_| {
                    remove_drafts.dispatch(ComposerAction::RemoveQuote {
                        lane: remove_key.clone(),
                        index,
                    });
                });
                html! {
                    <div class="composer-quote-band">
                        <span>{quote.lines().next().unwrap_or("quoted message")}</span>
                        <button
                            type="button"
                            aria-label="Remove quote"
                            onclick={remove}
                        >{"×"}</button>
                    </div>
                }
            })}
        </div>
    }
}

fn render_attachments(
    lane_key: &str,
    draft: &ComposerDraft,
    drafts: &UseReducerHandle<ComposerDrafts>,
) -> Html {
    html! {
        <div class="composer-attachment-list">
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
                    <span class="composer-attachment-chip" title={attachment.name.clone()}>
                        <img src={attachment.data_url.clone()} alt={attachment.name.clone()} />
                        <button
                            type="button"
                            aria-label="Remove image"
                            onclick={remove}
                        >{"×"}</button>
                    </span>
                }
            })}
        </div>
    }
}

fn choose_images(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        if let Some(files) = input.files() {
            add_image_files(&drafts, &lane_key, &files);
        }
        input.set_value("");
    })
}

fn paste_images(lane_key: String, drafts: UseReducerHandle<ComposerDrafts>) -> Callback<Event> {
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

fn drop_on_lane(
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
        let message = markdown_block_quote(&text);
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

fn compose_markdown(draft: &ComposerDraft) -> String {
    crate::composer_markdown::compose(&draft.body, &draft.quotes)
}

fn markdown_block_quote(raw: &str) -> String {
    crate::composer_markdown::block_quote(raw)
}

fn render_markdown(markdown: &str) -> Html {
    html! {
        {for crate::composer_markdown::render(markdown).into_iter().map(|block| {
            match block {
                crate::composer_markdown::Block::Paragraph(body) => html! {<p>{body}</p>},
                crate::composer_markdown::Block::Quote(quote) => {
                    html! {<blockquote>{quote}</blockquote>}
                }
            }
        })}
    }
}
