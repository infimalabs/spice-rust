//! Confirmed delivery of one private background notice to an exact occupant.

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use wire::CanonicalUuid;

use super::{CALLBACK_REPLY_BYTES, Kick, kick_path, send_to};
use crate::resident::design_input::BackgroundNotice;
use crate::resident::{RuntimePaths, new_uuid};

const REPLY_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Deserialize, Serialize)]
struct Reply {
    request: String,
    accepted: bool,
}

pub(super) fn send(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: BackgroundNotice,
) -> io::Result<bool> {
    send_with_owner(paths, lane, notice, None)
}

pub(super) fn send_retained(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: BackgroundNotice,
    owner: super::ServiceClaim,
) -> io::Result<bool> {
    send_with_owner(paths, lane, notice, Some(owner))
}

fn send_with_owner(
    paths: &RuntimePaths,
    lane: CanonicalUuid,
    notice: BackgroundNotice,
    owner: Option<super::ServiceClaim>,
) -> io::Result<bool> {
    send_to_door_with_owner(&kick_path(paths, lane), &paths.common, notice, owner)
}

#[cfg(test)]
/// Send `notice` through the kick door at `door`, awaiting the answer at a
/// reply door beside it.
pub(super) fn send_to_door(
    door: &Path,
    identity_source: &Path,
    notice: BackgroundNotice,
) -> io::Result<bool> {
    send_to_door_with_owner(door, identity_source, notice, None)
}

fn send_to_door_with_owner(
    door: &Path,
    identity_source: &Path,
    notice: BackgroundNotice,
    owner: Option<super::ServiceClaim>,
) -> io::Result<bool> {
    let request = new_uuid().to_string();
    let mut identity = blake3::Hasher::new();
    identity.update(identity_source.as_os_str().as_encoded_bytes());
    identity.update(request.as_bytes());
    let reply = door.with_file_name(format!("spice-b-{}.sock", identity.finalize()));
    let callback = door::Door::bind(&reply, CALLBACK_REPLY_BYTES)?;
    let kick = Kick::BackgroundConfirmed {
        notice,
        owner,
        request: request.clone(),
        reply,
    };
    send_to(door, &kick)?;
    let bytes = callback.recv(Some(REPLY_TIMEOUT))?;
    let answer: Reply = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if answer.request != request {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "background callback named another request",
        ));
    }
    Ok(answer.accepted)
}

pub(super) fn answer(reply: &Path, request: String, accepted: bool) {
    let bytes = serde_json::to_vec(&Reply { request, accepted })
        .expect("a background reply is representable as JSON");
    let _sent = door::send(reply, &bytes);
}
