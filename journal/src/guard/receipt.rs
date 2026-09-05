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
            return Ok(None);
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
}
