use super::*;
use crate::browser::mermaid::MarkdownControl;

struct MessageControlContext<'a> {
    message: &'a wire::AgentMessage,
    quote_lane: CanonicalUuid,
    quote_dispatcher: &'a UseReducerDispatcher<ComposerDrafts>,
    speak: &'a Callback<String>,
    speaking: bool,
    order: u64,
    producer_label: &'a str,
    attribution: &'a str,
}

fn collect_quote_controls(
    quotes: &[wire::QuotePresentation],
    quote_sources: &crate::composer_markdown::QuoteSources,
    context: &MessageControlContext<'_>,
    controls: &mut Vec<MarkdownControl>,
) {
    for quote in quotes {
        let wire::QuotePresentation::Resolved {
            target,
            text,
            quotes,
            ..
        } = quote
        else {
            continue;
        };
        let Some(quote_target) = target.parse::<wire::QuoteTarget>().ok() else {
            continue;
        };
        let display_millis = quote_sources.get(target).map_or_else(
            || crate::runtime::agent_message_display_millis(context.message),
            |source| source.display_millis,
        );
        let source_id = quote_sources
            .get(target)
            .and_then(|source| source.card_id.clone())
            .unwrap_or_else(|| agent_message_source_id(context.message));
        push_quote_control(
            quote_target,
            target,
            source_id,
            text,
            display_millis,
            context,
            controls,
        );
        collect_quote_controls(quotes, quote_sources, context, controls);
    }
}

fn collect_directive_controls(
    directives: &[wire::MessageDirective],
    text: &str,
    context: &MessageControlContext<'_>,
    controls: &mut Vec<MarkdownControl>,
) {
    for directive in directives {
        let (Ok(start), Ok(end)) = (
            usize::try_from(directive.start),
            usize::try_from(directive.end),
        ) else {
            continue;
        };
        let Some(text) = text.get(start..end) else {
            continue;
        };
        push_quote_control(
            directive.quote_target.clone(),
            &directive.quote_target.to_string(),
            agent_message_source_id(context.message),
            text,
            directive.authored.get().as_millisecond(),
            context,
            controls,
        );
    }
}

fn push_quote_control(
    target: wire::QuoteTarget,
    target_text: &str,
    source_id: String,
    text: &str,
    display_millis: i64,
    context: &MessageControlContext<'_>,
    controls: &mut Vec<MarkdownControl>,
) {
    let lane = context.quote_lane.to_string();
    let focus_identity = target_text.to_owned();
    let focus_lane = lane.clone();
    let dispatcher = context.quote_dispatcher.clone();
    let preview =
        watchband::quote_label(watchband::first_nonquoted_prose(text).as_deref()).to_owned();
    let order = context.order;
    let quote = Callback::from(move |()| {
        dispatcher.dispatch(ComposerAction::AddQuote {
            lane: lane.clone(),
            target: target.clone(),
            source_id: source_id.clone(),
            order,
            display_millis,
            preview: preview.clone(),
        });
        focus_after_render(quote_textarea_id(&focus_lane, &focus_identity));
    });
    let correspondent = view::chrome::Correspondent {
        accent: 0,
        speak: context.speak.clone(),
        speaking: context.speaking,
        quote: Some(quote),
    };
    controls.push(MarkdownControl {
        target: AttrValue::from(target_text.to_owned()),
        content: render_message_controls(
            context.message,
            text,
            &correspondent,
            true,
            context.producer_label,
            context.attribution,
        ),
    });
}

pub(super) struct MessageChrome {
    pub(super) signoff: Option<Html>,
    pub(super) controls: Rc<Vec<MarkdownControl>>,
    pub(super) corner: Option<Html>,
}

pub(super) fn render_message_chrome(
    props: &AgentMessageCardProps,
    shown_text: &str,
    is_prose: bool,
    lone_tool_image: bool,
    correspondent: &view::chrome::Correspondent,
    producer_label: &str,
    attribution: &str,
) -> MessageChrome {
    let message = &props.message;
    let signoff =
        is_prose.then(|| render_agent_message_signoff(message, producer_label, attribution));
    let mut controls = Vec::new();
    if is_prose {
        controls.push(MarkdownControl {
            target: AttrValue::from(""),
            content: render_message_controls(
                message,
                shown_text,
                correspondent,
                true,
                producer_label,
                attribution,
            ),
        });
    }
    let control_context = MessageControlContext {
        message,
        quote_lane: props.quote_lane,
        quote_dispatcher: &props.quote_dispatcher,
        speak: &correspondent.speak,
        speaking: correspondent.speaking,
        order: props.order,
        producer_label,
        attribution,
    };
    collect_quote_controls(
        &message.quotes,
        &props.quote_sources,
        &control_context,
        &mut controls,
    );
    collect_directive_controls(
        &message.directive_contexts,
        shown_text,
        &control_context,
        &mut controls,
    );
    let corner = (!is_prose && !lone_tool_image).then(|| {
        render_message_controls(
            message,
            shown_text,
            correspondent,
            false,
            producer_label,
            attribution,
        )
    });
    MessageChrome {
        signoff,
        controls: Rc::new(controls),
        corner,
    }
}
