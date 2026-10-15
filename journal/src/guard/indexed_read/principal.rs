//! Current human principal heads.

use wire::principal::PrincipalHead;

use super::{GuardError, JournalHead, JournalRead, decode_head, read_metrics};

impl JournalRead<'_> {
    /// Current head of one principal, absent before its birth.
    pub fn principal_head(
        &self,
        principal: wire::CanonicalUuid,
    ) -> Result<Option<JournalHead<PrincipalHead>>, GuardError> {
        let key = principal.to_string();
        let head = self.text_head::<PrincipalHead>("principal_heads", "principal", &key)?;
        if let Some(head) = &head
            && head.body.principal != principal
        {
            return Err(GuardError::Invalid(format!(
                "principal head indexed as {key} carries body principal {}",
                head.body.principal
            )));
        }
        Ok(head)
    }

    /// Every born principal's current head, oldest birth first.
    ///
    /// The index holds one row per admitted human, so the whole set is the
    /// bounded answer.
    pub fn principal_heads(&self) -> Result<Vec<JournalHead<PrincipalHead>>, GuardError> {
        let mut statement = self
            .connection
            .prepare("SELECT principal, head_seq, body FROM principal_heads")
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut heads = Vec::new();
        for row in rows {
            let (key, position, bytes) = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
            let head: JournalHead<PrincipalHead> =
                decode_head(position, &bytes).map_err(|error| {
                    GuardError::Invalid(format!("principal_heads principal {key:?}: {error}"))
                })?;
            if head.body.principal.to_string() != key {
                return Err(GuardError::Invalid(format!(
                    "principal head indexed as {key} carries body principal {}",
                    head.body.principal
                )));
            }
            if self.cursor.is_none_or(|cursor| head.position > cursor) {
                return Err(GuardError::Invalid(format!(
                    "principal_heads principal {key:?} head position {} is beyond cursor {:?}",
                    head.position,
                    self.cursor.map(wire::Seq::get)
                )));
            }
            heads.push(head);
        }
        heads.sort_by_key(|head| head.body.born);
        Ok(heads)
    }
}
