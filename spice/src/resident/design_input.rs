//! What a Design's occupied claimant is told privately after an authority
//! change: matching open Designs ([`super::design_awareness`]) and the
//! operator's newest return of its own active Design
//! ([`crate::board_command::owed_returns`]). Each telling is recorded before
//! it is sent, needs no acknowledgment, and starts no work.

use serde::{Deserialize, Serialize};
use wire::CanonicalUuid;

use super::supervisor::command::{KICK_BYTES, Kick};
use super::*;
use crate::board_command::{ReturnOwed, owed_returns, record_return};

/// Return reason length a notice keeps whole while fitting one datagram.
const SHORTEST_REASON_BYTES: usize = 16;

/// One private background notice for an exact occupant, replacing any queued
/// notice of the same purpose.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::resident) struct BackgroundNotice {
    pub(in crate::resident) occupant: CanonicalUuid,
    pub(in crate::resident) purpose: String,
    pub(in crate::resident) text: String,
}

impl BackgroundNotice {
    /// Whether this notice fits one kick datagram.
    pub(in crate::resident) fn fits(&self) -> bool {
        serde_json::to_vec(&Kick::Background(self.clone()))
            .is_ok_and(|encoded| encoded.len() <= KICK_BYTES)
    }
}

/// Tell every occupied claimant what its Design input owes it: matching open
/// Designs, then the newest return of an active Design it holds. A failed
/// record is reported and retried on the next authority change; it never
/// fails the caller, and nothing is told without its record.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    config: &SurfaceConfig,
    seats: &Seats,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    super::design_awareness::reconcile(bus, paths, config, seats, diagnostic)?;
    let occupied = super::design_awareness::occupied(seats);
    if occupied.is_empty() {
        return Ok(());
    }
    let owed = match owed_returns(bus, &occupied) {
        Ok(owed) => owed,
        Err(outcome) => {
            let _ = writeln!(diagnostic, "Design returns were not read: {outcome:?}");
            return Ok(());
        }
    };
    for owed in owed {
        match record_return(bus, &owed) {
            Ok(()) => super::supervisor::command::background_notice(
                paths,
                owed.lane,
                return_notice(&owed),
            ),
            Err(outcome) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} return of Design {} was not recorded: {outcome:?}",
                    owed.lane, owed.design
                );
            }
        }
    }
    Ok(())
}

/// The private notice carrying one return to the Design's holder, its reason
/// clipped until it fits one datagram; the room keeps the reason whole.
pub(in crate::resident) fn return_notice(owed: &ReturnOwed) -> BackgroundNotice {
    let mut reason = owed.reason.clone();
    loop {
        let notice = BackgroundNotice {
            occupant: owed.occupant,
            purpose: format!("design-return:{}", owed.design),
            text: format!(
                "<system-notice>\nThe operator returned candidate {} of your Design {} \
                 (decision {}): {reason}\nThis reason is the Design's newest input: revise its \
                 proposal with it. `spice room history {}` lists every decision. This is a \
                 private background notice: it needs no acknowledgment key or visible ACK/NAK.\n\
                 </system-notice>",
                owed.candidate, owed.design, owed.decision.decision, owed.design,
            ),
        };
        if notice.fits() || reason.len() <= SHORTEST_REASON_BYTES {
            return notice;
        }
        reason = clip(&reason, reason.len() / 2);
    }
}

/// `text` cut at a character boundary to at most `bytes` bytes.
pub(in crate::resident) fn clip(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes - '…'.len_utf8();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
