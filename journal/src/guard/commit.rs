impl Journal {
    /// Evaluate a typed answer whose own error remains distinct from journal
    /// faults while both are observed inside one immutable SQLite snapshot.
    pub fn read_with<T, E, F>(&self, answer: F) -> Result<T, ReadError<E>>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, E>,
    {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        let read = JournalRead::scoped(&transaction).map_err(ReadError::Journal)?;
        let answer = answer(&read).map_err(ReadError::Answer)?;
        transaction
            .commit()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        Ok(answer)
    }

    /// Evaluate an owned answer against one immutable SQLite snapshot.
    ///
    /// The cursor query is deliberately the first read in the deferred
    /// transaction. It establishes the SQLite snapshot that every subsequent
    /// query through `JournalRead` observes.
    pub fn read<T, F>(&self, answer: F) -> Result<T, GuardError>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, GuardError>,
    {
        self.read_with(answer).map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(error) => error,
        })
    }

    /// Evaluate guards and apply every proposed effect in one immediate SQLite
    /// transaction.
    pub fn apply(&mut self, mut write: JournalWrite) -> Result<Applied, CommitError> {
        if write.facts.is_empty() {
            return Err(CommitError::Refused(GuardRefusal::new(
                "a mutating journal transaction must append at least one fact",
            )));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_fault)?;
        let current = read_cursor(&transaction)?;
        run_guards(&transaction, current, std::mem::take(&mut write.guards))?;
        let assigned = assign_positions(current, write.facts.len())?;
        insert_facts(&transaction, &write, &assigned, current)?;
        apply_heads(&transaction, write.id, write.heads, &assigned)?;
        if let Some(receipt) = write.receipt {
            insert_receipt(
                &transaction,
                &write.actor,
                receipt,
                *assigned.first().expect("nonempty fact assignment"),
                *assigned.last().expect("nonempty fact assignment"),
            )?;
        }
        let last = *assigned.last().expect("nonempty fact assignment");
        transaction
            .execute(
                "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
                [to_sql(last)?],
            )
            .map_err(sqlite_fault)?;
        transaction.commit().map_err(sqlite_fault)?;
        Ok(Applied {
            first: assigned[0],
            last,
        })
    }
}

impl crate::JournalReader {
    /// Evaluate a typed answer inside one immutable read-only SQLite snapshot.
    pub fn read_with<T, E, F>(&self, answer: F) -> Result<T, ReadError<E>>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, E>,
    {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        let read = JournalRead::scoped(&transaction).map_err(ReadError::Journal)?;
        let answer = answer(&read).map_err(ReadError::Answer)?;
        transaction
            .commit()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        Ok(answer)
    }

    /// Evaluate an owned answer against one immutable read-only snapshot.
    pub fn read<T, F>(&self, answer: F) -> Result<T, GuardError>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, GuardError>,
    {
        self.read_with(answer).map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(error) => error,
        })
    }
}

fn run_guards(
    transaction: &Transaction<'_>,
    cursor: Option<wire::Seq>,
    guards: Vec<JournalGuard>,
) -> Result<(), CommitError> {
    let read = JournalRead {
        connection: transaction,
        cursor,
    };
    for guard in guards {
        match (guard.check)(&read) {
            Ok(()) => {}
            Err(GuardError::Refused(refusal)) => return Err(CommitError::Refused(refusal)),
            Err(error) => return Err(CommitError::Fault(CommitFault::new(error.to_string()))),
        }
    }
    Ok(())
}

fn read_cursor(connection: &Connection) -> Result<Option<wire::Seq>, CommitError> {
    visible_cursor(connection)
        .map_err(|error| CommitError::Fault(CommitFault::new(error.to_string())))
}

fn visible_cursor(connection: &Connection) -> Result<Option<wire::Seq>, GuardError> {
    let value = connection
        .query_row(
            "SELECT seq FROM cursor_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(GuardError::Store)?;
    decode_optional_seq(value).map_err(GuardError::Invalid)
}

fn assign_positions(
    current: Option<wire::Seq>,
    count: usize,
) -> Result<Vec<wire::Seq>, CommitError> {
    let start = match current {
        None => 1,
        Some(seq) => seq.get().checked_add(1).ok_or_else(|| {
            CommitError::Fault(CommitFault::new("journal position range exhausted"))
        })?,
    };
    (0..count)
        .map(|offset| {
            let offset = u64::try_from(offset).map_err(|_| {
                CommitError::Fault(CommitFault::new(
                    "fact count exceeds journal position range",
                ))
            })?;
            start
                .checked_add(offset)
                .and_then(wire::Seq::new)
                .ok_or_else(|| {
                    CommitError::Fault(CommitFault::new("journal position range exhausted"))
                })
        })
        .collect()
}

fn insert_facts(
    transaction: &Transaction<'_>,
    write: &JournalWrite,
    assigned: &[wire::Seq],
    current: Option<wire::Seq>,
) -> Result<(), CommitError> {
    for (index, proposed) in write.facts.iter().enumerate() {
        let seq = assigned[index];
        let cause = resolve_cause(proposed.cause, write.id, index, assigned, current)?;
        let envelope = wire::Envelope {
            v: wire::WireVersion::CURRENT,
            seq,
            at: write.at,
            actor: write.actor.clone(),
            origin: write.origin.clone(),
            cause,
            fact: proposed.fact.clone(),
        };
        let bytes = wire::encode(&envelope).map_err(wire_fault)?;
        transaction
            .execute(
                "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, \
                 kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    to_sql(seq)?,
                    i64::from(wire::WireVersion::CURRENT.0),
                    write.at.to_string(),
                    write.actor.to_string(),
                    write.origin.as_str(),
                    cause.map(to_sql).transpose()?,
                    proposed.fact.kind().as_str(),
                    bytes,
                ],
            )
            .map_err(sqlite_fault)?;
        insert_thread_index(transaction, seq, cause, write.origin.as_str())?;
    }
    Ok(())
}

fn resolve_cause(
    cause: FactCause,
    write: u64,
    index: usize,
    assigned: &[wire::Seq],
    current: Option<wire::Seq>,
) -> Result<Option<wire::Seq>, CommitError> {
    match cause {
        FactCause::Root => Ok(None),
        FactCause::Existing(seq) if current.is_some_and(|cursor| seq <= cursor) => Ok(Some(seq)),
        FactCause::Existing(seq) => Err(CommitError::Refused(GuardRefusal::new(format!(
            "cause {seq} is not already durable"
        )))),
        FactCause::Pending(pending) if pending.write != write => Err(CommitError::Refused(
            GuardRefusal::new("pending cause belongs to another journal transaction"),
        )),
        FactCause::Pending(pending) if pending.index < index => Ok(Some(assigned[pending.index])),
        FactCause::Pending(pending) => Err(CommitError::Refused(GuardRefusal::new(format!(
            "pending cause {} must precede fact {index}",
            pending.index
        )))),
    }
}

fn insert_thread_index(
    transaction: &Transaction<'_>,
    seq: wire::Seq,
    cause: Option<wire::Seq>,
    origin: &str,
) -> Result<(), CommitError> {
    let (root, depth) = match cause {
        None => (seq, 0_i64),
        Some(cause) => transaction
            .query_row(
                "SELECT root, depth, origin FROM thread_index WHERE seq = ?1",
                [to_sql(cause)?],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(sqlite_fault)
            .and_then(|(root, depth, parent_origin)| {
                if parent_origin != origin {
                    return Err(CommitError::Refused(GuardRefusal::new(
                        "a fact cause cannot cross origin boundaries",
                    )));
                }
                let root = decode_seq(root).map_err(|detail| {
                    CommitError::Fault(CommitFault::new(format!(
                        "thread root is invalid: {detail}"
                    )))
                })?;
                let depth = depth.checked_add(1).ok_or_else(|| {
                    CommitError::Fault(CommitFault::new("thread depth exhausted"))
                })?;
                Ok((root, depth))
            })?,
    };
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                to_sql(seq)?,
                origin,
                cause.map(to_sql).transpose()?,
                to_sql(root)?,
                depth,
            ],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn apply_heads(
    transaction: &Transaction<'_>,
    write: u64,
    heads: Vec<HeadUpdate>,
    assigned: &[wire::Seq],
) -> Result<(), CommitError> {
    for head in heads {
        if head.fact.write != write {
            return Err(CommitError::Refused(GuardRefusal::new(
                "head names a fact from another journal transaction",
            )));
        }
        let seq = assigned.get(head.fact.index).copied().ok_or_else(|| {
            CommitError::Refused(GuardRefusal::new(format!(
                "head names missing pending fact {}",
                head.fact.index
            )))
        })?;
        let row = (head.build)(seq).map_err(|detail| {
            CommitError::Refused(GuardRefusal::new(format!("head update refused: {detail}")))
        })?;
        let statement = format!(
            "INSERT INTO {table}({key}, head_seq, body) VALUES (?1, ?2, ?3) \
             ON CONFLICT({key}) DO UPDATE SET head_seq = excluded.head_seq, body = excluded.body",
            table = row.table,
            key = row.key_column,
        );
        match row.key {
            HeadKey::Text(key) => transaction
                .execute(&statement, params![key, to_sql(row.seq)?, row.body])
                .map_err(sqlite_fault)?,
            HeadKey::Singleton => transaction
                .execute(&statement, params![1_i64, to_sql(row.seq)?, row.body])
                .map_err(sqlite_fault)?,
        };
    }
    Ok(())
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    actor: &wire::Actor,
    receipt: PendingReceipt,
    first: wire::Seq,
    last: wire::Seq,
) -> Result<(), CommitError> {
    let response = wire::Response {
        v: wire::WireVersion::CURRENT,
        id: receipt.command.id,
        epoch: receipt.epoch,
        outcome: wire::Outcome::Reply(wire::Reply {
            position: wire::ReplyPosition::Committed(wire::SeqRange { first, last }),
            body: receipt.result,
        }),
    };
    let witnesses = wire::encode(&receipt.command.expected).map_err(wire_fault)?;
    let response = wire::encode(&response).map_err(wire_fault)?;
    transaction
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, \
             evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
            params![
                receipt.command.id.to_string(),
                actor.to_string(),
                receipt.command.kind.as_str(),
                receipt.request,
                witnesses,
                response,
                to_sql(first)?,
                to_sql(last)?,
                receipt.epoch.to_string(),
            ],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn decode_optional_seq(value: Option<i64>) -> Result<Option<wire::Seq>, String> {
    value.map(decode_seq).transpose()
}

fn decode_seq(value: i64) -> Result<wire::Seq, String> {
    u64::try_from(value)
        .ok()
        .and_then(wire::Seq::new)
        .ok_or_else(|| format!("{value} is not a positive wire position"))
}

fn to_sql(seq: wire::Seq) -> Result<i64, CommitError> {
    i64::try_from(seq.get()).map_err(|_| {
        CommitError::Fault(CommitFault::new(format!(
            "journal position {seq} exceeds SQLite integer range"
        )))
    })
}

fn sqlite_fault(error: rusqlite::Error) -> CommitError {
    CommitError::Fault(CommitFault::new(error.to_string()))
}

fn wire_fault(error: serde_json::Error) -> CommitError {
    CommitError::Fault(CommitFault::new(error.to_string()))
}
