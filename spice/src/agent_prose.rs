//! Transcript event shapes promoted into agent prose cards.

/// Assistant prose one transcript event presents as a quoteable message card.
///
/// Ordinary assistant prose presents itself. Streamed prose presents when it
/// carries an acknowledgement. A tool-call intent presents when it carries an
/// acknowledgement or an explicitly authored MSG, ASK, or ANS packet: OMP
/// retains intent as the assistant message's only verbatim words when no text
/// block survived settlement.
#[must_use]
pub fn quoteable(decoded: &listen::Event) -> Option<std::borrow::Cow<'_, listen::Prose>> {
    match decoded {
        listen::Event::Prose(prose) if prose.role == listen::ProseRole::Assistant => {
            Some(std::borrow::Cow::Borrowed(prose))
        }
        listen::Event::StreamedProse(prose)
            if prose.role == listen::ProseRole::Assistant
                && steer::has_acknowledgement(&prose.text) =>
        {
            Some(std::borrow::Cow::Borrowed(prose))
        }
        listen::Event::Tool(listen::Tool {
            moment:
                listen::ToolMoment::Call {
                    intent: Some(intent),
                    ..
                },
            locus,
            ..
        }) if steer::has_acknowledgement(intent) || steer::ack::scan::has_message(intent) => {
            Some(std::borrow::Cow::Owned(listen::Prose {
                role: listen::ProseRole::Assistant,
                text: intent.clone(),
                phase: None,
                media: Vec::new(),
                external_media: Vec::new(),
                locus: *locus,
            }))
        }
        _ => None,
    }
}
