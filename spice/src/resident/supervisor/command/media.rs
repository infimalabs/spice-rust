//! Private local projections of the steering media an occupant is told about.
//!
//! An interjection names each attachment by reference; the occupant can only
//! look at the bytes through a readable file. A path stays readable while this
//! occupant's compaction generation lasts, and a new occupant retires what its
//! predecessor saw.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use wire::CanonicalUuid;

use crate::steering_command::media_projection::{self, Projection, Record};

pub(super) struct Projector {
    root: PathBuf,
    lane: CanonicalUuid,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    compaction: Option<u64>,
    retained: Vec<Record>,
}

/// Encoded projection lines keyed by directive and attachment ordinal.
pub(super) type Projected = BTreeMap<(CanonicalUuid, u8), String>;

impl Projector {
    pub(super) fn new(root: PathBuf, lane: CanonicalUuid) -> Self {
        Self {
            root,
            lane,
            state: Mutex::new(State::default()),
        }
    }

    /// Publish every pending attachment and name where each one is readable.
    pub(super) fn project(&self, inbox: &wire::intent::LaneInbox) -> Result<Projected, String> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let generation_changed = match state.compaction {
            None => {
                media_projection::retire_lane(&self.root, self.lane)?;
                false
            }
            Some(seen) => inbox.compaction.is_some_and(|current| current > seen),
        };
        let projected = media_projection::reconcile(
            self.lane,
            &inbox.pending,
            &mut state.retained,
            &self.root,
            generation_changed,
        )?;
        state.compaction = Some(inbox.compaction.max(state.compaction).unwrap_or_default());
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
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const LANE: &str = "0a2f5b6c-1d3e-4f50-8a91-b2c3d4e5f607";
    const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";

    #[test]
    fn an_interjected_attachment_names_a_private_file_holding_its_exact_bytes() {
        let root = scratch();
        let lane: CanonicalUuid = LANE.parse().expect("fixture lane");
        let inbox = inbox(lane, 3, "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718");
        let projected = Projector::new(root.clone(), lane)
            .project(&inbox)
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
        let earlier = Projector::new(root.clone(), lane)
            .project(&inbox(lane, 3, "1b3f6c7d-2e4f-4051-9b02-c3d4e5f60718"))
            .expect("earlier media projects");
        let earlier = path(earlier.values().next().expect("earlier projection"));
        assert!(fs::metadata(&earlier).is_ok());

        Projector::new(root.clone(), lane)
            .project(&inbox(lane, 0, "2c4a7d8e-3f50-4162-8c13-d4e5f6071829"))
            .expect("successor media projects");
        assert!(
            fs::symlink_metadata(&earlier).is_err(),
            "a successor must not inherit its predecessor's private media",
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
