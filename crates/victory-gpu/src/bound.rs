//! The pipelines a host list has kept, by address.
//!
//! Adapted from Marvisak/VIctory#3167 (`avatarmanz/VIctory-7900XTX`, after
//! Belobug `92d353f8`, GPL-3.0-or-later).
//!
//! A list keeps a pipeline and its root signature so the title may release
//! them before it submits the list. A pipeline bound again in the same list,
//! with no reset between, is already kept: keeping it again is two queries
//! for an interface, three locks and a name looked up for nothing.

use std::collections::HashSet;

/// Host pipelines a list has kept since it was last reset (their addresses).
#[derive(Default)]
pub(crate) struct Bound(HashSet<usize>);

impl Bound {
    /// Whether `address` is a pipeline the list has not kept yet (and
    /// records it as kept). The first bind of a pipeline is its first keep.
    pub(crate) fn first(&mut self, address: usize) -> bool {
        self.0.insert(address)
    }
}

#[cfg(test)]
mod tests {
    use super::Bound;

    #[test]
    fn a_pipeline_bound_again_is_kept_once() {
        let mut bound = Bound::default();
        let mut kept = 0;
        for i in 0..10_000 {
            if bound.first(0x1000 + 8 * (i % 5)) {
                kept += 1;
            }
        }
        assert_eq!(kept, 5);
    }

    #[test]
    fn a_list_reset_binds_them_again() {
        let mut bound = Bound::default();
        assert!(bound.first(0x1000));
        assert!(!bound.first(0x1000));
        bound = Bound::default();
        assert!(bound.first(0x1000));
    }

    #[test]
    fn pipelines_an_address_apart_are_two() {
        let mut bound = Bound::default();
        assert!(bound.first(0x1000));
        assert!(bound.first(0x1008));
        assert!(!bound.first(0x1000));
    }
}
