//! Browser-local room console state.
//!
//! Working-group facts own saved organization. This state owns presentation
//! only: which group consoles and rooms this browser opened, their order,
//! drafts, reader positions, and focus anchors.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wire::TaskStamp;
use wire::rooms::RoomAuthorityStamp;

/// Browser-local presentation for all explicitly opened room consoles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoomViews {
    groups: Vec<GroupView>,
}

/// One explicitly mounted group console.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GroupView {
    pub(crate) group: RoomAuthorityStamp,
    rooms: Vec<TaskStamp>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    drafts: BTreeMap<TaskStamp, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    readers: BTreeMap<TaskStamp, i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    focus: Option<TaskStamp>,
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
impl RoomViews {
    /// Restore only canonical presentation rows. Corrupt or duplicate rows
    /// grant no authority and are discarded rather than guessed together.
    pub(crate) fn restore(document: &str) -> Option<Self> {
        let mut views = serde_json::from_str::<Self>(document).ok()?;
        let mut groups = BTreeSet::new();
        views.groups.retain_mut(|group| {
            if !groups.insert(group.group.clone()) {
                return false;
            }
            let mut rooms = BTreeSet::new();
            group.rooms.retain(|room| rooms.insert(room.clone()));
            group.drafts.retain(|room, _| rooms.contains(room));
            group.readers.retain(|room, _| rooms.contains(room));
            if group
                .focus
                .as_ref()
                .is_some_and(|room| !rooms.contains(room))
            {
                group.focus = None;
            }
            !group.rooms.is_empty()
        });
        Some(views)
    }

    pub(crate) fn document(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    pub(crate) fn groups(&self) -> &[GroupView] {
        &self.groups
    }

    pub(crate) fn open_room(&mut self, group: RoomAuthorityStamp, room: TaskStamp) {
        if let Some(view) = self.groups.iter_mut().find(|view| view.group == group) {
            if !view.rooms.contains(&room) {
                view.rooms.push(room.clone());
            }
            view.focus = Some(room);
            return;
        }
        self.groups.push(GroupView {
            group,
            rooms: vec![room.clone()],
            drafts: BTreeMap::new(),
            readers: BTreeMap::new(),
            focus: Some(room),
        });
    }

    pub(crate) fn close_room(&mut self, group: &RoomAuthorityStamp, room: &TaskStamp) {
        let Some(index) = self.groups.iter().position(|view| &view.group == group) else {
            return;
        };
        let view = &mut self.groups[index];
        view.rooms.retain(|opened| opened != room);
        view.drafts.remove(room);
        view.readers.remove(room);
        if view.focus.as_ref() == Some(room) {
            view.focus = view.rooms.last().cloned();
        }
        if view.rooms.is_empty() {
            self.groups.remove(index);
        }
    }

    pub(crate) fn close_group(&mut self, group: &RoomAuthorityStamp) {
        self.groups.retain(|view| &view.group != group);
    }

    pub(crate) fn move_group(&mut self, group: &RoomAuthorityStamp, delta: isize) {
        let Some(index) = self.groups.iter().position(|view| &view.group == group) else {
            return;
        };
        let target = index
            .saturating_add_signed(delta)
            .min(self.groups.len() - 1);
        if target != index {
            self.groups.swap(index, target);
        }
    }

    pub(crate) fn move_room(&mut self, group: &RoomAuthorityStamp, room: &TaskStamp, delta: isize) {
        let Some(view) = self.groups.iter_mut().find(|view| &view.group == group) else {
            return;
        };
        let Some(index) = view.rooms.iter().position(|opened| opened == room) else {
            return;
        };
        let target = index.saturating_add_signed(delta).min(view.rooms.len() - 1);
        if target != index {
            view.rooms.swap(index, target);
        }
    }

    pub(crate) fn set_draft(
        &mut self,
        group: &RoomAuthorityStamp,
        room: &TaskStamp,
        draft: String,
    ) {
        let Some(view) = self.groups.iter_mut().find(|view| &view.group == group) else {
            return;
        };
        if draft.is_empty() {
            view.drafts.remove(room);
        } else {
            view.drafts.insert(room.clone(), draft);
        }
    }

    pub(crate) fn set_reader(&mut self, group: &RoomAuthorityStamp, room: &TaskStamp, offset: i32) {
        if let Some(view) = self.groups.iter_mut().find(|view| &view.group == group) {
            view.readers.insert(room.clone(), offset.max(0));
        }
    }

    pub(crate) fn focus(&mut self, group: &RoomAuthorityStamp, room: &TaskStamp) {
        if let Some(view) = self.groups.iter_mut().find(|view| &view.group == group)
            && view.rooms.contains(room)
        {
            view.focus = Some(room.clone());
        }
    }

    /// Clear a posted room's draft only while it still holds the posted text.
    fn posted(&mut self, group: &RoomAuthorityStamp, room: &TaskStamp, text: &str) {
        if let Some(view) = self.groups.iter_mut().find(|view| &view.group == group)
            && view
                .drafts
                .get(room)
                .is_some_and(|draft| draft.trim() == text)
        {
            view.drafts.remove(room);
        }
    }

    /// Apply one local presentation change to the current state.
    pub(crate) fn apply(&mut self, action: RoomViewAction) {
        match action {
            RoomViewAction::Open { group, room } => self.open_room(group, room),
            RoomViewAction::CloseRoom { group, room } => self.close_room(&group, &room),
            RoomViewAction::CloseGroup { group } => self.close_group(&group),
            RoomViewAction::MoveGroup { group, delta } => self.move_group(&group, delta),
            RoomViewAction::MoveRoom { group, room, delta } => {
                self.move_room(&group, &room, delta);
            }
            RoomViewAction::Draft { group, room, draft } => self.set_draft(&group, &room, draft),
            RoomViewAction::Reader {
                group,
                room,
                offset,
            } => self.set_reader(&group, &room, offset),
            RoomViewAction::Focus { group, room } => self.focus(&group, &room),
            RoomViewAction::Posted { group, room, text } => self.posted(&group, &room, &text),
        }
    }
}

/// One local presentation change, applied to whatever state is current when
/// it lands rather than to the state its issuer last rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
pub(crate) enum RoomViewAction {
    Open {
        group: RoomAuthorityStamp,
        room: TaskStamp,
    },
    CloseRoom {
        group: RoomAuthorityStamp,
        room: TaskStamp,
    },
    CloseGroup {
        group: RoomAuthorityStamp,
    },
    MoveGroup {
        group: RoomAuthorityStamp,
        delta: isize,
    },
    MoveRoom {
        group: RoomAuthorityStamp,
        room: TaskStamp,
        delta: isize,
    },
    Draft {
        group: RoomAuthorityStamp,
        room: TaskStamp,
        draft: String,
    },
    Reader {
        group: RoomAuthorityStamp,
        room: TaskStamp,
        offset: i32,
    },
    Focus {
        group: RoomAuthorityStamp,
        room: TaskStamp,
    },
    /// A room post with this trimmed text was recorded.
    Posted {
        group: RoomAuthorityStamp,
        room: TaskStamp,
        text: String,
    },
}

impl GroupView {
    pub(crate) fn rooms(&self) -> &[TaskStamp] {
        &self.rooms
    }

    pub(crate) fn draft(&self, room: &TaskStamp) -> &str {
        self.drafts
            .get(room)
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub(crate) fn reader(&self, room: &TaskStamp) -> i32 {
        self.readers.get(room).copied().unwrap_or_default()
    }

    pub(crate) fn focused(&self, room: &TaskStamp) -> bool {
        self.focus.as_ref() == Some(room)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(text: &str) -> RoomAuthorityStamp {
        text.parse().expect("fixture group")
    }

    fn room(text: &str) -> TaskStamp {
        text.parse().expect("fixture room")
    }

    #[test]
    fn shared_group_changes_cannot_mount_reorder_or_close_local_views() {
        let first = group("dVsYdDj7d");
        let second = group("dVsZzx5S8");
        let a = room("dVrD59Y8N");
        let b = room("dVrD5vxwx");
        let mut views = RoomViews::default();
        views.open_room(first.clone(), a.clone());
        views.open_room(second.clone(), b.clone());
        views.set_draft(&first, &a, "unfinished".into());

        // No authority value enters this presentation model. A catalog update
        // therefore cannot mutate it; only explicit local actions below can.
        let restored = RoomViews::restore(&views.document().unwrap()).unwrap();
        assert_eq!(restored, views);
        assert_eq!(restored.groups()[0].group, first);
        assert_eq!(restored.groups()[0].draft(&a), "unfinished");

        views.close_group(&second);
        assert_eq!(views.groups().len(), 1);
    }

    #[test]
    fn room_identity_carries_draft_reader_and_focus_across_order_changes() {
        let group = group("dVsYdDj7d");
        let a = room("dVrD59Y8N");
        let b = room("dVrD5vxwx");
        let mut views = RoomViews::default();
        views.open_room(group.clone(), a.clone());
        views.open_room(group.clone(), b.clone());
        views.set_draft(&group, &a, "candidate".into());
        views.set_reader(&group, &a, 41);
        views.focus(&group, &a);
        views.move_room(&group, &a, 1);

        let restored = RoomViews::restore(&views.document().unwrap()).unwrap();
        let view = &restored.groups()[0];
        assert_eq!(view.rooms(), &[b, a.clone()]);
        assert_eq!(view.draft(&a), "candidate");
        assert_eq!(view.reader(&a), 41);
        assert!(view.focused(&a));
    }

    #[test]
    fn a_landing_post_keeps_later_changes_and_clears_only_an_unchanged_draft() {
        let group = group("dVsYdDj7d");
        let a = room("dVrD59Y8N");
        let b = room("dVrD5vxwx");
        let mut views = RoomViews::default();
        views.apply(RoomViewAction::Open {
            group: group.clone(),
            room: a.clone(),
        });
        let draft = |room: &TaskStamp, draft: &str| RoomViewAction::Draft {
            group: group.clone(),
            room: room.clone(),
            draft: draft.to_owned(),
        };
        let posted = |room: &TaskStamp| RoomViewAction::Posted {
            group: group.clone(),
            room: room.clone(),
            text: "ship it".to_owned(),
        };
        views.apply(draft(&a, " ship it\n"));
        // Changes made while the post is in flight.
        views.apply(RoomViewAction::Open {
            group: group.clone(),
            room: b.clone(),
        });
        views.apply(draft(&b, "elsewhere"));

        views.apply(posted(&a));
        let view = &views.groups()[0];
        assert_eq!(view.rooms(), &[a.clone(), b.clone()]);
        assert_eq!(view.draft(&a), "");
        assert_eq!(view.draft(&b), "elsewhere");
        assert!(view.focused(&b));

        views.apply(draft(&a, "ship it, and then more"));
        views.apply(posted(&a));
        assert_eq!(
            views.groups()[0].draft(&a),
            "ship it, and then more",
            "text typed after submitting survives the post's completion"
        );
    }
}
