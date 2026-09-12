use super::*;

pub(in crate::board_command) fn guard_capture(
    write: &mut JournalWrite,
    expected: Vec<Witness>,
    observed: AllocationHeads,
    actor: Actor,
    task: TaskHandle,
    projects: ProjectPolicy,
    possession: Possession,
) {
    write.guard(JournalGuard::new(move |read| {
        witnesses_hold(read, &expected)?;
        let heads = current_task_heads(read)?;
        let current = heads
            .iter()
            .map(|task| {
                (
                    TaskHandle::for_project(
                        &task.statement().body().project,
                        &task.statement().body().born,
                    ),
                    task.statement().position(),
                    task.workflow().position(),
                    task.claim().map(journal::JournalHead::position),
                    task.lease().map(journal::JournalHead::position),
                )
            })
            .collect::<AllocationHeads>();
        if current != observed {
            return Err(GuardError::refused(
                "board::witness-moved: capture head set changed",
            ));
        }
        refuse_active_claim_conflict(&heads, &actor)?;
        let entries = heads
            .iter()
            .map(|heads| {
                Entry::new(
                    heads.statement().body().clone(),
                    heads.workflow().body().clone(),
                    heads.claim().map(|head| head.body().clone()),
                    heads.lease().map(|head| head.body().clone()),
                    &projects,
                )
                .map_err(|error| {
                    GuardError::Invalid(format!("capture current heads are invalid: {error}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry = entries
            .iter()
            .find(|entry| entry.handle() == &task)
            .ok_or_else(|| GuardError::refused("board::witness-moved: capture task disappeared"))?;
        let trust = current_trust_heads(read)?;
        let environment = wire::Environment::journal_schema(journal::SCHEMA_VERSION)
            .expect("the compiled journal schema is positive")
            .with_trust_heads(trust.into_iter().map(journal::JournalHead::into_body));
        let eligible = match possession {
            Possession::Capture => board::capturable_entry(&entries, entry, &actor, &environment),
            Possession::Steal => board::stealable_entry(&entries, entry, &actor, &environment),
            Possession::Recover => board::eligible_entry(&entries, entry, &actor, &environment),
            Possession::Next => false,
        };
        if eligible {
            Ok(())
        } else {
            Err(GuardError::refused(
                "board::witness-moved: capture environment changed",
            ))
        }
    }));
}
