use super::{Session, Snapshot};
use std::sync::Arc;

impl Session {
    /// Retain the candidate until the background queue either adopts or discards
    /// it. A request keeps its own snapshot even if the session has moved on.
    // port: tsc/internal/project/api.go:Session.TryAdoptSnapshotInBackground
    pub fn try_adopt_snapshot_in_background(
        self: &Arc<Self>,
        base: &Snapshot,
        candidate: Snapshot,
    ) -> bool {
        let base = base.clone();
        let session = Arc::downgrade(self);
        self.enqueue_background(move |_| {
            if let Some(session) = session.upgrade() {
                session.adopt_snapshot(&base, candidate);
            }
        })
    }

    // port: tsc/internal/project/session.go:Session.adoptSnapshotChange
    fn adopt_snapshot(&self, base: &Snapshot, candidate: Snapshot) {
        let mut current = self.snapshot.write().expect("session snapshot");
        if current
            .as_ref()
            .is_some_and(|current| current.id() == base.id())
        {
            *current = Some(candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::ApiSnapshotRequest, session::SessionOptions};
    #[test]
    fn background_adoption_never_overwrites_a_newer_snapshot_or_reopens_a_closed_session() {
        let fs = Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish());
        let session = Session::new(SessionOptions::default(), fs, &tsr_arena::Counters::new());
        let base = session.snapshot().unwrap();
        let newer = session
            .api_update(
                crate::file_change::FileChangeSummary::default(),
                ApiSnapshotRequest::default(),
            )
            .unwrap()
            .snapshot;
        assert_ne!(base.id(), newer.id());
        assert!(session.try_adopt_snapshot_in_background(&base, base.clone()));
        session.wait_for_background_tasks();
        assert_eq!(session.snapshot().unwrap().id(), newer.id());
        session.close();
        assert!(!session.try_adopt_snapshot_in_background(&newer, base));
        assert!(session.snapshot().is_err());
    }
}
