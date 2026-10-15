//! Browser-local working-group console state.
//!
//! Working-group facts own saved organization, including which designs a group
//! holds and their order. This state owns presentation only: which group
//! consoles this browser opened, their order, each design composer's draft,
//! and the design composer holding focus. The one way shared organization
//! reaches it is a group ending: a completed snapshot that no longer holds a
//! group closes that group's console.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use wire::TaskStamp;
use wire::rooms::RoomAuthorityStamp;

/// Browser-local presentation for all explicitly opened group consoles.
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    drafts: BTreeMap<TaskStamp, String>,
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
        views
            .groups
            .retain(|group| groups.insert(group.group.clone()));
        Some(views)
    }

    pub(crate) fn document(&self) -> Option<String> {
        serde_json::to_string(self).ok()
    }

    pub(crate) fn groups(&self) -> &[GroupView] {
        &self.groups
    }

    fn view_mut(&mut self, group: &RoomAuthorityStamp) -> Option<&mut GroupView> {
        self.groups.iter_mut().find(|view| &view.group == group)
    }

    /// Mount a group console at the end, or keep an open one in place; an
    /// explicit design moves the console's focus to that design's composer.
    pub(crate) fn open_group(&mut self, group: RoomAuthorityStamp, design: Option<TaskStamp>) {
        if let Some(view) = self.view_mut(&group) {
            if design.is_some() {
                view.focus = design;
            }
            return;
        }
        self.groups.push(GroupView {
            group,
            drafts: BTreeMap::new(),
            focus: design,
        });
    }

    pub(crate) fn close_group(&mut self, group: &RoomAuthorityStamp) {
        self.groups.retain(|view| &view.group != group);
    }

    /// Close every console whose group `standing` no longer holds. `standing`
    /// is the group set of a completed room snapshot; `None`, while room
    /// authority loads or reconnects, closes nothing, so a transient gap keeps
    /// every console in place.
    pub(crate) fn close_ended(&mut self, standing: Option<&BTreeSet<RoomAuthorityStamp>>) {
        if let Some(standing) = standing {
            self.groups.retain(|view| standing.contains(&view.group));
        }
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

    pub(crate) fn set_draft(
        &mut self,
        group: &RoomAuthorityStamp,
        design: &TaskStamp,
        draft: String,
    ) {
        let Some(view) = self.view_mut(group) else {
            return;
        };
        if draft.is_empty() {
            view.drafts.remove(design);
        } else {
            view.drafts.insert(design.clone(), draft);
        }
    }

    pub(crate) fn focus(&mut self, group: &RoomAuthorityStamp, design: &TaskStamp) {
        if let Some(view) = self.view_mut(group) {
            view.focus = Some(design.clone());
        }
    }

    /// Clear a posted design's draft only while it still holds the posted
    /// text.
    fn posted(&mut self, group: &RoomAuthorityStamp, design: &TaskStamp, text: &str) {
        if let Some(view) = self.view_mut(group)
            && view
                .drafts
                .get(design)
                .is_some_and(|draft| draft.trim() == text)
        {
            view.drafts.remove(design);
        }
    }

    /// Apply one local presentation change to the current state.
    pub(crate) fn apply(&mut self, action: RoomViewAction) {
        match action {
            RoomViewAction::Open { group, design } => self.open_group(group, design),
            RoomViewAction::CloseGroup { group } => self.close_group(&group),
            RoomViewAction::Snapshot { standing } => self.close_ended(standing.as_ref()),
            RoomViewAction::MoveGroup { group, delta } => self.move_group(&group, delta),
            RoomViewAction::Draft {
                group,
                design,
                draft,
            } => self.set_draft(&group, &design, draft),
            RoomViewAction::Focus { group, design } => self.focus(&group, &design),
            RoomViewAction::Posted {
                group,
                design,
                text,
            } => self.posted(&group, &design, &text),
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
        design: Option<TaskStamp>,
    },
    CloseGroup {
        group: RoomAuthorityStamp,
    },
    /// Room authority now reads `standing`: the groups a completed snapshot
    /// holds, or `None` while it loads or reconnects.
    Snapshot {
        standing: Option<BTreeSet<RoomAuthorityStamp>>,
    },
    MoveGroup {
        group: RoomAuthorityStamp,
        delta: isize,
    },
    Draft {
        group: RoomAuthorityStamp,
        design: TaskStamp,
        draft: String,
    },
    Focus {
        group: RoomAuthorityStamp,
        design: TaskStamp,
    },
    /// A room post with this trimmed text was recorded.
    Posted {
        group: RoomAuthorityStamp,
        design: TaskStamp,
        text: String,
    },
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), allow(dead_code))]
impl GroupView {
    pub(crate) fn draft(&self, design: &TaskStamp) -> &str {
        self.drafts
            .get(design)
            .map(String::as_str)
            .unwrap_or_default()
    }

    pub(crate) fn focused(&self, design: &TaskStamp) -> bool {
        self.focus.as_ref() == Some(design)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(text: &str) -> RoomAuthorityStamp {
        text.parse().expect("fixture group")
    }

    fn design(text: &str) -> TaskStamp {
        text.parse().expect("fixture design")
    }

    #[test]
    fn shared_group_changes_cannot_mount_or_reorder_local_views() {
        let first = group("dVsYdDj7d");
        let second = group("dVsZzx5S8");
        let a = design("dVrD59Y8N");
        let mut views = RoomViews::default();
        views.open_group(first.clone(), None);
        views.open_group(second.clone(), Some(a.clone()));
        views.set_draft(&first, &a, "unfinished".into());
        views.move_group(&second, -1);

        // No membership or order change enters this presentation model; only
        // the explicit local actions below mount, move, or close a console.
        let restored = RoomViews::restore(&views.document().unwrap()).unwrap();
        assert_eq!(restored, views);
        assert_eq!(restored.groups()[0].group, second);
        assert!(restored.groups()[0].focused(&a));
        assert_eq!(restored.groups()[1].draft(&a), "unfinished");

        // Reopening an open group never moves it.
        views.open_group(first.clone(), None);
        assert_eq!(views.groups()[1].group, first);
        views.close_group(&second);
        assert_eq!(views.groups().len(), 1);
    }

    #[test]
    fn only_a_completed_snapshot_closes_an_ended_groups_console() {
        let ended = group("dVsYdDj7d");
        let live = group("dVsZzx5S8");
        let a = design("dVrD59Y8N");
        let mut views = RoomViews::default();
        views.open_group(ended.clone(), None);
        views.open_group(live.clone(), Some(a.clone()));
        views.set_draft(&live, &a, "unfinished".into());
        let before = views.clone();

        views.apply(RoomViewAction::Snapshot { standing: None });
        assert_eq!(views, before, "loading or reconnecting closes nothing");

        views.apply(RoomViewAction::Snapshot {
            standing: Some(BTreeSet::from([live.clone(), group("dVsZzx5S9")])),
        });
        assert_eq!(views.groups().len(), 1, "the ended group's console closed");
        let survivor = &views.groups()[0];
        assert_eq!(survivor.group, live);
        assert!(survivor.focused(&a));
        assert_eq!(survivor.draft(&a), "unfinished");
        assert_eq!(
            RoomViews::restore(&views.document().unwrap()).unwrap(),
            views,
            "the closed console is gone from what a reload restores"
        );

        views.apply(RoomViewAction::Snapshot {
            standing: Some(BTreeSet::new()),
        });
        assert!(views.groups().is_empty());
    }

    #[test]
    fn a_landing_post_keeps_later_changes_and_clears_only_an_unchanged_draft() {
        let group = group("dVsYdDj7d");
        let a = design("dVrD59Y8N");
        let b = design("dVrD5vxwx");
        let mut views = RoomViews::default();
        views.apply(RoomViewAction::Open {
            group: group.clone(),
            design: Some(a.clone()),
        });
        let draft = |design: &TaskStamp, draft: &str| RoomViewAction::Draft {
            group: group.clone(),
            design: design.clone(),
            draft: draft.to_owned(),
        };
        views.apply(draft(&a, " ship it\n"));
        // Changes made while the post is in flight.
        views.apply(draft(&b, "elsewhere"));
        views.apply(RoomViewAction::Focus {
            group: group.clone(),
            design: b.clone(),
        });

        let posted = || RoomViewAction::Posted {
            group: group.clone(),
            design: a.clone(),
            text: "ship it".to_owned(),
        };
        views.apply(posted());
        let view = &views.groups()[0];
        assert_eq!(view.draft(&a), "");
        assert_eq!(view.draft(&b), "elsewhere");
        assert!(view.focused(&b));

        views.apply(draft(&a, "ship it, and then more"));
        views.apply(posted());
        assert_eq!(
            views.groups()[0].draft(&a),
            "ship it, and then more",
            "text typed after submitting survives the post's completion"
        );
    }
}
