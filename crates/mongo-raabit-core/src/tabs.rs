//! Bookkeeping of tab strips and per-item state keyed by position, without the UI.

/// The active tab after tab `closed` was removed from a strip.
pub fn active_after_close(active: usize, closed: usize) -> usize {
    if active >= closed && active > 0 {
        active - 1
    } else {
        active
    }
}

/// The active tab once the strip holds `len` tabs (the last one when it shrank past it).
pub fn clamp_active(active: usize, len: usize) -> usize {
    active.min(len.saturating_sub(1))
}

/// Where the item that was at `index` is after an item was inserted at `at`.
pub fn after_insert(index: usize, at: usize) -> usize {
    if index >= at { index + 1 } else { index }
}

/// Where the item that was at `index` is after the item at `removed` was removed
/// (`None` for that item itself).
pub fn after_remove(index: usize, removed: usize) -> Option<usize> {
    match index.cmp(&removed) {
        std::cmp::Ordering::Less => Some(index),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(index - 1),
    }
}

/// Where the item that was at `index` is after the item at `from` moved to `to`.
pub fn after_move(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && index > from && index <= to {
        index - 1
    } else if to < from && index >= to && index < from {
        index + 1
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_a_tab_keeps_the_selection_on_its_neighbour() {
        assert_eq!(active_after_close(2, 2), 1);
        assert_eq!(active_after_close(3, 1), 2);
        assert_eq!(active_after_close(0, 0), 0);
        assert_eq!(active_after_close(1, 2), 1);
    }

    #[test]
    fn the_active_tab_stays_in_range() {
        assert_eq!(clamp_active(5, 3), 2);
        assert_eq!(clamp_active(1, 3), 1);
        assert_eq!(clamp_active(0, 0), 0);
    }

    #[test]
    fn positions_follow_inserts_removals_and_moves() {
        assert_eq!([0, 1, 2].map(|i| after_insert(i, 1)), [0, 2, 3]);
        assert_eq!(
            [0, 1, 2, 3].map(|i| after_remove(i, 1)),
            [Some(0), None, Some(1), Some(2)]
        );
        // 0 1 2 3 with 0 moved to 2 becomes 1 2 0 3.
        assert_eq!([0, 1, 2, 3].map(|i| after_move(i, 0, 2)), [2, 0, 1, 3]);
        // ... and with 3 moved to 1 becomes 0 3 1 2.
        assert_eq!([0, 1, 2, 3].map(|i| after_move(i, 3, 1)), [0, 2, 3, 1]);
    }
}
