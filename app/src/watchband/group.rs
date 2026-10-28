//! The accent deal of working groups and their designs.

use wire::TaskStamp;
use wire::rooms::{RoomAuthorityStamp, WorkingGroup};

use super::{DESIGN_BASE, group_accent};

/// The console rank of working `group`: how many of `groups` formed before
/// it. Formation age, never partition or browser order, ranks a group, so a
/// partition move recolours nothing.
#[must_use]
pub fn group_console(groups: &[WorkingGroup], group: &RoomAuthorityStamp) -> usize {
    groups
        .iter()
        .filter(|candidate| {
            (candidate.group.micros(), candidate.group.as_str()) < (group.micros(), group.as_str())
        })
        .count()
}

/// The accent slot `design` wears wherever it is named: the deal of its place
/// in its group's console, else of its place in the `ungrouped` list, which
/// deals from the palette's end backwards as the first group console does, the
/// way unassigned agents deal from its start. A design in neither list wears
/// [`DESIGN_BASE`].
#[must_use]
pub fn design_accent(
    groups: &[WorkingGroup],
    ungrouped: &[TaskStamp],
    design: &TaskStamp,
) -> usize {
    groups
        .iter()
        .find_map(|group| {
            let position = group.anchors.iter().position(|anchor| anchor == design)?;
            Some(group_accent(group_console(groups, &group.group), position))
        })
        .or_else(|| {
            ungrouped
                .iter()
                .position(|candidate| candidate == design)
                .map(|position| group_accent(0, position))
        })
        .unwrap_or(DESIGN_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(micros: i64) -> TaskStamp {
        TaskStamp::for_micros(micros).expect("fixture design")
    }

    fn group(micros: i64, anchors: &[i64]) -> WorkingGroup {
        WorkingGroup {
            group: RoomAuthorityStamp::for_micros(micros).expect("fixture group"),
            anchors: anchors.iter().copied().map(stamp).collect(),
        }
    }

    #[test]
    fn a_design_wears_its_places_deal_in_its_groups_age_ranked_console() {
        // The younger group stands first in the partition yet ranks second.
        let groups = [
            group(
                1_700_000_000_001_000,
                &[1_600_000_000_000_001, 1_600_000_000_000_002],
            ),
            group(1_700_000_000_000_000, &[1_600_000_000_000_003]),
        ];
        assert_eq!(group_console(&groups, &groups[1].group), 0);
        assert_eq!(group_console(&groups, &groups[0].group), 1);

        assert_eq!(
            design_accent(&groups, &[], &stamp(1_600_000_000_000_002)),
            group_accent(1, 1)
        );
        assert_eq!(
            design_accent(&groups, &[], &stamp(1_600_000_000_000_003)),
            group_accent(0, 0)
        );
        assert_eq!(
            design_accent(&groups, &[], &stamp(1_600_000_000_000_004)),
            DESIGN_BASE
        );
    }

    #[test]
    fn ungrouped_designs_deal_the_palette_backwards_from_plum_by_list_position() {
        let ungrouped = (0..7)
            .map(|offset| stamp(1_600_000_000_000_010 + offset))
            .collect::<Vec<_>>();
        let dealt = ungrouped
            .iter()
            .map(|design| design_accent(&[], &ungrouped, design))
            .collect::<Vec<_>>();
        assert_eq!(dealt, [DESIGN_BASE, 4, 3, 2, 1, 0, DESIGN_BASE]);
    }
}
