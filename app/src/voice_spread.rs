//! Spread agents across a device's voices when no voice list is configured.
//!
//! The assignment reads only the roster and the device's voice list, so every
//! page with the same roster and voices hears the same agent in the same voice.
//! Teams and their members are visited in identity order, so reordering a team
//! or the roster changes nothing. Teammates are what must sound different:
//! within one team no voice repeats until the team outnumbers the pool. An
//! agent keeps the voice its identity hashes to unless a teammate holds it,
//! even when another team holds it too; only an agent a teammate displaced
//! prefers, among the voices its team leaves free, one no other team holds.

use std::collections::BTreeSet;

use wire::CanonicalUuid;

/// One published voice as the pool reads it.
#[derive(Clone, Copy, Debug)]
pub struct OfferedVoice<'a> {
    pub name: &'a str,
    pub lang: &'a str,
    /// The device marks this voice as its default.
    pub default: bool,
}

/// Indices into `offered` forming the spread pool, ordered by name.
///
/// The pool keeps voices sharing the language of the device's default voice,
/// or of `device_language` when no voice is marked default. A name published
/// twice enters once.
#[must_use]
pub fn pool(offered: &[OfferedVoice<'_>], device_language: Option<&str>) -> Vec<usize> {
    let Some(language) = offered
        .iter()
        .find(|voice| voice.default)
        .map(|voice| voice.lang)
        .or(device_language)
        .map(primary_language)
    else {
        return Vec::new();
    };
    let mut pool: Vec<usize> = (0..offered.len())
        .filter(|&index| primary_language(offered[index].lang) == language)
        .collect();
    pool.sort_by(|&left, &right| offered[left].name.cmp(offered[right].name));
    pool.dedup_by(|right, left| offered[*left].name == offered[*right].name);
    pool
}

/// The primary language subtag, so `en-GB`, `en_US`, and `EN` all read `en`.
fn primary_language(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// The pool slot `member` speaks with among `teams`, given `size` voices.
///
/// A member absent from the roster keeps the slot its identity hashes to.
#[must_use]
pub fn voice_slot(
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    member: CanonicalUuid,
    size: usize,
) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let mut ordered: Vec<(CanonicalUuid, Vec<CanonicalUuid>)> = teams.to_vec();
    ordered.sort_by_key(|(team, _)| *team);
    let mut elsewhere = BTreeSet::new();
    for (_, mut members) in ordered {
        members.sort_unstable();
        let mut held = BTreeSet::new();
        for candidate in members {
            let preferred = preferred_slot(candidate, size);
            let probe = |free: &dyn Fn(usize) -> bool| {
                (0..size)
                    .map(|step| (preferred + step) % size)
                    .find(|&slot| free(slot))
            };
            let slot = if held.contains(&preferred) {
                probe(&|slot| !held.contains(&slot) && !elsewhere.contains(&slot))
                    .or_else(|| probe(&|slot| !held.contains(&slot)))
                    .unwrap_or(preferred)
            } else {
                preferred
            };
            if candidate == member {
                return Some(slot);
            }
            held.insert(slot);
        }
        elsewhere.extend(held);
    }
    Some(preferred_slot(member, size))
}

/// The slot one identity hashes to, mixed so neighbouring identities spread.
fn preferred_slot(member: CanonicalUuid, size: usize) -> usize {
    let bits = member.get().as_u128();
    let mut mixed = (bits >> 64) as u64 ^ bits as u64;
    mixed ^= mixed >> 30;
    mixed = mixed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed ^= mixed >> 27;
    mixed = mixed.wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^= mixed >> 31;
    (mixed % size as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::{OfferedVoice, pool, preferred_slot, voice_slot};
    use wire::CanonicalUuid;

    fn id(suffix: u16) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{suffix:012}")
            .parse()
            .expect("canonical test UUID")
    }

    fn slots(
        teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
        members: &[CanonicalUuid],
        size: usize,
    ) -> Vec<usize> {
        members
            .iter()
            .map(|member| voice_slot(teams, *member, size).expect("pool is nonempty"))
            .collect()
    }

    fn distinct(slots: &[usize]) -> bool {
        let mut seen = slots.to_vec();
        seen.sort_unstable();
        seen.dedup();
        seen.len() == slots.len()
    }

    #[test]
    fn teammates_hear_distinct_voices_while_the_pool_has_room() {
        for size in 2..=9 {
            for team_size in 1..=size {
                let members: Vec<_> = (0..team_size).map(|n| id(100 + n as u16)).collect();
                let teams = [(id(1), members.clone())];
                assert!(
                    distinct(&slots(&teams, &members, size)),
                    "{team_size} teammates over {size} voices repeated a voice"
                );
            }
        }
    }

    #[test]
    fn an_agent_keeps_its_own_voice_unless_a_teammate_holds_it() {
        let others: Vec<_> = (0..5).map(|n| id(200 + n)).collect();
        for size in [2, 3, 5, 8] {
            for n in 0..6 {
                let alone = id(300 + n);
                let without = voice_slot(&[(id(9), vec![alone])], alone, size);
                let beside = voice_slot(
                    &[(id(1), others.clone()), (id(9), vec![alone])],
                    alone,
                    size,
                );
                assert_eq!(without, Some(preferred_slot(alone, size)));
                assert_eq!(beside, without, "another team moved a lone agent's voice");
            }
        }
    }

    #[test]
    fn a_displaced_teammate_prefers_a_voice_no_other_team_holds() {
        let size = 3;
        let ids: Vec<_> = (0..64).map(|n| id(400 + n)).collect();
        let (first, second) = ids
            .iter()
            .flat_map(|left| ids.iter().map(move |right| (*left, *right)))
            .find(|(left, right)| {
                left < right && preferred_slot(*left, size) == preferred_slot(*right, size)
            })
            .expect("some identities share a slot");
        let shared = preferred_slot(first, size);
        let neighbour = ids
            .iter()
            .copied()
            .find(|other| preferred_slot(*other, size) == (shared + 1) % size)
            .expect("some identity hashes to the next slot");
        let teams = [(id(1), vec![neighbour]), (id(2), vec![second, first])];
        assert_eq!(voice_slot(&teams, first, size), Some(shared));
        assert_eq!(voice_slot(&teams, second, size), Some((shared + 2) % size));
    }

    #[test]
    fn reordering_members_or_teams_keeps_every_voice() {
        let first: Vec<_> = (0..4).map(|n| id(400 + n)).collect();
        let second: Vec<_> = (0..3).map(|n| id(500 + n)).collect();
        let everyone: Vec<_> = first.iter().chain(&second).copied().collect();
        let teams = [(id(1), first.clone()), (id(2), second.clone())];
        let mut reversed_first = first.clone();
        reversed_first.reverse();
        let mut reversed_second = second.clone();
        reversed_second.rotate_left(1);
        let reordered = [(id(2), reversed_second), (id(1), reversed_first)];
        for size in [3, 5, 7, 12] {
            assert_eq!(
                slots(&teams, &everyone, size),
                slots(&reordered, &everyone, size)
            );
        }
    }

    #[test]
    fn a_team_larger_than_the_pool_uses_every_voice_before_sharing() {
        let members: Vec<_> = (0..7).map(|n| id(600 + n)).collect();
        let teams = [(id(1), members.clone())];
        let mut heard = slots(&teams, &members, 4);
        heard.sort_unstable();
        heard.dedup();
        assert_eq!(heard, vec![0, 1, 2, 3]);
    }

    #[test]
    fn an_empty_pool_offers_no_voice() {
        assert_eq!(voice_slot(&[(id(1), vec![id(2)])], id(2), 0), None);
    }

    #[test]
    fn the_pool_keeps_the_default_voices_language_ordered_by_name() {
        let offered = [
            OfferedVoice {
                name: "Samantha",
                lang: "en-US",
                default: true,
            },
            OfferedVoice {
                name: "Kyoko",
                lang: "ja-JP",
                default: false,
            },
            OfferedVoice {
                name: "Daniel",
                lang: "en_GB",
                default: false,
            },
            OfferedVoice {
                name: "Amélie",
                lang: "fr-CA",
                default: false,
            },
            OfferedVoice {
                name: "Daniel",
                lang: "en-GB",
                default: false,
            },
            OfferedVoice {
                name: "Karen",
                lang: "EN-au",
                default: false,
            },
        ];
        let names: Vec<_> = pool(&offered, Some("fr-FR"))
            .into_iter()
            .map(|index| offered[index].name)
            .collect();
        assert_eq!(names, ["Daniel", "Karen", "Samantha"]);
    }

    #[test]
    fn without_a_default_voice_the_device_language_chooses_the_pool() {
        let offered = [
            OfferedVoice {
                name: "Thomas",
                lang: "fr-FR",
                default: false,
            },
            OfferedVoice {
                name: "Alex",
                lang: "en-US",
                default: false,
            },
        ];
        let names: Vec<_> = pool(&offered, Some("fr"))
            .into_iter()
            .map(|index| offered[index].name)
            .collect();
        assert_eq!(names, ["Thomas"]);
        assert!(pool(&offered, None).is_empty());
    }
}
