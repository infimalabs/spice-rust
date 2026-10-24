//! Current native work that makes a lane listen to a room.

use std::collections::BTreeMap;

use wire::{CanonicalUuid, Seq, TaskStamp};

/// Current native work associated with one room and held by one lane.
///
/// Construction is derived from the current task/preparation and active-claim
/// projection. It is an input reference, not a claim or membership grant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomWork {
    pub(super) room: TaskStamp,
    pub(super) task: TaskStamp,
    pub(super) lane: CanonicalUuid,
    pub(super) since: Seq,
}

impl RoomWork {
    /// Derive relevant current work from one native task-head bundle.
    ///
    /// The anchor itself and tasks whose explicit preparation relationship
    /// names the anchor are relevant only while their claim remains active.
    #[must_use]
    pub fn from_task(room: &TaskStamp, task: &board::Entry) -> Option<Self> {
        let statement = task.statement();
        let related = task.born() == room
            || statement.preparation.as_ref().is_some_and(|preparation| {
                preparation.parent == *room
                    || preparation.controller == *room
                    || preparation.construction == *room
            });
        let claim = task
            .claim()
            .filter(|claim| claim.disposition == wire::ClaimDisposition::Active)?;
        related.then(|| Self {
            room: room.clone(),
            task: task.born().clone(),
            lane: claim.lane,
            since: claim.claimed.max(statement.queue_origin),
        })
    }

    /// The lane whose active claim makes this work current.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// Derive relevant current work for every room in one pass over the task
    /// heads, grouped by room. Each actively claimed task names the rooms it
    /// is relevant to, so a room's rows equal what [`Self::from_task`] yields
    /// for it over the same entries, in the same order.
    #[must_use]
    pub fn from_tasks(tasks: &[board::Entry]) -> BTreeMap<TaskStamp, Vec<Self>> {
        let mut rooms = BTreeMap::<TaskStamp, Vec<Self>>::new();
        for task in tasks {
            let Some(claim) = task
                .claim()
                .filter(|claim| claim.disposition == wire::ClaimDisposition::Active)
            else {
                continue;
            };
            let statement = task.statement();
            let since = claim.claimed.max(statement.queue_origin);
            let mut related = [Some(task.born()), None, None, None];
            if let Some(preparation) = statement.preparation.as_ref() {
                related[1] = Some(&preparation.parent);
                related[2] = Some(&preparation.controller);
                related[3] = Some(&preparation.construction);
            }
            for (index, room) in related.iter().enumerate() {
                let Some(room) = room else { continue };
                if related[..index].contains(&Some(room)) {
                    continue;
                }
                rooms.entry((*room).clone()).or_default().push(Self {
                    room: (*room).clone(),
                    task: task.born().clone(),
                    lane: claim.lane,
                    since,
                });
            }
        }
        rooms
    }
}
