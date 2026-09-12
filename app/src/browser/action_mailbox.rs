//! Bounded browser-owned admission for actions awaiting socket service.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use futures_channel::mpsc;

use super::ClientAction;

const ACTION_RECORD_LIMIT: usize = 64;
const HISTORY_RECORD_LIMIT: usize = 32;
const TEXT_ALLOWANCE_BYTES: usize = wire::media::MAX_MEDIA_BYTES;
const MEDIA_URL_BYTES: usize = wire::media::MAX_MEDIA_BYTES.div_ceil(3) * 4 + 64;
const ACTION_PAYLOAD_LIMIT: usize =
    wire::media::MAX_MEDIA_ITEMS * MEDIA_URL_BYTES + TEXT_ALLOWANCE_BYTES;

#[derive(Clone)]
pub(super) struct ActionQueue {
    mailbox: Rc<RefCell<ActionMailbox>>,
    wake: Rc<RefCell<mpsc::Sender<()>>>,
}

impl ActionQueue {
    pub(super) fn new() -> (Self, mpsc::Receiver<()>) {
        let (wake, receiver) = mpsc::channel(1);
        (
            Self {
                mailbox: Rc::new(RefCell::new(ActionMailbox::default())),
                wake: Rc::new(RefCell::new(wake)),
            },
            receiver,
        )
    }

    pub(super) fn admit(&self, action: ClientAction) -> Result<(), ActionAdmissionError> {
        if self.wake.borrow().is_closed() {
            return Err(ActionAdmissionError::Unavailable);
        }
        self.mailbox.borrow_mut().admit(action)?;
        self.rearm()
    }

    pub(super) fn lease(&self) -> Option<ClientAction> {
        self.mailbox.borrow_mut().lease()
    }

    pub(super) fn commit(&self) {
        self.mailbox.borrow_mut().commit();
    }

    pub(super) fn restore(&self) {
        self.mailbox.borrow_mut().restore();
    }

    pub(super) fn rearm(&self) -> Result<(), ActionAdmissionError> {
        if self.mailbox.borrow().is_empty() {
            return Ok(());
        }
        match self.wake.borrow_mut().try_send(()) {
            Ok(()) => Ok(()),
            Err(error) if error.is_full() => Ok(()),
            Err(_) => Err(ActionAdmissionError::Unavailable),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActionAdmissionError {
    Unavailable,
    Records,
    Payload,
    History,
}

impl std::fmt::Display for ActionAdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("Canvas action delivery is unavailable."),
            Self::Records => write!(
                formatter,
                "Canvas already retains {ACTION_RECORD_LIMIT} unsent actions; wait for delivery before retrying."
            ),
            Self::Payload => write!(
                formatter,
                "Canvas unsent action payloads exceed the {ACTION_PAYLOAD_LIMIT}-byte limit."
            ),
            Self::History => write!(
                formatter,
                "Canvas already retains {HISTORY_RECORD_LIMIT} distinct history requests."
            ),
        }
    }
}

#[derive(Default)]
struct ActionMailbox {
    records: VecDeque<QueuedAction>,
    leased: Option<LeasedAction>,
    history: HashSet<(wire::CanonicalUuid, wire::MessageLocus)>,
    payload_bytes: usize,
    mutation_turn: bool,
}

struct QueuedAction {
    action: ClientAction,
    payload_bytes: usize,
}

struct LeasedAction {
    queued: QueuedAction,
    index: usize,
}

impl ClientAction {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::Seat {
                name,
                worktree,
                life,
                vendor,
                ..
            } => {
                name.as_ref().map_or(0, String::len)
                    + worktree.len()
                    + life.len()
                    + vendor.as_str().len()
            }
            Self::ArrangeLineages { lineages, .. } => lineages.iter().fold(
                lineages.len() * std::mem::size_of::<wire::intent::LineageArrangement>(),
                |bytes, lineage| {
                    bytes.saturating_add(
                        lineage.lanes.len() * std::mem::size_of::<wire::CanonicalUuid>(),
                    )
                },
            ),
            Self::Note { task, note, .. } => task.as_str().len().saturating_add(note.len()),
            Self::Steer {
                message,
                attachments,
                ..
            } => attachments.iter().fold(message.len(), |bytes, attachment| {
                bytes
                    .saturating_add(attachment.name.len())
                    .saturating_add(attachment.content_type.len())
                    .saturating_add(attachment.data_url.len())
            }),
            Self::DrainSubmissions
            | Self::Refresh
            | Self::FastMode(_)
            | Self::MessageHistory { .. }
            | Self::ArrangeLane { .. }
            | Self::RenewLane { .. }
            | Self::ConfigureLineage { .. } => 0,
        }
    }

    pub(super) const fn is_mutation(&self) -> bool {
        !matches!(self, Self::Refresh | Self::MessageHistory { .. })
    }
}

impl ActionMailbox {
    fn admit(&mut self, action: ClientAction) -> Result<(), ActionAdmissionError> {
        let singleton = matches!(
            action,
            ClientAction::Refresh | ClientAction::DrainSubmissions
        );
        if singleton
            && self
                .records
                .iter()
                .chain(self.leased.iter().map(|leased| &leased.queued))
                .any(|queued| {
                    matches!(
                        (&queued.action, &action),
                        (ClientAction::Refresh, ClientAction::Refresh)
                            | (
                                ClientAction::DrainSubmissions,
                                ClientAction::DrainSubmissions
                            )
                    )
                })
        {
            return Ok(());
        }
        if let ClientAction::FastMode(value) = &action
            && let Some(queued) = self
                .records
                .iter_mut()
                .find(|queued| matches!(queued.action, ClientAction::FastMode(_)))
        {
            queued.action = ClientAction::FastMode(*value);
            return Ok(());
        }
        let history = match &action {
            ClientAction::MessageHistory { lane, before } => Some((*lane, *before)),
            _ => None,
        };
        if history.is_some_and(|key| self.history.contains(&key)) {
            return Ok(());
        }
        if history.is_some() && self.history.len() >= HISTORY_RECORD_LIMIT {
            return Err(ActionAdmissionError::History);
        }
        if self.records.len() + usize::from(self.leased.is_some()) >= ACTION_RECORD_LIMIT {
            return Err(ActionAdmissionError::Records);
        }
        let payload_bytes = action.payload_bytes();
        if payload_bytes > ACTION_PAYLOAD_LIMIT.saturating_sub(self.payload_bytes) {
            return Err(ActionAdmissionError::Payload);
        }
        if let Some(key) = history {
            self.history.insert(key);
        }
        self.payload_bytes = self.payload_bytes.saturating_add(payload_bytes);
        self.records.push_back(QueuedAction {
            action,
            payload_bytes,
        });
        Ok(())
    }

    fn lease(&mut self) -> Option<ClientAction> {
        assert!(self.leased.is_none(), "only one action may be leased");
        let index = self
            .records
            .iter()
            .position(|queued| queued.action.is_mutation() == self.mutation_turn)
            .unwrap_or(0);
        let queued = self.records.remove(index)?;
        let action = queued.action.clone();
        self.leased = Some(LeasedAction { queued, index });
        Some(action)
    }

    fn commit(&mut self) {
        let leased = self.leased.take().expect("one leased action commits");
        self.release_accounting(&leased.queued);
        self.mutation_turn = !self.mutation_turn;
    }

    fn restore(&mut self) {
        let leased = self.leased.take().expect("one leased action restores");
        if matches!(leased.queued.action, ClientAction::FastMode(_))
            && self
                .records
                .iter()
                .any(|queued| matches!(queued.action, ClientAction::FastMode(_)))
        {
            self.release_accounting(&leased.queued);
            return;
        }
        self.records
            .insert(leased.index.min(self.records.len()), leased.queued);
    }

    fn release_accounting(&mut self, queued: &QueuedAction) {
        if let ClientAction::MessageHistory { lane, before } = &queued.action {
            self.history.remove(&(*lane, *before));
        }
        self.payload_bytes = self.payload_bytes.saturating_sub(queued.payload_bytes);
    }

    fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
