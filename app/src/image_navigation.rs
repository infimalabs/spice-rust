//! Stable movement through lane media rendered in Mosaic card order.
//!
//! The plane's cards are newest first. Images within one card are oldest first,
//! including crystallized runs, so crossing a card boundary reverses the card
//! index step while retaining the image index direction inside a card.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Older,
    Newer,
}

/// The adjacent media position among newest-first cards.
///
/// `media_count` returns zero for a card outside the current lane, or for a card
/// with no eligible image. This keeps lane filtering at the browser boundary
/// without allocating a second index of the rendered plane.
pub(crate) fn adjacent_position(
    card_count: usize,
    current_card: usize,
    current_image: usize,
    direction: Direction,
    mut media_count: impl FnMut(usize) -> usize,
) -> Option<(usize, usize)> {
    if current_card >= card_count {
        return None;
    }
    let images = media_count(current_card);
    if current_image >= images {
        return None;
    }
    match direction {
        Direction::Newer if current_image + 1 < images => Some((current_card, current_image + 1)),
        Direction::Older if current_image > 0 => Some((current_card, current_image - 1)),
        Direction::Newer => (0..current_card)
            .rev()
            .find_map(|card| (media_count(card) > 0).then_some((card, 0))),
        Direction::Older => ((current_card + 1)..card_count).find_map(|card| {
            let images = media_count(card);
            (images > 0).then_some((card, images.saturating_sub(1)))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, adjacent_position};

    #[test]
    fn lane_media_moves_in_time_across_condensed_cards_and_skips_other_lanes() {
        // Cards are newest first; each card's images are oldest first. A zero is
        // another lane's card, which cannot become a navigation stop.
        let cards = [2, 0, 3];
        let adjacent = |card, image, direction| {
            adjacent_position(cards.len(), card, image, direction, |card| cards[card])
        };

        assert_eq!(adjacent(2, 0, Direction::Newer), Some((2, 1)));
        assert_eq!(adjacent(2, 2, Direction::Newer), Some((0, 0)));
        assert_eq!(adjacent(0, 0, Direction::Older), Some((2, 2)));
        assert_eq!(adjacent(2, 1, Direction::Older), Some((2, 0)));
    }

    #[test]
    fn lane_media_holds_at_both_ends() {
        let cards = [2, 0, 3];
        let adjacent = |card, image, direction| {
            adjacent_position(cards.len(), card, image, direction, |card| cards[card])
        };

        assert_eq!(adjacent(0, 1, Direction::Newer), None);
        assert_eq!(adjacent(2, 0, Direction::Older), None);
    }
}
