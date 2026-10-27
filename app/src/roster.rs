//! One roster for both grouping kinds: teams of agent lanes and working
//! groups of Design tasks.
//!
//! A roster is what the global menu draws and the operator drags: the formed
//! groups in order, each with its members, member zero first and naming the
//! group, and the members that stand in no group. Every gesture resolves here
//! to at most one [`Change`] for the kind's authority to admit at the revision
//! the roster was drawn from: one member's move, or a complete restatement of
//! group and member order. The authority decides membership; the roster
//! states nothing for a gesture that changes nothing or that a full group
//! could not admit.

use std::collections::{BTreeMap, BTreeSet};

use wire::grouping::MEMBER_LIMIT;
use wire::intent::BoardTask;
use wire::rooms::{RoomAuthorityStamp, WorkingGroupPartition};
use wire::{CanonicalUuid, TaskStamp};

/// One group as the kind's arrangement restates it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot<G, M> {
    group: G,
    members: Vec<M>,
    /// Whether the roster draws it as a group. A kind that seats an ungrouped
    /// member in a group of its own keeps that group unformed.
    formed: bool,
}

/// The formed groups and ungrouped members one roster draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roster<G, M> {
    /// Every group an arrangement restates, in order.
    slots: Vec<Slot<G, M>>,
    /// Members in no formed group, in the order the roster lists them.
    ungrouped: Vec<M>,
}

/// What one gesture lifts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lift<G, M> {
    /// One member's tile.
    Member(M),
    /// One formed group's card.
    Group(G),
}

/// Which half of a group card or member tile a release falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

/// What the element under a release states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target<G, M> {
    /// The New target: a member released here founds a group.
    New,
    /// The ungrouped list: a grouped member released here leaves its group.
    Ungrouped,
    /// A formed group's card.
    Group(G, Side),
    /// A grouped member's tile.
    Member(M, Side),
}

/// Where one member moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination<G> {
    /// A group founded with a fresh identity.
    New,
    /// An existing formed group, after its current members.
    Group(G),
    /// Out of every group.
    Ungrouped,
}

/// The one change a gesture asks the authority for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change<G, M> {
    /// Move one member.
    Move { member: M, to: Destination<G> },
    /// Restate every group the kind arranges, in order, each with its members
    /// in order.
    Arrange(Vec<(G, Vec<M>)>),
}

/// A member's composer-menu actions: begin a group of its own, or leave its
/// group. Each is absent when it would change nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu<G, M> {
    pub new_group: Option<Change<G, M>>,
    pub leave: Option<Change<G, M>>,
}

impl<G: Clone + Eq, M: Clone + Eq> Roster<G, M> {
    /// A kind that seats every member: `formed` says which groups the roster
    /// draws, and the members of every other group are listed ungrouped.
    pub fn seated(groups: Vec<(G, Vec<M>)>, formed: impl Fn(&G, &[M]) -> bool) -> Self {
        let slots = groups
            .into_iter()
            .map(|(group, members)| Slot {
                formed: formed(&group, &members),
                group,
                members,
            })
            .collect::<Vec<_>>();
        let ungrouped = slots
            .iter()
            .filter(|slot| !slot.formed)
            .flat_map(|slot| slot.members.iter().cloned())
            .collect();
        Self { slots, ungrouped }
    }

    /// A kind whose groups hold only grouped members: every group is drawn,
    /// and each of `members` no group holds is listed ungrouped.
    pub fn partitioned(groups: Vec<(G, Vec<M>)>, members: impl IntoIterator<Item = M>) -> Self {
        let slots = groups
            .into_iter()
            .map(|(group, members)| Slot {
                group,
                members,
                formed: true,
            })
            .collect::<Vec<Slot<G, M>>>();
        let ungrouped = members
            .into_iter()
            .filter(|member| !slots.iter().any(|slot| slot.members.contains(member)))
            .collect();
        Self { slots, ungrouped }
    }

    /// Every formed group in order with its members, member zero first.
    pub fn groups(&self) -> impl Iterator<Item = (&G, &[M])> {
        self.slots
            .iter()
            .filter(|slot| slot.formed)
            .map(|slot| (&slot.group, slot.members.as_slice()))
    }

    /// Members in no formed group.
    #[must_use]
    pub fn ungrouped(&self) -> &[M] {
        &self.ungrouped
    }

    /// The formed group holding `member`.
    #[must_use]
    pub fn group_of(&self, member: &M) -> Option<&G> {
        self.formed(member).map(|slot| &slot.group)
    }

    /// Whether a member from elsewhere may join `group`: it is formed and not
    /// full. A full group's drop target is disabled.
    #[must_use]
    pub fn accepts(&self, group: &G) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.formed && slot.group == *group && slot.members.len() < MEMBER_LIMIT)
    }

    /// The change releasing `lift` over `target` asks for, if any.
    #[must_use]
    pub fn landing(&self, lift: &Lift<G, M>, target: &Target<G, M>) -> Option<Change<G, M>> {
        match lift {
            Lift::Member(member) => self.member_landing(member, target),
            Lift::Group(group) => self.group_landing(group, target),
        }
    }

    /// The composer-menu actions for `member`.
    #[must_use]
    pub fn menu(&self, member: &M) -> Menu<G, M> {
        let slot = self.formed(member);
        let moving = |to| Change::Move {
            member: member.clone(),
            to,
        };
        Menu {
            new_group: slot
                .filter(|slot| slot.members.len() > 1)
                .map(|_| moving(Destination::New)),
            leave: slot.map(|_| moving(Destination::Ungrouped)),
        }
    }

    fn formed(&self, member: &M) -> Option<&Slot<G, M>> {
        self.slots
            .iter()
            .find(|slot| slot.formed && slot.members.contains(member))
    }

    fn member_landing(&self, member: &M, target: &Target<G, M>) -> Option<Change<G, M>> {
        let from = self.formed(member);
        let to = match target {
            // A group's only member already stands alone.
            Target::New if from.is_some_and(|slot| slot.members.len() == 1) => return None,
            Target::New => Destination::New,
            Target::Ungrouped => {
                from?;
                Destination::Ungrouped
            }
            Target::Group(group, _) => Destination::Group(group.clone()),
            Target::Member(other, side) => {
                let at = self.formed(other)?;
                if from.is_some_and(|from| from.group == at.group) {
                    return self.reorder_member(member, other, *side);
                }
                Destination::Group(at.group.clone())
            }
        };
        if let Destination::Group(group) = &to
            && (from.is_some_and(|from| from.group == *group) || !self.accepts(group))
        {
            return None;
        }
        Some(Change::Move {
            member: member.clone(),
            to,
        })
    }

    fn reorder_member(&self, member: &M, before: &M, side: Side) -> Option<Change<G, M>> {
        if member == before {
            return None;
        }
        let mut slots = self.slots.clone();
        let slot = slots
            .iter_mut()
            .find(|slot| slot.members.contains(member))?;
        slot.members.retain(|current| current != member);
        let at = slot.members.iter().position(|current| current == before)?;
        let at = match side {
            Side::Before => at,
            Side::After => at + 1,
        };
        slot.members.insert(at, member.clone());
        self.arranged(slots)
    }

    fn group_landing(&self, group: &G, target: &Target<G, M>) -> Option<Change<G, M>> {
        let (at, side) = match target {
            Target::Group(at, side) => (at, *side),
            Target::Member(member, side) => (&self.formed(member)?.group, *side),
            Target::New | Target::Ungrouped => return None,
        };
        if at == group {
            return None;
        }
        let mut slots = self.slots.clone();
        let from = slots
            .iter()
            .position(|slot| slot.formed && slot.group == *group)?;
        let lifted = slots.remove(from);
        let at = slots
            .iter()
            .position(|slot| slot.formed && slot.group == *at)?;
        let at = match side {
            Side::Before => at,
            Side::After => at + 1,
        };
        slots.insert(at, lifted);
        self.arranged(slots)
    }

    fn arranged(&self, slots: Vec<Slot<G, M>>) -> Option<Change<G, M>> {
        (slots != self.slots).then(|| {
            Change::Arrange(
                slots
                    .into_iter()
                    .map(|slot| (slot.group, slot.members))
                    .collect(),
            )
        })
    }
}

/// The team roster: every lane row, drawing the rows that are teams and
/// listing every lane seated alone under its own identity as unassigned.
#[must_use]
pub fn teams(
    topology: &[(CanonicalUuid, Vec<CanonicalUuid>)],
) -> Roster<CanonicalUuid, CanonicalUuid> {
    Roster::seated(topology.to_vec(), |team, members| {
        wire::head::teamed(*team, members.iter().copied())
    })
}

/// The working-group roster: the partition's groups, and every Design task on
/// the board that no group holds, in board order.
#[must_use]
pub fn working_groups(
    partition: &WorkingGroupPartition,
    tasks: &[BoardTask],
) -> Roster<RoomAuthorityStamp, TaskStamp> {
    Roster::partitioned(
        partition
            .groups
            .iter()
            .map(|group| (group.group.clone(), group.anchors.clone()))
            .collect(),
        designs(tasks).map(|task| task.statement.body.born.clone()),
    )
}

/// Every open Design task a working group could hold, in board order.
pub fn designs(tasks: &[BoardTask]) -> impl Iterator<Item = &BoardTask> {
    tasks.iter().filter(|task| {
        wire::intent::is_open_design(task.statement.body.intent, &task.workflow.body)
    })
}

/// Distinct lanes holding live claims on work associated with one Design.
///
/// One lane can reach the Design through more than one claimed row over time;
/// the pip and group controls still count that worker once.
#[must_use]
pub fn design_claimants(
    board: &crate::runtime::BoardView,
    design: &TaskStamp,
) -> BTreeSet<CanonicalUuid> {
    board
        .tasks
        .iter()
        .filter_map(|row| {
            let claim = row
                .claim
                .as_ref()
                .filter(|claim| claim.body.disposition == wire::ClaimDisposition::Active)?;
            row.statement
                .body
                .is_associated_with_design(design)
                .then_some(claim.body.lane)
        })
        .collect()
}

/// The lanes holding live claims on work associated with each working group's
/// Design anchors. Association survives output selection through the task's
/// persistent preparation controller.
#[must_use]
pub fn claimants(
    partition: &WorkingGroupPartition,
    board: &crate::runtime::BoardView,
) -> BTreeMap<RoomAuthorityStamp, BTreeSet<CanonicalUuid>> {
    partition
        .groups
        .iter()
        .map(|group| {
            let lanes = group
                .anchors
                .iter()
                .flat_map(|design| design_claimants(board, design))
                .collect();
            (group.group.clone(), lanes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::tests::{board, claim, design, stamp, task};
    use wire::ClaimDisposition;

    fn lane(value: u64) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{value:012}")
            .parse()
            .expect("fixture lane")
    }

    fn activate(row: &mut BoardTask, lane: CanonicalUuid) {
        let mut active = claim(row, ClaimDisposition::Active);
        active.body.lane = lane;
        row.claim = Some(active);
    }

    #[test]
    fn group_claimants_cover_gated_and_selected_work_across_anchor_standing() {
        let first = stamp(1);
        let second = stamp(4);
        let mut first_design = design(1, "design.first");
        activate(&mut first_design, lane(1));

        let mut selected = task(2, "work.selected");
        selected.statement.body.preparation = Some(wire::TaskPreparation {
            parent: first.clone(),
            controller: first.clone(),
            construction: stamp(2),
        });
        activate(&mut selected, lane(2));

        let mut gated = task(3, "work.gated");
        gated.statement.body.dependencies = vec![first.clone()];
        activate(&mut gated, lane(3));

        let unavailable_anchor = task(4, "design.unavailable");
        let mut other_selected = task(5, "work.other");
        other_selected.statement.body.preparation = Some(wire::TaskPreparation {
            parent: second.clone(),
            controller: second.clone(),
            construction: stamp(5),
        });
        activate(&mut other_selected, lane(4));

        let mut duplicate_selected = task(7, "work.duplicate");
        duplicate_selected.statement.body.preparation = Some(wire::TaskPreparation {
            parent: first.clone(),
            controller: first.clone(),
            construction: stamp(7),
        });
        activate(&mut duplicate_selected, lane(2));

        let mut unrelated = task(6, "work.unrelated");
        activate(&mut unrelated, lane(5));

        let first_group = RoomAuthorityStamp::for_micros(100).expect("fixture first group");
        let second_group = RoomAuthorityStamp::for_micros(200).expect("fixture second group");
        let partition = WorkingGroupPartition {
            revision: 1,
            groups: vec![
                wire::rooms::WorkingGroup {
                    group: first_group.clone(),
                    anchors: vec![first.clone()],
                },
                wire::rooms::WorkingGroup {
                    group: second_group.clone(),
                    anchors: vec![second.clone()],
                },
            ],
            fast: Vec::new(),
        };
        let board = board(vec![
            first_design,
            selected,
            gated,
            unavailable_anchor,
            other_selected,
            unrelated,
            duplicate_selected,
        ]);
        let workers = claimants(&partition, &board);

        assert_eq!(
            workers.get(&first_group),
            Some(&BTreeSet::from([lane(1), lane(2), lane(3)]))
        );
        assert_eq!(workers.get(&second_group), Some(&BTreeSet::from([lane(4)])));
        assert_eq!(
            design_claimants(&board, &first),
            BTreeSet::from([lane(1), lane(2), lane(3)]),
            "the Design, selected work, and gated work contribute each distinct lane once",
        );
        assert_eq!(
            design_claimants(&board, &second),
            BTreeSet::from([lane(4)]),
            "another Design selects only its own worker",
        );
    }
}
