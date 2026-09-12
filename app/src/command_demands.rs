//! Ordered browser command submissions retained across socket replacement.
//!
//! This queue admits at most one operator mutation at a time into Canvas's
//! bounded global command scheduler; it does not replace or bypass that
//! scheduler's ownership and source-fairness rules.

use std::collections::VecDeque;

use wire::{CanonicalUuid, SeqRange};

const SETTLED_LIMIT: usize = 32;

/// How far one operator mutation has crossed its command protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DemandStatus {
    Submitting,
    Admitted(SeqRange),
    Committed(SeqRange),
    Refused(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DemandResponse {
    Committed(SeqRange),
    Refused(String),
}

pub(crate) fn classify_demand_outcome(
    id: CanonicalUuid,
    outcome: &wire::Outcome<serde_json::Value>,
) -> Result<DemandResponse, String> {
    match outcome {
        wire::Outcome::Reply(reply) => match reply.position {
            wire::ReplyPosition::Committed(range) => Ok(DemandResponse::Committed(range)),
            wire::ReplyPosition::Snapshot(_) | wire::ReplyPosition::Direct {} => Err(format!(
                "mutating command demand {id} returned a non-committed position"
            )),
        },
        wire::Outcome::Refusal(refusal) => Ok(DemandResponse::Refused(format!(
            "{}: {}",
            refusal.reason(),
            refusal.explanation()
        ))),
        wire::Outcome::Fault(fault) => Ok(DemandResponse::Refused(format!(
            "{}: {}",
            fault.reason, fault.detail
        ))),
    }
}

/// One client-identified mutation and the exact body its retry must preserve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Demand<T> {
    pub(crate) id: CanonicalUuid,
    pub(crate) body: T,
    pub(crate) status: DemandStatus,
    pub(crate) prepared: bool,
    pub(crate) in_flight: bool,
}

/// FIFO mutation owner shared by consecutive socket attempts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DemandQueue<T> {
    pending: VecDeque<Demand<T>>,
    settled: VecDeque<Demand<T>>,
    awaiting_authority: bool,
    rebase_ready: bool,
    rebase_on_authority: bool,
}

impl<T> Default for DemandQueue<T> {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            settled: VecDeque::new(),
            awaiting_authority: false,
            rebase_ready: false,
            rebase_on_authority: false,
        }
    }
}

impl<T> DemandQueue<T> {
    pub(crate) fn accepts(&self, id: CanonicalUuid) -> Result<(), String> {
        if self
            .pending
            .iter()
            .chain(&self.settled)
            .any(|demand| demand.id == id)
        {
            return Err(format!("command demand identity {id} is already known"));
        }
        Ok(())
    }

    pub(crate) fn submit(&mut self, id: CanonicalUuid, body: T) -> Result<(), String> {
        self.accepts(id)?;
        self.pending.push_back(Demand {
            id,
            body,
            status: DemandStatus::Submitting,
            prepared: false,
            in_flight: false,
        });
        Ok(())
    }

    pub(crate) fn ready(&self) -> Option<&Demand<T>> {
        if self.awaiting_authority {
            return None;
        }
        self.pending
            .front()
            .filter(|demand| demand.status == DemandStatus::Submitting && !demand.in_flight)
    }

    pub(crate) fn await_authority(&mut self, rebase_next: bool) {
        self.awaiting_authority = true;
        self.rebase_on_authority = rebase_next;
    }

    pub(crate) fn observe_authority(&mut self) {
        self.awaiting_authority = false;
        self.rebase_ready = self.rebase_on_authority && !self.pending.is_empty();
        self.rebase_on_authority = false;
    }

    /// Bind server-observed fields once, preserving them for every later replay.
    pub(crate) fn prepare_ready(
        &mut self,
        prepare: impl FnOnce(&mut T, bool),
    ) -> Option<&Demand<T>> {
        let demand = self
            .pending
            .front_mut()
            .filter(|demand| demand.status == DemandStatus::Submitting)?;
        if !demand.prepared {
            prepare(&mut demand.body, self.rebase_ready);
            demand.prepared = true;
            self.rebase_ready = false;
        }
        Some(demand)
    }

    pub(crate) fn sent(&mut self, id: CanonicalUuid) -> Result<(), String> {
        let demand = self.front_mut(id)?;
        if demand.status != DemandStatus::Submitting || demand.in_flight {
            return Err(format!(
                "command demand {id} was not ready when its transport completed"
            ));
        }
        demand.in_flight = true;
        Ok(())
    }

    pub(crate) fn admit(&mut self, id: CanonicalUuid, range: SeqRange) -> Result<(), String> {
        let demand = self.front_mut(id)?;
        if demand.status != DemandStatus::Submitting || !demand.in_flight {
            return Err(format!(
                "command demand {id} was not in flight when admitted"
            ));
        }
        demand.status = DemandStatus::Admitted(range);
        Ok(())
    }

    pub(crate) fn commit(
        &mut self,
        id: CanonicalUuid,
        range: SeqRange,
        rebase_next: bool,
    ) -> Result<(), String> {
        let demand = self.front_mut(id)?;
        if demand.status != DemandStatus::Admitted(range) {
            return Err(format!(
                "command demand {id} committed a range other than its admission"
            ));
        }
        self.settle(id, DemandStatus::Committed(range))?;
        self.rebase_ready = rebase_next && !self.pending.is_empty();
        Ok(())
    }

    pub(crate) fn refuse(&mut self, id: CanonicalUuid, issue: String) -> Result<(), String> {
        let demand = self.front_mut(id)?;
        if demand.status != DemandStatus::Submitting || !demand.in_flight {
            return Err(format!(
                "command demand {id} was not in flight when refused"
            ));
        }
        self.settle(id, DemandStatus::Refused(issue))?;
        self.rebase_ready = false;
        Ok(())
    }

    /// Make a result-less write eligible for exact replay on the next socket.
    pub(crate) fn reconnect(&mut self) {
        if let Some(demand) = self.pending.front_mut() {
            if matches!(demand.status, DemandStatus::Admitted(_)) {
                demand.status = DemandStatus::Submitting;
            }
            demand.in_flight = false;
        }
    }

    #[cfg(test)]
    fn status(&self, id: CanonicalUuid) -> Option<&DemandStatus> {
        self.pending
            .iter()
            .chain(&self.settled)
            .find(|demand| demand.id == id)
            .map(|demand| &demand.status)
    }

    fn settle(&mut self, id: CanonicalUuid, status: DemandStatus) -> Result<(), String> {
        let mut demand = self
            .pending
            .pop_front()
            .ok_or_else(|| format!("command result named unknown demand {id}"))?;
        if demand.id != id {
            self.pending.push_front(demand);
            return Err(format!(
                "command result named demand {id} before the queue front"
            ));
        }
        if !matches!(demand.status, DemandStatus::Admitted(_))
            && !matches!(status, DemandStatus::Refused(_))
        {
            self.pending.push_front(demand);
            return Err(format!(
                "command demand {id} received a terminal result before admission"
            ));
        }
        demand.status = status;
        self.settled.push_back(demand);
        if self.settled.len() > SETTLED_LIMIT {
            self.settled.pop_front();
        }
        Ok(())
    }

    fn front_mut(&mut self, id: CanonicalUuid) -> Result<&mut Demand<T>, String> {
        let demand = self
            .pending
            .front_mut()
            .ok_or_else(|| format!("command demand {id} is not pending"))?;
        if demand.id != id {
            return Err(format!("command demand {id} is not the queue front"));
        }
        Ok(demand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(position: wire::ReplyPosition) -> wire::Outcome<serde_json::Value> {
        wire::Outcome::Reply(wire::Reply {
            position,
            body: serde_json::Value::Null,
        })
    }

    fn id(suffix: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{suffix:02x}")
            .parse()
            .expect("fixture identity")
    }

    fn range(position: u64) -> SeqRange {
        let position = wire::Seq::new(position).expect("positive fixture position");
        SeqRange {
            first: position,
            last: position,
        }
    }

    #[test]
    fn queue_serializes_and_correlates_terminal_results() {
        let mut queue = DemandQueue::default();
        queue.submit(id(1), "drain").expect("first demand");
        queue.submit(id(2), "steer").expect("second demand");
        queue.sent(id(1)).expect("first sent");
        assert!(queue.ready().is_none());
        assert!(queue.admit(id(2), range(2)).is_err());
        queue.admit(id(1), range(1)).expect("first admitted");
        queue.reconnect();
        assert_eq!(
            queue.ready().map(|row| (row.id, row.body)),
            Some((id(1), "drain"))
        );
        queue.sent(id(1)).expect("replayed first sent");
        queue
            .admit(id(1), range(1))
            .expect("replayed first admitted");
        assert!(queue.commit(id(1), range(2), true).is_err());
        queue
            .commit(id(1), range(1), true)
            .expect("first committed");
        assert_eq!(queue.ready().map(|demand| demand.id), Some(id(2)));
        assert!(queue.admit(id(1), range(1)).is_err());
        assert_eq!(queue.ready().map(|demand| demand.id), Some(id(2)));
        queue
            .prepare_ready(|body, rebase| {
                assert!(rebase);
                *body = "rebased steer";
            })
            .expect("second prepared");
        queue.sent(id(2)).expect("second sent");
        queue
            .refuse(id(2), "stale topology".to_owned())
            .expect("second refused");
        assert_eq!(
            queue.status(id(1)),
            Some(&DemandStatus::Committed(range(1)))
        );
        assert_eq!(
            queue.status(id(2)),
            Some(&DemandStatus::Refused("stale topology".to_owned()))
        );
    }

    #[test]
    fn an_authority_barrier_holds_the_next_command_until_a_fresh_read() {
        let mut queue = DemandQueue::default();
        queue.submit(id(1), "drain").expect("first demand");
        queue.submit(id(2), "steer").expect("second demand");
        queue.sent(id(1)).expect("first sent");
        queue
            .refuse(id(1), "stale topology".to_owned())
            .expect("first refused");
        queue.await_authority(false);
        assert!(queue.ready().is_none());
        queue.observe_authority();
        assert_eq!(queue.ready().map(|demand| demand.id), Some(id(2)));
        queue
            .prepare_ready(|_, rebase| assert!(!rebase))
            .expect("refusal does not rebase the next demand");
    }

    #[test]
    fn committed_refresh_rebases_only_after_authority_arrives() {
        let mut queue = DemandQueue::default();
        queue.submit(id(1), "seat").expect("first demand");
        queue.submit(id(2), "steer").expect("second demand");
        queue.sent(id(1)).expect("first sent");
        queue.admit(id(1), range(1)).expect("first admitted");
        queue
            .commit(id(1), range(1), false)
            .expect("first committed without a lane table");
        queue.await_authority(true);
        assert!(queue.ready().is_none());
        queue.observe_authority();
        queue
            .prepare_ready(|body, rebase| {
                assert!(rebase);
                *body = "rebased steer";
            })
            .expect("second prepared from refreshed authority");
    }

    #[test]
    fn response_classification_preserves_only_committed_ranges() {
        assert_eq!(
            classify_demand_outcome(id(1), &reply(wire::ReplyPosition::Committed(range(1)))),
            Ok(DemandResponse::Committed(range(1)))
        );
        assert!(
            classify_demand_outcome(id(1), &reply(wire::ReplyPosition::Snapshot(range(1).last)))
                .is_err()
        );
        assert!(classify_demand_outcome(id(1), &reply(wire::ReplyPosition::Direct {})).is_err());
        let refusal = wire::Outcome::Refusal(wire::Refusal::bare(wire::RefusalDiagnostic::new(
            "lane::revision-stale",
            None,
            Some("read lanes again".to_owned()),
            None,
        )));
        let fault = wire::Outcome::Fault(wire::Fault {
            class: wire::FaultClass::Protocol,
            reason: "command::invalid".to_owned(),
            detail: "bad body".to_owned(),
        });

        assert_eq!(
            classify_demand_outcome(id(1), &refusal),
            Ok(DemandResponse::Refused(
                "lane::revision-stale: read lanes again".to_owned()
            ))
        );
        assert_eq!(
            classify_demand_outcome(id(1), &fault),
            Ok(DemandResponse::Refused(
                "command::invalid: bad body".to_owned()
            ))
        );
    }
}
