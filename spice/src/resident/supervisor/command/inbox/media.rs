//! Private local projections of the steering media an occupant is told about.
//!
//! An interjection names each attachment by reference; the occupant can only
//! look at the bytes through a readable file. A path stays readable while this
//! occupant's compaction generation lasts, and a new occupant retires what its
//! predecessor saw.
//!
//! Every inbox read runs in its own process, so what this occupant has been
//! shown lives in a state file beside its supervisor state. A missing file is
//! a new occupant.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::CanonicalUuid;

use crate::steering_command::media_projection::{self, Projection, Record};

/// Bytes a projection state file may hold before it is refused as corrupt.
const STATE_BYTES: u64 = 1 << 20;

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    compaction: Option<u64>,
    retained: Vec<Record>,
}

/// Encoded projection lines keyed by directive and attachment ordinal.
pub(super) type Projected = BTreeMap<(CanonicalUuid, u8), String>;

/// Publish every pending attachment under `root` and name where each one is
/// readable, carrying what this occupant was shown in the file at `state`.
pub(super) fn project(
    root: &Path,
    lane: CanonicalUuid,
    state: &Path,
    inbox: &wire::intent::LaneInbox,
) -> Result<Projected, String> {
    let mut shown = match crate::resident::supervisor::read_json::<State>(state, STATE_BYTES) {
        Ok(shown) => shown,
        Err(error) if error.kind() == io::ErrorKind::NotFound => State::default(),
        Err(error) => return Err(format!("steering media state is unreadable: {error}")),
    };
    let generation_changed = match shown.compaction {
        None => {
            media_projection::retire_lane(root, lane)?;
            false
        }
        Some(seen) => inbox.compaction.is_some_and(|current| current > seen),
    };
    let projected = media_projection::reconcile(
        lane,
        &inbox.pending,
        &mut shown.retained,
        root,
        generation_changed,
    )?;
    shown.compaction = Some(inbox.compaction.max(shown.compaction).unwrap_or_default());
    crate::resident::supervisor::publish_json(state, &shown)
        .map_err(|error| format!("steering media state was not recorded: {error}"))?;
    projected
        .values()
        .flatten()
        .map(|projection: &Projection| {
            let encoded = String::from_utf8(projection.encoded()?)
                .map_err(|_| "an encoded media projection is UTF-8 JSON".to_owned())?;
            let reference = projection.reference();
            Ok(((reference.directive(), reference.ordinal()), encoded))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    const LANE: &str = "0a2f5b6c-1d3e-4f50-8a91-b2c3d4e5f607";
    const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";

    #[test]
    fn an_interjected_attachment_names_a_private_file_holding_its_exact_bytes() {
        let root = scratch();
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        let inbox = inbox(lane, 3, "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718");
        let projected = project(&root, lane, &root.join("occupant.json"), &inbox)
            .expect("pending media projects");
        let rendered = steer::inbox::render_pending(
            &steer::Key::for_identity(lane),
            &inbox.pending,
            |directive, ordinal| projected.get(&(directive, ordinal)).cloned(),
        );

        let reference = rendered
            .lines()
            .position(|line| line.starts_with("media-reference="))
            .expect("the interjection names the attachment");
        let projection = rendered
            .lines()
            .nth(reference + 1)
            .and_then(|line| line.strip_prefix("media-projection="))
            .expect("the projection follows its reference");
        let projection: serde_json::Value =
            serde_json::from_str(projection).expect("projection is JSON");
        let path = projection["path"]
            .as_str()
            .expect("projection names a path");
        assert_eq!(
            fs::read(path).expect("the named file is readable"),
            inbox.pending[0].attachments[0]
                .decoded_bytes()
                .expect("fixture bytes decode"),
        );
        fs::remove_dir_all(root).expect("remove projection fixture");
    }

    #[test]
    fn a_new_occupant_retires_what_its_predecessor_was_shown() {
        let root = scratch();
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        let earlier = project(
            &root,
            lane,
            &root.join("predecessor.json"),
            &inbox(lane, 3, "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718"),
        )
        .expect("earlier media projects");
        let earlier = path(earlier.values().next().expect("earlier projection"));
        assert!(fs::metadata(&earlier).is_ok());

        project(
            &root,
            lane,
            &root.join("successor.json"),
            &inbox(lane, 0, "2c4a7d8e-3f50-4162-8c13-d4e5f6071829"),
        )
        .expect("successor media projects");
        assert!(
            fs::symlink_metadata(&earlier).is_err(),
            "a successor must not inherit its predecessor's private media",
        );
        fs::remove_dir_all(root).expect("remove projection fixture");
    }

    #[test]
    fn one_occupant_keeps_what_it_was_shown_across_reads_until_compaction() {
        let root = scratch();
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        let state = root.join("occupant.json");
        let shown = project(
            &root,
            lane,
            &state,
            &inbox(lane, 3, "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718"),
        )
        .expect("first read projects");
        let shown = path(shown.values().next().expect("first projection"));

        // A later read in the same generation no longer lists the answered
        // directive, yet the path it advertised stays readable.
        project(
            &root,
            lane,
            &state,
            &inbox(lane, 3, "2c4a7d8e-3f50-4162-8c13-d4e5f6071829"),
        )
        .expect("later read projects");
        assert!(
            fs::metadata(&shown).is_ok(),
            "a path advertised to this occupant must survive its next read",
        );

        project(
            &root,
            lane,
            &state,
            &inbox(lane, 4, "2c4a7d8e-3f50-4162-8c13-d4e5f6071829"),
        )
        .expect("post-compaction read projects");
        assert!(
            fs::symlink_metadata(&shown).is_err(),
            "compaction retires what the dropped context was shown",
        );
        fs::remove_dir_all(root).expect("remove projection fixture");
    }

    fn path(encoded: &str) -> String {
        let projection: serde_json::Value = serde_json::from_str(encoded).expect("projection");
        projection["path"].as_str().expect("path").to_owned()
    }

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!(
            "spice-supervisor-media-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple(),
        ))
    }

    fn inbox(lane: CanonicalUuid, compaction: u64, directive: &str) -> wire::intent::LaneInbox {
        serde_json::from_value(serde_json::json!({
            "lane": lane,
            "occupant": "3d5b8e9f-4061-4273-9d24-e5f607182930",
            "compaction": compaction,
            "pending": [{
                "directive": directive,
                "key": "1kMedia1",
                "text": "inspect the capture",
                "attachments": [{
                    "name": "capture.png",
                    "content_type": "image/png",
                    "data_url": PNG_DATA_URL,
                }],
                "authored": "2026-08-30T09:00:00.000Z",
                "stage": "published",
                "expires": "2026-08-30T10:00:00.000Z",
            }],
        }))
        .expect("inbox fixture admits")
    }
}
