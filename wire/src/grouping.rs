//! One grouping model shared by app teams and working groups.
//!
//! A [`Partition`] places distinct members, in a durable order, into groups.
//! A group exists only while it holds a member, so it is never stored apart
//! from its members: groups stand in the order of their first member, and a
//! transition that empties a group ends it in the same value. Every member
//! belongs to at most one group, a group grows only up to [`MEMBER_LIMIT`],
//! and every change is admitted against an observed revision.
//!
//! The module is pure. A member kind supplies what differs between teams and
//! working groups through [`Kind`]: which members it admits, how it encodes a
//! member that stands in no formation, and the identity a group takes when it
//! must be renamed.

#[cfg(test)]
mod tests;

/// Most members a group may be formed or grown to hold.
///
/// An existing larger group may be restated or reduced, but never enlarged.
pub const MEMBER_LIMIT: usize = 6;

/// One member and the group it stands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement<G, M> {
    /// The placed member.
    pub member: M,
    /// The group holding it.
    pub group: G,
}

/// One group and its members, in arranged order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group<G, M> {
    /// Group identity.
    pub group: G,
    /// Members, member zero first.
    pub members: Vec<M>,
}

/// The ordered placement of distinct members into groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition<G, M> {
    placements: Vec<Placement<G, M>>,
}

/// What one member kind decides for itself.
pub trait Kind {
    /// Group identity.
    type Group: Copy + Eq;
    /// Member identity.
    type Member: Copy + Eq;

    /// Whether this member may be placed at all.
    fn admits(&self, member: Self::Member) -> bool;

    /// The group a member stands in alone when it belongs to no formation,
    /// or `None` when such a member is simply absent from the partition.
    fn ungrouped(&self, member: Self::Member) -> Option<Self::Group>;

    /// A fresh identity for the members remaining in `group` when `leaving`
    /// keeps that identity for itself.
    fn rekey(
        &self,
        partition: &Partition<Self::Group, Self::Member>,
        group: Self::Group,
        leaving: Self::Member,
    ) -> Self::Group;
}

/// Where one member moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination<G> {
    /// The group with this identity: joined when it holds members, founded
    /// with this identity when it holds none.
    Group(G),
    /// Out of every formation, as the kind encodes that.
    Ungrouped,
}

/// What one admitted move changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved<G> {
    /// The group the member stood in before, if any.
    pub from: Option<G>,
    /// The group the member stands in after, if any.
    pub to: Option<G>,
    /// The identity the source group's remaining members took, when the
    /// member kept the source identity for itself.
    pub rekeyed: Option<G>,
}

/// One admitted whole-partition arrangement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arranged<G, M> {
    /// The partition the arrangement states.
    pub partition: Partition<G, M>,
    /// Each fresh group, with the current group its one member split from.
    pub splits: Vec<(G, G)>,
}

/// Why a grouping change was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupingError<G, M> {
    /// A partition was built naming this member twice.
    Repeated(M),
    /// The kind does not admit this member.
    NotAdmitted(M),
    /// The move would leave the member where it stands.
    Unchanged,
    /// The change would grow a group past [`MEMBER_LIMIT`].
    MemberLimit {
        /// Members the group would hold.
        members: usize,
    },
    /// A rename chose an identity another group already holds.
    IdentityTaken(G),
    /// A member was placed before itself.
    PlaceSelf(M),
    /// The member to place stands in no group.
    Absent(M),
    /// The member to place before stands in no group.
    TargetAbsent(M),
    /// An arrangement stated no group.
    ArrangeEmpty,
    /// An arrangement named a group that is neither current nor a fresh
    /// singleton split from a current group of two or more.
    UnknownGroup(G),
    /// An arrangement stated one group twice.
    DuplicateGroup(G),
    /// An arrangement stated a group with no members.
    VacantGroup(G),
    /// An arrangement named a member the partition does not hold.
    UnknownMember(M),
    /// An arrangement placed one member twice.
    DuplicateMember(M),
    /// An arrangement dropped a current member.
    Incomplete {
        /// Members the arrangement had to state.
        expected: usize,
        /// The first current member it left out.
        missing: M,
    },
}

/// Why an observed revision cannot advance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevisionError {
    /// The partition moved since the change was observed.
    Moved {
        /// The current revision.
        current: u64,
    },
    /// The revision cannot advance further.
    Exhausted,
}

/// Admit a change observed at `observed` and answer the revision it commits.
///
/// # Errors
///
/// Refuses a stale observation, and a revision that cannot advance.
pub fn advance_revision(current: u64, observed: u64) -> Result<u64, RevisionError> {
    if observed != current {
        return Err(RevisionError::Moved { current });
    }
    current.checked_add(1).ok_or(RevisionError::Exhausted)
}

/// Admit a group holding `members` that held `current` before.
///
/// # Errors
///
/// Refuses growth past [`MEMBER_LIMIT`]; a larger group may stay or shrink.
pub fn admit_size<G, M>(current: usize, members: usize) -> Result<(), GroupingError<G, M>> {
    if members > MEMBER_LIMIT && members > current {
        return Err(GroupingError::MemberLimit { members });
    }
    Ok(())
}

impl<G: Copy + Eq, M: Copy + Eq> Partition<G, M> {
    /// The partition these placements state, in order.
    ///
    /// # Errors
    ///
    /// Refuses a member placed twice.
    pub fn new(
        placements: impl IntoIterator<Item = Placement<G, M>>,
    ) -> Result<Self, GroupingError<G, M>> {
        let placements = placements.into_iter().collect::<Vec<_>>();
        for (index, placement) in placements.iter().enumerate() {
            if placements[..index]
                .iter()
                .any(|earlier| earlier.member == placement.member)
            {
                return Err(GroupingError::Repeated(placement.member));
            }
        }
        Ok(Self { placements })
    }

    /// Every placement, in durable order.
    #[must_use]
    pub fn placements(&self) -> &[Placement<G, M>] {
        &self.placements
    }

    /// The group this member stands in.
    #[must_use]
    pub fn group_of(&self, member: M) -> Option<G> {
        self.placements
            .iter()
            .find(|placement| placement.member == member)
            .map(|placement| placement.group)
    }

    /// Whether any member stands in this group.
    #[must_use]
    pub fn holds(&self, group: G) -> bool {
        self.placements
            .iter()
            .any(|placement| placement.group == group)
    }

    /// The members of one group, member zero first.
    pub fn members(&self, group: G) -> impl Iterator<Item = M> + '_ {
        self.placements
            .iter()
            .filter(move |placement| placement.group == group)
            .map(|placement| placement.member)
    }

    /// The member a group's label derives from: member zero.
    #[must_use]
    pub fn lead(&self, group: G) -> Option<M> {
        self.members(group).next()
    }

    /// Every group in the order of its first member.
    #[must_use]
    pub fn groups(&self) -> Vec<Group<G, M>> {
        let mut groups = Vec::<Group<G, M>>::new();
        for placement in &self.placements {
            if let Some(group) = groups
                .iter_mut()
                .find(|group| group.group == placement.group)
            {
                group.members.push(placement.member);
            } else {
                groups.push(Group {
                    group: placement.group,
                    members: vec![placement.member],
                });
            }
        }
        groups
    }

    /// Move one member into a group, founding it when it holds none, or out
    /// of every formation.
    ///
    /// A joining member stands after the group's current members; a founding
    /// or departing member keeps its place. Dropping the member onto the group
    /// it already hosts under its own ungrouped identity splits it: the member
    /// keeps that identity alone, and the remaining members take the kind's
    /// fresh identity.
    ///
    /// # Errors
    ///
    /// Refuses an unadmitted member, a move that changes nothing, growth past
    /// the limit, and a rename onto an identity another group holds.
    pub fn apply_move<K>(
        &self,
        kind: &K,
        member: M,
        destination: Destination<G>,
    ) -> Result<(Self, Moved<G>), GroupingError<G, M>>
    where
        K: Kind<Group = G, Member = M>,
    {
        if !kind.admits(member) {
            return Err(GroupingError::NotAdmitted(member));
        }
        let from = self.group_of(member);
        let target = match destination {
            Destination::Group(group) => group,
            Destination::Ungrouped => match kind.ungrouped(member) {
                Some(group) => group,
                None => return self.ungroup(member, from),
            },
        };
        if from == Some(target) {
            return self.split(kind, member, target);
        }
        let mut next = self.clone();
        if self.holds(target) {
            let current = self.members(target).count();
            admit_size(current, current + 1)?;
            next.placements
                .retain(|placement| placement.member != member);
            let tail = next
                .placements
                .iter()
                .rposition(|placement| placement.group == target)
                .map_or(next.placements.len(), |last| last + 1);
            next.placements.insert(
                tail,
                Placement {
                    member,
                    group: target,
                },
            );
        } else if let Some(placement) = next
            .placements
            .iter_mut()
            .find(|placement| placement.member == member)
        {
            placement.group = target;
        } else {
            next.placements.push(Placement {
                member,
                group: target,
            });
        }
        Ok((
            next,
            Moved {
                from,
                to: Some(target),
                rekeyed: None,
            },
        ))
    }

    fn ungroup(&self, member: M, from: Option<G>) -> Result<(Self, Moved<G>), GroupingError<G, M>> {
        if from.is_none() {
            return Err(GroupingError::Unchanged);
        }
        let mut next = self.clone();
        next.placements
            .retain(|placement| placement.member != member);
        Ok((
            next,
            Moved {
                from,
                to: None,
                rekeyed: None,
            },
        ))
    }

    fn split<K>(
        &self,
        kind: &K,
        member: M,
        group: G,
    ) -> Result<(Self, Moved<G>), GroupingError<G, M>>
    where
        K: Kind<Group = G, Member = M>,
    {
        if kind.ungrouped(member) != Some(group) || self.members(group).nth(1).is_none() {
            return Err(GroupingError::Unchanged);
        }
        let rekeyed = kind.rekey(self, group, member);
        if rekeyed == group || self.holds(rekeyed) {
            return Err(GroupingError::IdentityTaken(rekeyed));
        }
        let mut next = self.clone();
        for placement in &mut next.placements {
            if placement.group == group && placement.member != member {
                placement.group = rekeyed;
            }
        }
        Ok((
            next,
            Moved {
                from: Some(group),
                to: Some(group),
                rekeyed: Some(rekeyed),
            },
        ))
    }

    /// Place one member immediately before another, in that member's group.
    ///
    /// # Errors
    ///
    /// Refuses placing a member before itself, either member standing in no
    /// group, and growth past the limit.
    pub fn place_before(
        &self,
        member: M,
        before: M,
    ) -> Result<(Self, Moved<G>), GroupingError<G, M>> {
        if member == before {
            return Err(GroupingError::PlaceSelf(member));
        }
        let Some(from) = self.group_of(member) else {
            return Err(GroupingError::Absent(member));
        };
        let Some(target) = self.group_of(before) else {
            return Err(GroupingError::TargetAbsent(before));
        };
        if from != target {
            let current = self.members(target).count();
            admit_size(current, current + 1)?;
        }
        let mut next = self.clone();
        next.placements
            .retain(|placement| placement.member != member);
        let at = next
            .placements
            .iter()
            .position(|placement| placement.member == before)
            .ok_or(GroupingError::TargetAbsent(before))?;
        next.placements.insert(
            at,
            Placement {
                member,
                group: target,
            },
        );
        Ok((
            next,
            Moved {
                from: Some(from),
                to: Some(target),
                rekeyed: None,
            },
        ))
    }

    /// The single current group of two or more that a proposed singleton
    /// group splits from.
    fn split_source(&self, proposed: &Group<G, M>) -> Option<G> {
        let [member] = proposed.members.as_slice() else {
            return None;
        };
        let source = self.group_of(*member)?;
        self.members(source).nth(1).map(|_| source)
    }

    /// Admit a faithful restatement of the whole partition: group order,
    /// members per group, and member order.
    ///
    /// A group absent from the partition is admitted only as a singleton split
    /// from a current group of two or more. Nothing is repaired by inference.
    ///
    /// # Errors
    ///
    /// Refuses an empty arrangement, an unknown, repeated, or vacant group,
    /// growth past the limit, an invented or repeated member, and a dropped
    /// member.
    pub fn arrange(&self, proposed: &[Group<G, M>]) -> Result<Arranged<G, M>, GroupingError<G, M>> {
        if proposed.is_empty() {
            return Err(GroupingError::ArrangeEmpty);
        }
        let mut groups = Vec::<G>::new();
        let mut placements = Vec::<Placement<G, M>>::with_capacity(self.placements.len());
        let mut splits = Vec::new();
        for group in proposed {
            if !self.holds(group.group) {
                let Some(source) = self.split_source(group) else {
                    return Err(GroupingError::UnknownGroup(group.group));
                };
                splits.push((group.group, source));
            }
            if groups.contains(&group.group) {
                return Err(GroupingError::DuplicateGroup(group.group));
            }
            groups.push(group.group);
            if group.members.is_empty() {
                return Err(GroupingError::VacantGroup(group.group));
            }
            admit_size(self.members(group.group).count(), group.members.len())?;
            for member in &group.members {
                if self.group_of(*member).is_none() {
                    return Err(GroupingError::UnknownMember(*member));
                }
                if placements.iter().any(|placed| placed.member == *member) {
                    return Err(GroupingError::DuplicateMember(*member));
                }
                placements.push(Placement {
                    member: *member,
                    group: group.group,
                });
            }
        }
        if let Some(missing) = self.placements.iter().find(|current| {
            !placements
                .iter()
                .any(|placed| placed.member == current.member)
        }) {
            return Err(GroupingError::Incomplete {
                expected: self.placements.len(),
                missing: missing.member,
            });
        }
        Ok(Arranged {
            partition: Self { placements },
            splits,
        })
    }
}
