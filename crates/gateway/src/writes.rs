//! Serializes writes to the same tag.
//!
//! `TagStore::write_mode` reads the current value before it writes (pulse
//! restores it, toggle/increase/decrease compute from it). Two such writes to
//! one tag running at once would read each other's intermediate value: two
//! overlapping pulses leave the tag stuck at the pulse value (a reset coil held
//! high). Every write therefore holds its tag's lock for its whole duration.
//! A pulse that finds its tag busy is refused at once instead of queueing, so a
//! double-clicked Reset button sends one pulse, not two in a row.

use moviclone_tagdb::TagId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Default)]
pub(crate) struct WriteLocks {
    // Bounded by the number of tags; entries are never removed.
    map: Mutex<HashMap<TagId, Arc<AsyncMutex<()>>>>,
}

impl WriteLocks {
    fn entry(&self, id: TagId) -> Arc<AsyncMutex<()>> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id).or_default().clone()
    }

    /// Waits until no other write to `id` runs.
    pub(crate) async fn lock(&self, id: TagId) -> OwnedMutexGuard<()> {
        self.entry(id).lock_owned().await
    }

    /// `None` when another write to `id` is still running.
    pub(crate) fn try_lock(&self, id: TagId) -> Option<OwnedMutexGuard<()>> {
        self.entry(id).try_lock_owned().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn same_tag_is_exclusive_other_tags_are_not() {
        let locks = WriteLocks::default();
        let (a, b): (TagId, TagId) = (0, 1);
        let held = locks.lock(a).await;
        assert!(locks.try_lock(a).is_none());
        assert!(locks.try_lock(b).is_some());
        drop(held);
        assert!(locks.try_lock(a).is_some());
    }
}
