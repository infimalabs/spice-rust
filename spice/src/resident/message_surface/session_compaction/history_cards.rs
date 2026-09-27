//! Preserved compaction cards shared by every history consumer of one lane.
//!
//! Each window chains back from the frozen source end, so a card's
//! presentation, including the tool terminals left pending inside its window,
//! is fixed by the source rather than by which anchor asked first. Consumers
//! paging at different positions select their own pages from one decoded
//! suffix, and a region any of them has read is never decoded again.

use super::*;

/// Compaction cards decoded from one frozen preserved-session prefix.
#[derive(Debug, Default)]
pub(in crate::resident::message_surface) struct SessionCards {
    /// Every record in `[start, end)` is decoded; `None` before the first window.
    start: Option<u64>,
    /// Decoded cards in source order.
    cards: Vec<wire::AgentMessage>,
    bytes: usize,
}

impl SessionCards {
    /// Heap and inline bytes the decoded suffix retains.
    pub(in crate::resident::message_surface) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Forget the decoded suffix; the next page reads from the source end.
    pub(in crate::resident::message_surface) fn clear(&mut self) {
        *self = Self::default();
    }

    fn start(&self, end: &HistoryCheckpoint) -> u64 {
        self.start.unwrap_or_else(|| end.next())
    }

    fn eligible<'a>(
        &'a self,
        before: Option<&'a wire::AgentMessage>,
    ) -> impl Iterator<Item = &'a wire::AgentMessage> {
        self.cards.iter().filter(move |card| {
            before.is_none_or(|before| card.chronology_key() < before.chronology_key())
        })
    }

    fn prepend(&mut self, window: HistoryScan) {
        self.bytes += window.messages.iter().map(card_bytes).sum::<usize>();
        let mut cards = window.messages;
        cards.append(&mut self.cards);
        self.cards = cards;
        self.start = Some(window.start);
    }

    fn page(
        &self,
        end: &HistoryCheckpoint,
        before: Option<&wire::AgentMessage>,
        limit: usize,
        work: HistoryWork,
    ) -> HistoryScan {
        let eligible = self.eligible(before).collect::<Vec<_>>();
        let first = eligible.len().saturating_sub(limit);
        let start = self.start(end);
        HistoryScan {
            messages: eligible[first..]
                .iter()
                .map(|card| (*card).clone())
                .collect(),
            more: first > 0 || start > 0,
            start,
            work,
        }
    }
}

fn card_bytes(card: &wire::AgentMessage) -> usize {
    std::mem::size_of::<wire::AgentMessage>()
        + card.text.len()
        + card
            .unavailable_tool_terminals
            .iter()
            .map(String::len)
            .sum::<usize>()
}

pub(in crate::resident::message_surface) struct HistoryScan {
    pub(in crate::resident::message_surface) messages: Vec<wire::AgentMessage>,
    pub(in crate::resident::message_surface) more: bool,
    pub(in crate::resident::message_surface) start: u64,
    pub(in crate::resident::message_surface) work: HistoryWork,
}

pub(in crate::resident::message_surface) struct HistoryRequest<'a> {
    pub(in crate::resident::message_surface) path: PathBuf,
    pub(in crate::resident::message_surface) worktree: PathBuf,
    pub(in crate::resident::message_surface) dialect: listen::Dialect,
    pub(in crate::resident::message_surface) thread: String,
    pub(in crate::resident::message_surface) lane: CanonicalUuid,
    pub(in crate::resident::message_surface) author: CanonicalUuid,
    pub(in crate::resident::message_surface) before: Option<&'a wire::AgentMessage>,
    /// The frozen source end every window chains back from.
    pub(in crate::resident::message_surface) end: &'a HistoryCheckpoint,
    pub(in crate::resident::message_surface) limit: usize,
}

/// Serve the newest `limit` cards before the request anchor, decoding older
/// windows into `cards` only while the decoded suffix holds too few.
///
/// Extension is bounded the way the tape half is, by [`HISTORY_WINDOW_LIMIT`]
/// windows and [`HISTORY_DURATION`] of scan time; a page still short keeps
/// `more`, and the next request resumes behind the suffix this one decoded.
pub(in crate::resident::message_surface) fn retained_messages(
    request: HistoryRequest<'_>,
    cards: &mut SessionCards,
) -> CommandResult<HistoryScan> {
    admit_history_source(&request.path, request.end)
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    let mut cursor = None;
    let mut work = HistoryWork::default();
    let mut reads = 0;
    while cards.eligible(request.before).count() < request.limit
        && cards.start(request.end) > 0
        && reads < HISTORY_WINDOW_LIMIT
        && work.elapsed < HISTORY_DURATION
    {
        let cursor = cursor.get_or_insert_with(|| {
            Cursor::new(
                request.thread.clone(),
                request.path.clone(),
                request.worktree.clone(),
                request.dialect,
                request.author,
            )
        });
        let end = request.end.retreat(cards.start(request.end));
        let window = history_window(cursor, &request, &end)?;
        work = work.joined(window.work);
        reads += 1;
        cards.prepend(window);
    }
    Ok(cards.page(request.end, request.before, request.limit, work))
}

/// Read one window ending at `end` and keep every card it holds.
pub(super) fn history_window(
    cursor: &mut Cursor,
    request: &HistoryRequest<'_>,
    end: &HistoryCheckpoint,
) -> CommandResult<HistoryScan> {
    let page = bootstrap_before(cursor, 0, end, HistoryRead::Page)
        .map_err(|error| store_fault("canvas::message-history-unavailable", &error.to_string()))?;
    let messages = HistoryMessages::default();
    // Preserved-session loci name no tape batch, so observations never apply.
    let mut state = MessageState::default();
    for record in page.records {
        publish(
            record,
            request.lane,
            request.author,
            &request.worktree,
            &messages,
            &mut state,
        )?;
    }
    let messages = messages.0.into_inner().map_err(|_| {
        store_fault(
            "canvas::message-history-unavailable",
            "history lock poisoned",
        )
    })?;
    Ok(HistoryScan {
        messages,
        more: page.start > 0,
        start: page.start,
        work: page.work,
    })
}

#[derive(Default)]
struct HistoryMessages(std::sync::Mutex<Vec<wire::AgentMessage>>);

impl AgentMessagePublisher for HistoryMessages {
    fn publish(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        self.0
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?
            .push(message);
        Ok(true)
    }

    fn revise_final(
        &self,
        message: wire::AgentMessage,
    ) -> Result<bool, canvas::stream::MessagePublishError> {
        let mut collected = self
            .0
            .lock()
            .map_err(|_| canvas::stream::MessagePublishError::Unavailable)?;
        let Some(existing) = collected
            .iter_mut()
            .find(|existing| existing.lane == message.lane && existing.locus == message.locus)
        else {
            return Err(canvas::stream::MessagePublishError::IdentityConflict);
        };
        *existing = message;
        Ok(true)
    }
}
