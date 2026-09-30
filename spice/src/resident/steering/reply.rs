//! A lane's answer to a peer message, delivered to the lane that told it.

use super::*;

/// Append the reply one lane's answer to a live peer message owes its sender.
///
/// Only a plain peer message earns a reply. Direction without a sender is the
/// operator's, the resident's, or taste's, and a reply carries its peer-reply
/// control, so answering it ends the exchange. The reply joins the
/// retirement's write, so the journal holds both or neither and a restart can
/// neither drop nor repeat it. It lands whether or not the sending lane is
/// seated: a sender that asked and ended its turn hears the answer at its next
/// seat, since a reply, like any peer message, never wakes a lane. It returns
/// the sending lane that must be told once the write commits.
#[expect(
    clippy::too_many_arguments,
    reason = "the retirement write, its answered head, and the spoken answer are one reply"
)]
pub(super) fn append_peer_reply(
    bus: &Bus,
    proposal: &mut JournalWrite,
    epoch: CanonicalUuid,
    answered: &journal::JournalHead<wire::SteeringHead>,
    retirement: PendingFact,
    disposition: Disposition,
    clause: Option<&str>,
    at: Instant,
) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
    let told = answered.body();
    let (Some(sender), None) = (told.from, told.control.as_deref()) else {
        return Ok(None);
    };
    let Some(answer) = wire::PeerAnswer::from_disposition(disposition) else {
        return Ok(None);
    };
    let residency = current_residency(bus, epoch).map_err(typed_outcome)?;
    let Some(row) = residency.body().lanes.iter().find(|row| row.lane == sender) else {
        return Ok(None);
    };
    let occupant = row.occupant;
    let directive = reply_identity(told.directive);
    let (existing, minted_micros) = keyspace(bus, directive)?;
    if existing.is_some() {
        return Err(typed_store_fault(
            "steer::identity-taken",
            "the reply to this peer message already exists",
        ));
    }
    let key = mint_key(minted_micros)?;
    let lifetime = told.expires.get().duration_since(told.published.get());
    let expires = at
        .get()
        .checked_add(lifetime)
        .map_err(|error| error.to_string())
        .and_then(|expires| Instant::new(expires).map_err(|error| error.to_string()))
        .map_err(|error| typed_environment_fault("steer::expiry-invalid", &error))?;
    let spoken = format!("{} {}", answer.marker(), told.key);
    let replied = wire::body::SteerReplied {
        directive,
        key: key.to_string(),
        lane: sender,
        from: told.lane,
        answered: told.key.clone(),
        answer,
        text: clause.map_or_else(|| spoken.clone(), |clause| format!("{spoken}: {clause}")),
        expires,
    };
    let witness = Witness::Residency {
        lane: sender,
        occupant,
        epoch,
    };
    proposal.guard(send_guard(residency.position(), witness, directive, key));
    let pending = proposal
        .fact(replied.clone(), FactCause::Pending(retirement))
        .map_err(|error| typed_store_fault("steer::fact-invalid", &error.to_string()))?;
    let origin = told.origin.clone();
    proposal.steering_head(pending, move |seq| {
        steer::Ledger::replied_head(&replied, origin, at, seq)
            .expect("the reply body is admitted before publication")
    });
    Ok(Some(sender))
}

/// The one reply identity an answered peer message can ever own.
fn reply_identity(answered: CanonicalUuid) -> CanonicalUuid {
    let identity = format!("spice:peer-reply:{answered}");
    let digest = blake3::hash(identity.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}
