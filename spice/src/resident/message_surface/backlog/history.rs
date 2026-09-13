use super::*;

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
        retain_covered(&mut visible, self.tape.as_ref(), self.session.as_ref());
        visible.sort_by_key(wire::AgentMessage::chronology_key);
        visible.dedup_by(|right, left| right.locus == left.locus);
        let end = before.map_or(visible.len(), |before| {
            visible.partition_point(|message| message.chronology_key() < before.chronology_key())
        });
        let start = end.saturating_sub(limit);
        (visible[start..end].to_vec(), more || start > 0)
    }
}

fn retain_covered(
    visible: &mut Vec<wire::AgentMessage>,
    tape: Option<&TapeHistoryScan>,
    session: Option<&session_compaction::HistoryScan>,
) {
    let floors = [
        tape.and_then(|scan| {
            scan.more.then(|| {
                scan.messages
                    .iter()
                    .map(wire::AgentMessage::chronology_key)
                    .min()
            })
        }),
        session.and_then(|scan| {
            scan.more.then(|| {
                scan.messages
                    .iter()
                    .map(wire::AgentMessage::chronology_key)
                    .min()
            })
        }),
    ];
    if floors.iter().any(Option::is_some) {
        match floors.into_iter().flatten().flatten().max() {
            Some(floor) => visible.retain(|message| message.chronology_key() >= floor),
            None => visible.clear(),
        }
    }
}
