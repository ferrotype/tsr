#![forbid(unsafe_code)]
use crate::{lock, Error};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventKind(pub i32);
#[allow(non_upper_case_globals)]
impl EventKind {
    pub const EventUpdate: Self = Self(1);
    pub const EventDelete: Self = Self(2);
}
impl std::fmt::Display for EventKind {
    // port: tsc/internal/fswatch/event.go:EventKind.String
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match *self {
            Self::EventUpdate => "update",
            Self::EventDelete => "delete",
            _ => "unknown",
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: EventKind,
    pub path: Vec<u8>,
}

// Kept outside the public event so existing callback consumers can construct it.
#[derive(Clone)]
pub(crate) struct PendingEvent {
    pub event: Event,
    pub included_watch_root: bool,
}
#[derive(Default)]
struct Entry {
    created: u64,
    updated: u64,
    deleted: u64,
    included_watch_root: bool,
}
impl Entry {
    // port: tsc/internal/fswatch/event.go:eventEntry.kindSince
    fn kind_since(&self, start: u64) -> Option<EventKind> {
        if self.deleted > start {
            if self.created > start && self.created < self.deleted && self.updated < self.deleted {
                return None;
            }
            return Some(EventKind::EventDelete);
        }
        (self.created.max(self.updated) > start).then_some(EventKind::EventUpdate)
    }
}
#[derive(Default)]
struct State {
    entries: HashMap<Vec<u8>, Entry>,
    error: Option<Error>,
    sequence: u64,
}
#[derive(Default)]
pub(crate) struct EventList(Mutex<State>);
impl EventList {
    fn record(&self, path: &[u8], sequence: Option<u64>, operation: u8, root: bool) -> u64 {
        let mut state = lock(&self.0);
        let sequence = match sequence {
            Some(sequence) => {
                state.sequence = state.sequence.max(sequence);
                sequence
            }
            None => {
                state.sequence = state.sequence.wrapping_add(1);
                state.sequence
            }
        };
        let entry = state.entries.entry(path.to_vec()).or_default();
        match operation {
            0 if entry.deleted > entry.created && entry.deleted > entry.updated => {
                entry.deleted = 0;
                entry.created = 0;
                entry.updated = sequence;
            }
            0 => entry.created = sequence,
            1 => entry.updated = sequence,
            _ => entry.deleted = sequence,
        }
        entry.included_watch_root |= root;
        sequence
    }
    #[cfg(any(target_os = "linux", test))]
    // port: tsc/internal/fswatch/event.go:eventList.create
    pub(crate) fn create(&self, p: &[u8]) {
        self.record(p, None, 0, false);
    }
    #[cfg(any(target_os = "linux", test))]
    // port: tsc/internal/fswatch/event.go:eventList.update
    pub(crate) fn update(&self, p: &[u8]) {
        self.record(p, None, 1, false);
    }
    #[cfg(any(target_os = "linux", test))]
    // port: tsc/internal/fswatch/event.go:eventList.remove
    pub(crate) fn remove(&self, p: &[u8]) {
        self.record(p, None, 2, false);
    }
    #[cfg(test)]
    // port: tsc/internal/fswatch/event.go:eventList.removeAndGetSequence
    pub(crate) fn remove_and_get_sequence(&self, p: &[u8]) -> u64 {
        self.record(p, None, 2, false)
    }
    #[cfg(test)]
    // port: tsc/internal/fswatch/event.go:eventList.createAt
    pub(crate) fn create_at(&self, p: &[u8], s: u64) {
        self.record(p, Some(s), 0, false);
    }
    #[cfg(any(target_os = "macos", test))]
    // port: tsc/internal/fswatch/event.go:eventList.updateAt
    pub(crate) fn update_at(&self, p: &[u8], s: u64) {
        self.record(p, Some(s), 1, false);
    }
    #[cfg(any(target_os = "macos", test))]
    // port: tsc/internal/fswatch/event.go:eventList.removeAt
    pub(crate) fn remove_at(&self, p: &[u8], s: u64) {
        self.record(p, Some(s), 2, false);
    }
    #[cfg(any(target_os = "macos", test))]
    // port: tsc/internal/fswatch/event.go:eventList.updateWatchRootAt
    pub(crate) fn update_watch_root_at(&self, p: &[u8], s: u64) {
        self.record(p, Some(s), 1, true);
    }
    #[cfg(any(target_os = "macos", test))]
    // port: tsc/internal/fswatch/event.go:eventList.removeWatchRootAt
    pub(crate) fn remove_watch_root_at(&self, p: &[u8], s: u64) {
        self.record(p, Some(s), 2, true);
    }
    // port: tsc/internal/fswatch/event.go:eventList.sequence
    pub(crate) fn sequence(&self) -> u64 {
        lock(&self.0).sequence
    }
    pub(crate) fn has_pending(&self) -> bool {
        let s = lock(&self.0);
        !s.entries.is_empty() || s.error.is_some()
    }
    // port: tsc/internal/fswatch/event.go:eventList.setError
    pub(crate) fn set_error(&self, error: Error) {
        lock(&self.0).error.get_or_insert(error);
    }
    // port: tsc/internal/fswatch/event.go:eventList.drainForSequences
    pub(crate) fn drain_for_sequences(
        &self,
        starts: &[u64],
    ) -> (Vec<Vec<PendingEvent>>, Option<Error>) {
        let mut state = lock(&self.0);
        let output = starts
            .iter()
            .map(|start| {
                state
                    .entries
                    .iter()
                    .filter_map(|(path, entry)| {
                        entry.kind_since(*start).map(|kind| PendingEvent {
                            event: Event {
                                kind,
                                path: path.clone(),
                            },
                            included_watch_root: entry.included_watch_root,
                        })
                    })
                    .collect()
            })
            .collect();
        state.entries.clear();
        (output, state.error.take())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn drain(events: &EventList) -> (Vec<Event>, Option<Error>) {
        let (mut batches, error) = events.drain_for_sequences(&[0]);
        let mut events: Vec<_> = batches
            .remove(0)
            .into_iter()
            .map(|pending| pending.event)
            .collect();
        events.sort_by(|a, b| a.path.cmp(&b.path));
        (events, error)
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListCreateThenDelete
    fn create_then_delete_cancels_the_event_but_keeps_pending_entry() {
        let e = EventList::default();
        e.create(b"a");
        e.remove(b"a");
        assert_eq!(lock(&e.0).entries.len(), 1);
        assert!(drain(&e).0.is_empty());
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListDeleteThenCreate
    fn delete_then_create_is_an_update() {
        let e = EventList::default();
        e.remove(b"a");
        e.create(b"a");
        assert_eq!(
            drain(&e).0,
            [Event {
                kind: EventKind::EventUpdate,
                path: b"a".to_vec()
            }]
        );
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListCreateDeleteCreate
    fn create_delete_create_is_an_update() {
        let e = EventList::default();
        e.create(b"a");
        e.remove(b"a");
        e.create(b"a");
        assert_eq!(
            drain(&e).0,
            [Event {
                kind: EventKind::EventUpdate,
                path: b"a".to_vec()
            }]
        );
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListErrorIsLatchedAndCleared
    fn first_error_is_latched_and_cleared_by_the_production_drain() {
        let e = EventList::default();
        assert!(lock(&e.0).error.is_none());
        assert!(!e.has_pending());
        e.set_error(Error::Message("first".into()));
        e.set_error(Error::Message("second".into()));
        assert!(e.has_pending());
        assert_eq!(lock(&e.0).error, Some(Error::Message("first".into())));
        assert_eq!(drain(&e).1, Some(Error::Message("first".into())));
        assert!(!e.has_pending());
        assert!(lock(&e.0).error.is_none());
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListDrainIsAtomic
    fn drain_takes_events_and_error_together_then_clears_both() {
        let e = EventList::default();
        e.create(b"a");
        e.update(b"b");
        e.set_error(Error::Message("oops".into()));
        let (events, error) = drain(&e);
        assert_eq!(
            events,
            [
                Event {
                    kind: EventKind::EventUpdate,
                    path: b"a".to_vec()
                },
                Event {
                    kind: EventKind::EventUpdate,
                    path: b"b".to_vec()
                }
            ]
        );
        assert_eq!(error, Some(Error::Message("oops".into())));
        assert_eq!(drain(&e), (Vec::new(), None));
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListDrainReturnsErrorWithEvents
    fn drain_returns_error_alongside_events() {
        let e = EventList::default();
        e.create(b"file.txt");
        e.set_error(Error::Overflow);
        let (events, error) = drain(&e);
        assert_eq!(
            events,
            [Event {
                kind: EventKind::EventUpdate,
                path: b"file.txt".to_vec()
            }]
        );
        assert_eq!(error, Some(Error::Overflow));
    }
    #[test]
    // source: tsc/internal/fswatch/eventlist_test.go:TestEventListDrainForSequences
    fn later_subscription_sees_deletion_of_an_earlier_pending_create() {
        let e = EventList::default();
        e.create(b"file.txt");
        let sequence = e.sequence();
        e.remove(b"file.txt");
        let (batches, error) = e.drain_for_sequences(&[0, sequence]);
        assert!(error.is_none());
        assert!(batches[0].is_empty());
        assert_eq!(batches[1].len(), 1);
        assert_eq!(
            batches[1][0].event,
            Event {
                kind: EventKind::EventDelete,
                path: b"file.txt".to_vec()
            }
        );
    }
    #[test]
    fn implicit_sequence_wraps_but_explicit_sequence_only_advances() {
        let events = EventList::default();
        events.create_at(b"last", u64::MAX);
        events.create(b"wrapped");
        assert_eq!(events.sequence(), 0);
        events.drain_for_sequences(&[]);
        events.update_at(b"next", 7);
        events.update_at(b"older", 3);
        assert_eq!(events.sequence(), 7);
        events.remove_at(b"next", 9);
        assert_eq!(events.sequence(), 9);
        let (batches, _) = events.drain_for_sequences(&[7]);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].event.kind, EventKind::EventDelete);
        assert_eq!(batches[0][0].event.path, b"next");
    }
    #[test]
    fn explicit_sequences_advance_cutoff_without_replaying_older_events() {
        let events = EventList::default();
        events.create_at(b"old", 10);
        events.update_at(b"new", 20);
        events.create_at(b"older", 5);
        assert_eq!(events.sequence(), 20);
        let (batches, _) = events.drain_for_sequences(&[10, 20]);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[0][0].event.path, b"new");
        assert!(batches[1].is_empty());
        events.create(b"next");
        assert_eq!(events.sequence(), 21);
    }
}
