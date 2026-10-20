use rusqlite::Transaction;

use super::OpenError;

/// Index each retained steering head's room as a typed column, after every
/// other crossing has settled the bodies it is copied from.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch(
            "ALTER TABLE steering_heads ADD COLUMN room TEXT;
             UPDATE steering_heads SET room = json_extract(CAST(body AS TEXT), '$.room');
             CREATE INDEX steering_heads_lane_room_created
                 ON steering_heads(lane, room, created_seq);",
        )
        .map_err(OpenError::Sqlite)
}
