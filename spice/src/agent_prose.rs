//! Transcript event shapes promoted into agent prose cards.

/// Assistant prose one transcript event presents as a quoteable message card.
///
/// Ordinary assistant prose presents itself. Streamed prose and a tool-call
/// intent present only when they carry an acknowledgement: those are the
/// transcript shapes the resident message surface promotes into prose cards.
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
        }) if steer::has_acknowledgement(intent) => Some(std::borrow::Cow::Owned(listen::Prose {
            role: listen::ProseRole::Assistant,
            text: intent.clone(),
            phase: None,
            media: Vec::new(),
            external_media: Vec::new(),
            locus: *locus,
        })),
        _ => None,
    }
}
