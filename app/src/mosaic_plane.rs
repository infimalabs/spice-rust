//! The production bridge from one live lane feed to the pure Mosaic engine.
//!
//! Browser rendering owns measurement and markup. This module owns the one
//! boundary that must not disappear during view rewrites: every lane submits
//! its measured candidates to [`crate::mosaic::pack`] and receives a plane
//! whose extent comes from the same engine result.

use crate::mosaic::{self, Candidate, Card, TRACK_COUNT};

/// One measured card offered to a persistent lane plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredCard<K> {
    /// Stable message or directive identity.
    pub key: K,
    /// Policy candidates used by insertion and explicit full replay.
    pub candidates: Vec<Candidate>,
    /// Fresh measurement at the already-latched span, when distinct from the
    /// policy candidates.
    pub latched: Option<Candidate>,
    /// Semantic content epoch that must commit even when the resulting row
    /// reservation is unchanged, such as an image leaving its placeholder.
    pub content_revision: u64,
    /// Whether this card starts a new chronological placement epoch.
    pub boundary: bool,
}

/// Why the most recent committed plane changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transition {
    /// One or more newly observed cards were appended to the live tail.
    Append,
    /// Older retained cards were placed below the committed plane.
    Backfill,
    /// Existing content changed while horizontal placement stayed latched.
    ContentResize,
    /// Geometry or membership removal required an explicit complete replay.
    FullReplay,
}

impl Transition {
    /// Stable browser-instrumentation spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Backfill => "backfill",
            Self::ContentResize => "content-resize",
            Self::FullReplay => "full-replay",
        }
    }
}

/// One keyed placement retained across browser renders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaneCard<K> {
    /// Stable message or directive identity.
    pub key: K,
    /// Fresh measurement at the latched span, if it is outside policy tiers.
    pub latched: Option<Candidate>,
    /// Last semantic content epoch incorporated into this placement.
    pub content_revision: u64,
    /// Whether this card starts a new chronological placement epoch.
    pub boundary: bool,
    /// Committed lattice placement.
    pub card: Card,
}

/// Stateful live plane layered over the pure Mosaic transition functions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PersistentLanePlane<K> {
    geometry: i32,
    cards: Vec<PlaneCard<K>>,
    max_row: i32,
    min_base: i32,
    commits: u64,
    transition: Transition,
}

impl<K> PersistentLanePlane<K> {
    /// Measurable content width for which this plane was committed.
    #[must_use]
    pub const fn geometry(&self) -> i32 {
        self.geometry
    }

    /// Keyed cards in stable creation order.
    #[must_use]
    pub fn cards(&self) -> &[PlaneCard<K>] {
        &self.cards
    }

    /// Highest occupied lattice row.
    #[must_use]
    pub const fn max_row(&self) -> i32 {
        self.max_row
    }

    /// Number of rows the CSS grid must expose.
    #[must_use]
    pub fn rows(&self) -> i32 {
        (self.max_row - self.min_base).max(1)
    }

    /// Number of non-noop plane commits since this component mounted.
    #[must_use]
    pub const fn commits(&self) -> u64 {
        self.commits
    }

    /// Transition used by the most recent commit.
    #[must_use]
    pub const fn transition(&self) -> Transition {
        self.transition
    }
}

impl<K: Clone + Eq> PersistentLanePlane<K> {
    /// Create the first committed plane at one measurable geometry.
    #[must_use]
    pub fn new(geometry: i32, measured: &[MeasuredCard<K>]) -> Self {
        Self::full_replay(geometry, measured, 1)
    }

    /// Reconcile a complete measured snapshot through the narrowest live path.
    #[must_use]
    pub fn reconcile(&self, geometry: i32, measured: &[MeasuredCard<K>]) -> Self {
        assert_unique(measured);
        if geometry != self.geometry
            || self
                .cards
                .iter()
                .any(|entry| measured.iter().all(|item| item.key != entry.key))
            || self.cards.iter().any(|entry| {
                measured
                    .iter()
                    .find(|item| item.key == entry.key)
                    .is_some_and(|item| item.boundary != entry.boundary)
            })
        {
            return Self::full_replay(geometry, measured, self.commits + 1);
        }

        let mut next = self.clone();
        let changed_content = apply_content(&mut next.cards, measured);
        let new_items = measured
            .iter()
            .filter(|item| next.cards.iter().all(|entry| entry.key != item.key))
            .cloned()
            .collect::<Vec<_>>();
        let backfill_count = prefix_backfill_count(&self.cards, measured);
        let mut transition = changed_content.then_some(Transition::ContentResize);
        if backfill_count > 0 {
            apply_backfill(&mut next.cards, &measured[..backfill_count]);
            transition = Some(Transition::Backfill);
        }
        let appended = if backfill_count > 0 {
            new_items
                .into_iter()
                .skip(backfill_count)
                .collect::<Vec<_>>()
        } else {
            new_items
        };
        if !appended.is_empty() {
            apply_append(&mut next.cards, &appended);
            transition = Some(Transition::Append);
        }
        let Some(transition) = transition else {
            return self.clone();
        };
        commit(next, transition)
    }

    fn full_replay(geometry: i32, measured: &[MeasuredCard<K>], commits: u64) -> Self {
        assert_unique(measured);
        let cards = mosaic::pack(
            &measured
                .iter()
                .map(|item| item.candidates.clone())
                .collect::<Vec<_>>(),
            TRACK_COUNT,
        );
        let mut cards = measured
            .iter()
            .zip(cards)
            .map(|(item, card)| PlaneCard {
                key: item.key.clone(),
                latched: item.latched,
                content_revision: item.content_revision,
                boundary: item.boundary,
                card,
            })
            .collect::<Vec<_>>();
        freeze_completed_epochs(&mut cards);
        assert_boundary_floors(&cards);
        let (max_row, min_base) = mosaic::extent(&placed_cards(&cards));
        Self {
            geometry,
            cards,
            max_row,
            min_base,
            commits,
            transition: Transition::FullReplay,
        }
    }
}

fn apply_content<K: Clone + Eq>(
    entries: &mut [PlaneCard<K>],
    measured: &[MeasuredCard<K>],
) -> bool {
    let mut cards = placed_cards(entries);
    let mut changed = false;
    for entry in entries.iter_mut() {
        let current = measured
            .iter()
            .find(|item| item.key == entry.key)
            .expect("removed keys take the full-replay path");
        if current.candidates == entry.card.candidates
            && current.latched == entry.latched
            && current.content_revision == entry.content_revision
        {
            continue;
        }
        changed = true;
        entry.latched = current.latched;
        entry.content_revision = current.content_revision;
        let index = cards
            .iter()
            .position(|card| card.creation_index == entry.card.creation_index)
            .expect("persistent plane keeps every keyed placement");
        let desired = current
            .latched
            .filter(|candidate| candidate.span == cards[index].span)
            .or_else(|| {
                current
                    .candidates
                    .iter()
                    .copied()
                    .find(|candidate| candidate.span == cards[index].span)
            })
            .expect("a content resize measures the committed Mosaic span");
        cards[index].candidates = current.candidates.clone();
        if cards[index].lifecycle == mosaic::Lifecycle::Frozen {
            cards =
                mosaic::resolve_frozen_resize(&cards, cards[index].creation_index, desired.rows);
        } else {
            cards[index].rows = desired.rows;
        }
    }
    if changed {
        cards = mosaic::replay_liquid(&cards, TRACK_COUNT, mosaic::FREEZE_DEPTH);
        replace_cards(entries, &cards);
    }
    changed
}

fn prefix_backfill_count<K: Eq>(committed: &[PlaneCard<K>], measured: &[MeasuredCard<K>]) -> usize {
    let count = measured
        .iter()
        .take_while(|item| committed.iter().all(|entry| entry.key != item.key))
        .count();
    let retains_creation_order = measured[count..]
        .iter()
        .filter(|item| committed.iter().any(|entry| entry.key == item.key))
        .map(|item| &item.key)
        .eq(committed.iter().map(|entry| &entry.key));
    if count > 0 && retains_creation_order {
        count
    } else {
        0
    }
}

fn apply_backfill<K: Clone>(entries: &mut Vec<PlaneCard<K>>, older: &[MeasuredCard<K>]) {
    let current = placed_cards(entries);
    let placed = mosaic::backfill(&current, &candidate_sets(older), TRACK_COUNT);
    let mut introduced = older
        .iter()
        .zip(placed.iter())
        .map(|(item, card)| plane_card(item, card))
        .collect::<Vec<_>>();
    for (entry, card) in entries.iter_mut().zip(&placed[older.len()..]) {
        entry.card = card.clone();
    }
    introduced.append(entries);
    *entries = introduced;
}

fn apply_append<K: Clone>(entries: &mut Vec<PlaneCard<K>>, appended: &[MeasuredCard<K>]) {
    let current = placed_cards(entries);
    let placed = mosaic::append(&current, &candidate_sets(appended), TRACK_COUNT);
    replace_cards(entries, &placed[..current.len()]);
    entries.extend(
        appended
            .iter()
            .zip(&placed[current.len()..])
            .map(|(item, card)| plane_card(item, card)),
    );
}

fn candidate_sets<K>(measured: &[MeasuredCard<K>]) -> Vec<Vec<Candidate>> {
    measured
        .iter()
        .map(|item| item.candidates.clone())
        .collect()
}

fn placed_cards<K>(entries: &[PlaneCard<K>]) -> Vec<Card> {
    entries.iter().map(|entry| entry.card.clone()).collect()
}

fn plane_card<K: Clone>(item: &MeasuredCard<K>, card: &Card) -> PlaneCard<K> {
    PlaneCard {
        key: item.key.clone(),
        latched: item.latched,
        content_revision: item.content_revision,
        boundary: item.boundary,
        card: card.clone(),
    }
}

fn commit<K>(mut plane: PersistentLanePlane<K>, transition: Transition) -> PersistentLanePlane<K> {
    freeze_completed_epochs(&mut plane.cards);
    assert_boundary_floors(&plane.cards);
    let (max_row, min_base) = mosaic::extent(&placed_cards(&plane.cards));
    plane.max_row = max_row;
    plane.min_base = min_base;
    plane.commits += 1;
    plane.transition = transition;
    plane
}

fn freeze_completed_epochs<K>(entries: &mut [PlaneCard<K>]) {
    let Some(boundary) = entries.iter().rposition(|entry| entry.boundary) else {
        return;
    };
    for entry in &mut entries[..=boundary] {
        entry.card.lifecycle = mosaic::Lifecycle::Frozen;
    }
}

fn assert_boundary_floors<K>(entries: &[PlaneCard<K>]) {
    for (index, boundary) in entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.boundary)
    {
        assert_eq!(
            (boundary.card.track, boundary.card.span),
            (0, TRACK_COUNT),
            "a Mosaic placement boundary occupies every track"
        );
        let floor = boundary.card.base;
        assert!(
            entries[..index]
                .iter()
                .all(|entry| entry.card.base + entry.card.rows <= floor),
            "cards before a Mosaic placement boundary remain below its floor"
        );
        let ceiling = boundary.card.base + boundary.card.rows;
        let epoch_end = entries[index + 1..]
            .iter()
            .position(|entry| entry.boundary)
            .map_or(entries.len(), |offset| index + 1 + offset);
        assert!(
            entries[index + 1..epoch_end]
                .iter()
                .all(|entry| entry.card.base >= ceiling),
            "cards after a Mosaic placement boundary cannot cross its floor"
        );
    }
}

fn assert_unique<K: Eq>(measured: &[MeasuredCard<K>]) {
    for (index, item) in measured.iter().enumerate() {
        assert!(
            measured[..index].iter().all(|other| other.key != item.key),
            "persistent Mosaic input contains a duplicate card identity"
        );
        assert!(
            !item.candidates.is_empty(),
            "persistent Mosaic card has no measurement candidate"
        );
    }
}

fn replace_cards<K>(entries: &mut [PlaneCard<K>], cards: &[Card]) {
    assert_eq!(entries.len(), cards.len());
    for (entry, card) in entries.iter_mut().zip(cards) {
        entry.card = card.clone();
    }
}

/// One independently packed lane plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanePlane {
    /// Cards placed in stream creation order.
    pub cards: Vec<Card>,
    /// Highest occupied lattice row.
    pub max_row: i32,
    /// Lowest occupied lattice base.
    pub min_base: i32,
}

impl LanePlane {
    /// Number of rows the CSS grid must expose, including a negative ripple.
    #[must_use]
    pub fn rows(&self) -> i32 {
        (self.max_row - self.min_base).max(1)
    }
}

/// Pack one lane without carrying a skyline in from any sibling lane.
#[must_use]
pub fn arrange(candidate_sets: &[Vec<Candidate>]) -> LanePlane {
    let cards = mosaic::pack(candidate_sets, TRACK_COUNT);
    let (max_row, min_base) = mosaic::extent(&cards);
    LanePlane {
        cards,
        max_row,
        min_base,
    }
}
