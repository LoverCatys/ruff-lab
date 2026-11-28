//! Refresh sets (`VICTORY_REFRESH_SET=1`, on by default; `=0` disables):
//! replaces pushing one `Step::Refresh` per by-address copy onto every command
//! list with a single shared `Step::RefreshSet(Arc<CopySet>)`.
//!
//! Ported from `Kemerd/VIctory-NVIDIA-RT` (`d06b7ab`). Because a submission
//! walks every list's steps in `ask_once`, `stale_under_flights`, and
//! `run_deferred`, deduplicating an immutable `CopySet` pointer per submission
//! eliminates tens of thousands of redundant step visits per frame.

use std::sync::{Arc, OnceLock};

use crate::shadow::Shadow;

/// One title-memory copy inside a [`CopySet`].
pub(crate) struct Member {
    pub(crate) shadow: Arc<Shadow>,
    pub(crate) at: usize,
    pub(crate) len: usize,
}

/// An immutable snapshot of all copies `memory::ready` brings up to date,
/// shared by every command list while the number of numbered copies is unchanged.
pub(crate) struct CopySet {
    copies: usize,
    members: Vec<Member>,
}

impl CopySet {
    pub(crate) fn new(copies: usize, members: Vec<Member>) -> Self {
        Self { copies, members }
    }

    pub(crate) fn copies(&self) -> usize {
        self.copies
    }

    pub(crate) fn members(&self) -> &[Member] {
        &self.members
    }
}

/// Whether `memory::ready` records one `Step::RefreshSet` per list instead of
/// one `Step::Refresh` per copy (`VICTORY_REFRESH_SET=0` disables).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_REFRESH_SET").is_none_or(|v| v != "0"))
}
