//! Bounded browser-owned admission for actions awaiting socket service.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use futures_channel::mpsc;

use crate::client_action::ClientAction;

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

    pub(super) fn lease(&self) -> Option<ActionLease> {
        let action = self.mailbox.borrow_mut().lease()?;
        Some(ActionLease {
            queue: self.clone(),
            action: Some(action),
        })
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
    history: Option<(wire::CanonicalUuid, wire::MessageLocus)>,
    class: ActionClass,
}

struct LeasedAction {
    index: usize,
    payload_bytes: usize,
    history: Option<(wire::CanonicalUuid, wire::MessageLocus)>,
    class: ActionClass,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActionClass {
    Refresh,
    DrainSubmissions,
    FastMode,
    Other,
}

pub(super) struct ActionLease {
    queue: ActionQueue,
    action: Option<ClientAction>,
}

impl ActionLease {
    pub(super) fn action(&self) -> &ClientAction {
        self.action
            .as_ref()
            .expect("an unsettled lease owns its action")
    }

    pub(super) fn commit(mut self) -> ClientAction {
        let action = self.action.take().expect("an unsettled lease commits");
        self.queue.mailbox.borrow_mut().commit();
        action
    }

    pub(super) fn restore(mut self) {
        self.restore_inner();
    }

    fn restore_inner(&mut self) {
        if let Some(action) = self.action.take() {
            self.queue.mailbox.borrow_mut().restore(action);
        }
    }
}

impl Drop for ActionLease {
    fn drop(&mut self) {
        self.restore_inner();
    }
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
        let class = ActionClass::of(&action);
        if matches!(class, ActionClass::Refresh | ActionClass::DrainSubmissions)
            && self
                .records
                .iter()
                .map(|queued| queued.class)
                .chain(self.leased.iter().map(|leased| leased.class))
                .any(|retained| retained == class)
        {
            return Ok(());
        }
        if class == ActionClass::FastMode
            && let Some(queued) = self
                .records
                .iter_mut()
                .find(|queued| queued.class == ActionClass::FastMode)
        {
            queued.action = action;
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
            history,
            class,
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
        self.leased = Some(LeasedAction {
            index,
            payload_bytes: queued.payload_bytes,
            history: queued.history,
            class: queued.class,
        });
        Some(queued.action)
    }

    fn commit(&mut self) {
        let leased = self.leased.take().expect("one leased action commits");
        self.release_accounting(&leased);
        self.mutation_turn = !self.mutation_turn;
    }

    fn restore(&mut self, action: ClientAction) {
        let leased = self.leased.take().expect("one leased action restores");
        assert!(
            ActionClass::of(&action) == leased.class,
            "a lease restores its exact action class"
        );
        if leased.class == ActionClass::FastMode
            && self
                .records
                .iter()
                .any(|queued| queued.class == ActionClass::FastMode)
        {
            self.release_accounting(&leased);
            return;
        }
        let index = leased.index.min(self.records.len());
        self.records.insert(
            index,
            QueuedAction {
                action,
                payload_bytes: leased.payload_bytes,
                history: leased.history,
                class: leased.class,
            },
        );
    }

    fn release_accounting(&mut self, leased: &LeasedAction) {
        if let Some(history) = leased.history {
            self.history.remove(&history);
        }
        self.payload_bytes = self.payload_bytes.saturating_sub(leased.payload_bytes);
    }

    fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl ActionClass {
    fn of(action: &ClientAction) -> Self {
        match action {
            ClientAction::Refresh => Self::Refresh,
            ClientAction::DrainSubmissions => Self::DrainSubmissions,
            ClientAction::FastMode(_) => Self::FastMode,
            _ => Self::Other,
        }
    }
}
