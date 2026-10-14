//! Confirmed delivery of one private background notice to an exact occupant.

use std::fs;
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
    send_to_door(&kick_path(paths, lane), &paths.common, notice)
}

pub(super) fn send_to_door(
    door: &Path,
    identity_source: &Path,
    notice: BackgroundNotice,
) -> io::Result<bool> {
    #[cfg(unix)]
    {
        let request = new_uuid().to_string();
        let mut identity = blake3::Hasher::new();
        identity.update(identity_source.as_os_str().as_encoded_bytes());
        identity.update(request.as_bytes());
        let reply = Path::new("/tmp").join(format!("spice-b-{}.sock", identity.finalize()));
        let _ = fs::remove_file(&reply);
        let socket = std::os::unix::net::UnixDatagram::bind(&reply)?;
        socket.set_read_timeout(Some(REPLY_TIMEOUT))?;
        let kick = Kick::BackgroundConfirmed {
            notice,
            request: request.clone(),
            reply: reply.clone(),
        };
        let result = send_to(door, &kick).and_then(|()| {
            let mut bytes = [0u8; CALLBACK_REPLY_BYTES];
            let length = socket.recv(&mut bytes)?;
            let answer: Reply = serde_json::from_slice(&bytes[..length])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if answer.request != request {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "background callback named another request",
                ));
            }
            Ok(answer.accepted)
        });
        drop(socket);
        let _ = fs::remove_file(reply);
        result
    }
    #[cfg(not(unix))]
    {
        let _ = (door, identity_source, notice);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "confirmed background notices require Unix command doors",
        ))
    }
}

pub(super) fn answer(reply: &Path, request: String, accepted: bool) {
    #[cfg(unix)]
    {
        let bytes = serde_json::to_vec(&Reply { request, accepted })
            .expect("a background reply is representable as JSON");
        let _sent = std::os::unix::net::UnixDatagram::unbound()
            .and_then(|socket| socket.send_to(&bytes, reply));
    }
    #[cfg(not(unix))]
    let _ = (reply, request, accepted);
}
