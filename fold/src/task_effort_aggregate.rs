//! Available-phase aggregation for the task-effort fold.

use std::collections::BTreeMap;

use wire::Phase;
use wire::intent::{
    EffortAggregateAnswer, EffortAvailability, EffortIdentity, EffortPhaseAnswer, EffortUsage,
};

use super::{add_usage, divide_usage, zero_usage};

pub(super) fn aggregate(
    phases: &[EffortPhaseAnswer],
) -> Result<Vec<EffortAggregateAnswer>, String> {
    let mut groups = BTreeMap::<(EffortIdentity, Phase), Aggregate>::new();
    for phase in phases {
        let EffortAvailability::Available {
            identity,
            usage,
            wall_ms,
        } = &phase.availability
        else {
            continue;
        };
        groups
            .entry((identity.clone(), phase.phase.clone()))
            .or_insert_with(Aggregate::new)
            .add(usage, *wall_ms)?;
    }
    groups
        .into_iter()
        .map(|((identity, phase), total)| total.answer(identity, phase))
        .collect()
}

#[derive(Debug)]
struct Aggregate {
    count: u64,
    usage: EffortUsage,
    wall_ms: u64,
}

impl Aggregate {
    fn new() -> Self {
        Self {
            count: 0,
            usage: zero_usage(),
            wall_ms: 0,
        }
    }

    fn add(&mut self, usage: &EffortUsage, wall_ms: u64) -> Result<(), String> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or_else(|| "task::effort aggregate count overflow".to_owned())?;
        self.usage = add_usage(&self.usage, usage)
            .ok_or_else(|| "task::effort aggregate usage overflow".to_owned())?;
        self.wall_ms = self
            .wall_ms
            .checked_add(wall_ms)
            .ok_or_else(|| "task::effort aggregate wall time overflow".to_owned())?;
        Ok(())
    }

    fn answer(
        self,
        identity: EffortIdentity,
        phase: Phase,
    ) -> Result<EffortAggregateAnswer, String> {
        if self.count == 0 {
            return Err("task::effort aggregate has zero members".to_owned());
        }
        Ok(EffortAggregateAnswer {
            identity,
            phase,
            count: self.count,
            usage_average: divide_usage(&self.usage, self.count),
            wall_ms_average: self.wall_ms / self.count,
            usage_sum: self.usage,
            wall_ms_sum: self.wall_ms,
        })
    }
}
