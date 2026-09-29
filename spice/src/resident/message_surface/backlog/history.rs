use super::*;

impl DurableHistory {
    /// Chain older windows behind `newest` until the page holds `limit` visible
    /// cards or the tape begins.
    ///
    /// A tool-heavy lane can run for megabytes without one visible card, so a
    /// single window cannot promise a page. The request keeps reading the way
    /// the predecessor did, bounded by [`HISTORY_READ_LIMIT`] windows and
    /// [`HISTORY_DURATION`] of scan time; a page still short keeps `more` and
    /// its continuation so the next request resumes where this one stopped.
    pub(super) fn fill_tape_messages(
        &self,
        lane: CanonicalUuid,
        source: &DurableLane,
        before: Option<&wire::AgentMessage>,
        limit: usize,
        newest: TapeHistoryScan,
    ) -> Result<TapeHistoryScan, String> {
        let mut scan = newest;
        let mut reads = 1;
        while scan.messages.len() < limit
            && scan.start.next != listen::Position::start()
            && reads < HISTORY_READ_LIMIT
            && scan.work.duration < HISTORY_DURATION
        {
            let tail = history_tail(&source.tape, source.dialect, &scan.start)
                .map_err(history_tail_error)?;
            let remaining = limit - scan.messages.len();
            let older = self.collect_tape_messages(lane, source, before, remaining, tail)?;
            reads += 1;
            scan = older.extend_newer(scan);
        }
        Ok(scan)
    }
}

impl TapeHistoryScan {
    /// Join `newer`, the window read immediately after this one, onto it.
    ///
    /// `more` and `complete` stay as this older window judged them: the newer
    /// window was only chained onto because it held every card it found and
    /// only claimed more because this one had not been read yet.
    fn extend_newer(mut self, newer: Self) -> Self {
        debug_assert!(self.end.next <= newer.start.next);
        self.messages.extend(newer.messages);
        self.end = newer.end;
        self.work.source_bytes = self
            .work
            .source_bytes
            .saturating_add(newer.work.source_bytes);
        self.work.records = self.work.records.saturating_add(newer.work.records);
        self.work.passes = self.work.passes.saturating_add(newer.work.passes);
        self.work.duration = self.work.duration.saturating_add(newer.work.duration);
        self
    }
}

pub(super) fn history_newest(
    tape: &Path,
    dialect: listen::Dialect,
) -> Result<listen::TailScan, listen::ScanError> {
    listen::scan_tail(tape, dialect, history_budget())
}

pub(super) fn history_tail(
    tape: &Path,
    dialect: listen::Dialect,
    before: &listen::Checkpoint,
) -> Result<listen::TailScan, listen::ScanError> {
    listen::scan_before(tape, dialect, before, history_budget())
}

pub(super) fn history_tail_error(error: listen::ScanError) -> String {
    if matches!(error, listen::ScanError::SourceChanged { .. }) {
        "the transcript source changed after Canvas bootstrap".to_owned()
    } else {
        error.to_string()
    }
}

pub(super) struct HistorySources {
    pub(super) tape: Option<TapeHistoryScan>,
    pub(super) session: Option<session_compaction::HistoryScan>,
}

impl HistorySources {
    pub(super) fn page(
        &self,
        before: Option<&wire::AgentMessage>,
        limit: usize,
    ) -> (Vec<wire::AgentMessage>, bool) {
        let mut visible = self
            .tape
            .as_ref()
            .map_or_else(Vec::new, |tape| tape.messages.clone());
        let mut more = self.tape.as_ref().is_some_and(|tape| tape.more);
        if let Some(session) = &self.session {
            visible.extend(session.messages.clone());
            more |= session.more;
        }
        let tape = self
            .tape
            .as_ref()
            .map(|scan| (scan.more, scan.messages.as_slice()));
        let session = self
            .session
            .as_ref()
            .map(|scan| (scan.more, scan.messages.as_slice()));
        retain_covered(&mut visible, tape.into_iter().chain(session));
        visible.sort_by_key(wire::AgentMessage::chronology_key);
        visible.dedup_by(|right, left| right.locus == left.locus);
        reconcile_boundaries(&mut visible);
        let end = before.map_or(visible.len(), |before| {
            visible.partition_point(|message| message.chronology_key() < before.chronology_key())
        });
        let start = end.saturating_sub(limit);
        (visible[start..end].to_vec(), more || start > 0)
    }
}

/// Drop each captured announcement whose preserved record this page also
/// carries, so one compaction is one boundary however the window read it.
fn reconcile_boundaries(visible: &mut Vec<wire::AgentMessage>) {
    for position in wire::superseded_boundaries(visible).into_iter().rev() {
        visible.remove(position);
    }
}

/// Keep only cards every source with more to read has proven complete.
///
/// A source proves the range from its oldest card up to the anchor. One that
/// still has more to read yet found no card proves nothing, so the page stays
/// empty and the next request resumes from both sources' progress instead of
/// letting the other source's older cards step past it.
pub(super) fn retain_covered<'a>(
    visible: &mut Vec<wire::AgentMessage>,
    sources: impl IntoIterator<Item = (bool, &'a [wire::AgentMessage])>,
) {
    let mut floor = None;
    for (more, messages) in sources {
        if !more {
            continue;
        }
        let Some(proven) = messages
            .iter()
            .map(wire::AgentMessage::chronology_key)
            .min()
        else {
            visible.clear();
            return;
        };
        floor = floor.max(Some(proven));
    }
    if let Some(floor) = floor {
        visible.retain(|message| message.chronology_key() >= floor);
    }
}
