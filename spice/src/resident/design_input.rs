//! Private Design input due after authority or session-evidence changes:
//! profile-selected invitations are recorded before allocation, then the newest
//! return of a lane's own active Design is delivered and durably consumed.

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
    /// Whether this notice and a maximum-size confirmation route fit one kick
    /// datagram.
    fn confirmed_fits(&self) -> bool {
        serde_json::to_vec(&Kick::BackgroundConfirmed {
            notice: self.clone(),
            request: "00000000-0000-0000-0000-000000000000".to_owned(),
            reply:
                "/tmp/spice-b-0000000000000000000000000000000000000000000000000000000000000000.sock"
                    .into(),
        })
        .is_ok_and(|encoded| encoded.len() <= KICK_BYTES)
    }
}

/// Record session-profile Design requests, then tell every occupied claimant
/// the newest return of an active Design it holds. Return delivery is confirmed
/// before its durable consumption record; either failure is retried on the
/// next reconciliation pass.
pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    seats: &Seats,
    completions: &mut super::design_awareness::CompletionCursor,
    profiles: &std::sync::Mutex<message_surface::MessageState>,
    diagnostic: &mut dyn io::Write,
) -> CommandResult<()> {
    super::design_awareness::reconcile(bus, seats, completions, profiles, diagnostic)?;
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
        let notice = return_notice(&owed);
        match super::supervisor::command::confirmed_background_notice(paths, owed.lane, notice) {
            Ok(true) => {
                if let Err(outcome) = record_return(bus, &owed) {
                    let _ = writeln!(
                        diagnostic,
                        "lane {} return of Design {} was delivered but not recorded: {outcome:?}",
                        owed.lane, owed.design
                    );
                }
            }
            Ok(false) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} return of Design {} was refused by its current supervisor",
                    owed.lane, owed.design
                );
            }
            Err(error) => {
                let _ = writeln!(
                    diagnostic,
                    "lane {} return of Design {} was not delivered: {error}",
                    owed.lane, owed.design
                );
            }
        }
    }
    Ok(())
}

/// The private notice carrying one decision's feedback to the Design holder,
/// clipped until it fits one datagram; room history keeps the text whole.
pub(in crate::resident) fn return_notice(owed: &ReturnOwed) -> BackgroundNotice {
    let mut reason = owed.reason.clone();
    loop {
        let notice = BackgroundNotice {
            occupant: owed.occupant,
            purpose: format!("design-return:{}", owed.design),
            text: format!(
                "<system-notice>\nThe operator recorded feedback on candidate {} of your Design {} \
                 (decision {}):\n{reason}\nThis feedback is the Design's newest input: revise its \
                 proposal with it. `spice room history {}` lists every decision. This is a \
                 private background notice: it needs no acknowledgment key or visible ACK/NAK.\n\
                 </system-notice>",
                owed.candidate, owed.design, owed.decision.decision, owed.design,
            ),
        };
        if notice.confirmed_fits() || reason.len() <= SHORTEST_REASON_BYTES {
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
