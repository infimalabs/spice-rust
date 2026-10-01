use rusqlite::OptionalExtension as _;

use super::*;

/// Result of looking up a client command identity in durable authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandReplay {
    /// The identity names this exact admitted request and its committed answer.
    Exact(wire::Outcome<wire::intent::CommandAnswer>),
    /// The identity is durable, but it was bound to another request or principal.
    Conflict,
}

impl JournalRead<'_> {
    /// Recover the durable answer for an exact retry of one admitted command.
    pub fn command_replay(
        &self,
        command: &wire::AdmittedCommand,
    ) -> Result<Option<CommandReplay>, GuardError> {
        let row = self
            .connection
            .query_row(
                "SELECT actor, command_kind, request, response, epoch \
                 FROM command_receipts WHERE command_id = ?1",
                [command.id().to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(GuardError::Store)?;
        let Some((actor, kind, request, response, epoch)) = row else {
            let historical = self.connection.query_row(
                "SELECT EXISTS (SELECT 1 FROM historical_command_receipts WHERE command_id = ?1)",
                [command.id().to_string()],
                |row| row.get::<_, bool>(0),
            ).map_err(GuardError::Store)?;
            return Ok(historical.then_some(CommandReplay::Conflict));
        };
        if actor != command.actor().to_string() || kind != command.kind().as_str() {
            return Ok(Some(CommandReplay::Conflict));
        }
        let stored: wire::Command<serde_json::Value> = wire::decode(&request).map_err(|error| {
            GuardError::Invalid(format!("command receipt request is invalid: {error}"))
        })?;
        let current: wire::Command<serde_json::Value> = wire::decode(command.canonical_request())
            .map_err(|error| {
            GuardError::Invalid(format!("admitted command request is invalid: {error}"))
        })?;
        if epoch != stored.epoch.to_string()
            || stored.v != current.v
            || stored.id != current.id
            || stored.kind != current.kind
            || stored.expected != current.expected
            || stored.body != current.body
        {
            return Ok(Some(CommandReplay::Conflict));
        }
        let response: wire::Response<serde_json::Value> = wire::decode(&response)
            .map_err(|error| GuardError::Invalid(format!("command receipt is invalid: {error}")))?;
        if !response.v.is_current() || response.id != command.id() || response.epoch != stored.epoch
        {
            return Err(GuardError::Invalid(
                "command receipt response identity disagrees with its request".to_owned(),
            ));
        }
        let wire::Outcome::Reply(reply) = response.outcome else {
            return Err(GuardError::Invalid(
                "durable command receipt does not contain a successful reply".to_owned(),
            ));
        };
        if !matches!(reply.position, wire::ReplyPosition::Committed(_)) {
            return Err(GuardError::Invalid(
                "durable command receipt does not name a committed position".to_owned(),
            ));
        }
        let body = wire::encode(&reply.body)
            .and_then(|body| command.kind().binding().answer.decode(&body))
            .map_err(|error| {
                GuardError::Invalid(format!("command receipt answer is invalid: {error}"))
            })?;
        Ok(Some(CommandReplay::Exact(wire::Outcome::Reply(
            wire::Reply {
                position: reply.position,
                body,
            },
        ))))
    }

    /// Read case-only authored review flows without publishing request prose.
    ///
    /// The caller must admit the returned target through the board's project
    /// scope. This is retained command-record evidence, not a journal fact.
    pub fn report_review_flows(
        &self,
        position: wire::Seq,
    ) -> Result<Option<wire::intent::ReportFlowRecord>, GuardError> {
        let position = i64::try_from(position.get()).map_err(|_| {
            GuardError::Invalid("report command position exceeds SQLite".to_owned())
        })?;
        let row = self
            .connection
            .query_row(
                "SELECT command_id, first_seq, last_seq, request \
                 FROM command_receipts WHERE first_seq <= ?1 AND last_seq >= ?1 \
                 AND command_kind = 'task::review' LIMIT 1",
                [position],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(GuardError::Store)?;
        let Some((id, first, last, bytes)) = row else {
            return Ok(None);
        };
        let request: wire::Command<serde_json::Value> = wire::decode(&bytes)
            .map_err(|error| GuardError::Invalid(format!("report command record: {error}")))?;
        if request.id.to_string() != id || request.kind != wire::CommandKind::TaskReview {
            return Err(GuardError::Invalid(
                "report command record identity disagrees".to_owned(),
            ));
        }
        let task = request
            .body
            .get("task")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| GuardError::Invalid("report review record has no target".to_owned()))?
            .parse()
            .map_err(|error| GuardError::Invalid(format!("report review target: {error}")))?;
        let mut requested_flows = Vec::new();
        if let Some(corrections) = request
            .body
            .get("then")
            .and_then(serde_json::Value::as_array)
        {
            for correction in corrections {
                let flow = correction
                    .get("add")
                    .and_then(|add| add.get("flow"))
                    .map(|flow| {
                        flow.as_str().map(str::to_owned).ok_or_else(|| {
                            GuardError::Invalid("report authored flow is not text".to_owned())
                        })
                    })
                    .transpose()?;
                requested_flows.push(flow);
            }
        }
        let position = |value| {
            u64::try_from(value)
                .ok()
                .and_then(wire::Seq::new)
                .ok_or_else(|| GuardError::Invalid("report record position is invalid".to_owned()))
        };
        Ok(Some(wire::intent::ReportFlowRecord {
            command: request.id,
            request_digest: wire::Digest::compute(wire::Domain::ReportCommandRecord, &bytes),
            first: position(first)?,
            last: position(last)?,
            task,
            requested_flows,
        }))
    }
}
