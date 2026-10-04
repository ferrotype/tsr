use super::{Error, Session};
use crate::{
    background::Queue,
    clock::{Clock, Timer},
};
use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tsr_core::CancellationToken;

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Update,
    IdleClean,
}
impl Kind {
    fn index(self) -> usize {
        match self {
            Self::Update => 0,
            Self::IdleClean => 1,
        }
    }
}
#[derive(Default)]
struct Slot {
    epoch: u64,
    timer: Option<Timer>,
}
pub(super) struct Timers {
    session: Weak<Session>,
    clock: Arc<dyn Clock>,
    slots: Mutex<[Slot; 2]>,
    queue: Queue,
    token: CancellationToken,
    errors: Mutex<Vec<Error>>,
}
impl Timers {
    pub(super) fn new(session: Weak<Session>, clock: Arc<dyn Clock>) -> Self {
        Self {
            session,
            clock,
            slots: Mutex::default(),
            queue: Queue::new(),
            token: CancellationToken::new(),
            errors: Mutex::default(),
        }
    }
    pub(super) fn cancel(&self, kind: Kind) {
        let previous = {
            let mut slots = self.slots.lock().unwrap();
            let slot = &mut slots[kind.index()];
            slot.epoch = slot
                .epoch
                .checked_add(1)
                .expect("session timer epoch exhausted");
            slot.timer.take()
        };
        drop(previous);
    }
    pub(super) fn matches(&self, kind: Kind, epoch: u64) -> bool {
        !self.token.is_canceled() && self.slots.lock().unwrap()[kind.index()].epoch == epoch
    }
    pub(super) fn schedule(&self, kind: Kind, delay: Duration) {
        if self.token.is_canceled() {
            return;
        }
        let (epoch, previous) = {
            let mut slots = self.slots.lock().unwrap();
            let slot = &mut slots[kind.index()];
            slot.epoch = slot
                .epoch
                .checked_add(1)
                .expect("session timer epoch exhausted");
            (slot.epoch, slot.timer.take())
        };
        drop(previous);
        let weak = self.session.clone();
        let timer = self.clock.after(
            delay,
            Box::new(move || {
                let Some(session) = weak.upgrade() else {
                    return;
                };
                if !session.timers.matches(kind, epoch) {
                    return;
                }
                let weak = Arc::downgrade(&session);
                session
                    .timers
                    .queue
                    .enqueue(session.timers.token.clone(), move |_| {
                        if let Some(session) = weak.upgrade() {
                            if let Err(error) = session.flush_inner(
                                None,
                                matches!(kind, Kind::IdleClean),
                                Some((kind, epoch)),
                            ) {
                                if !matches!(error, Error::Closed) {
                                    session.timers.errors.lock().unwrap().push(error);
                                }
                            }
                        }
                    });
            }),
        );
        let previous = {
            let mut slots = self.slots.lock().unwrap();
            let slot = &mut slots[kind.index()];
            if slot.epoch == epoch && !self.token.is_canceled() {
                slot.timer.replace(timer)
            } else {
                Some(timer)
            }
        };
        drop(previous);
    }
    pub(super) fn wait(&self) {
        self.queue.wait();
    }
    pub(super) fn take_errors(&self) -> Vec<Error> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }
    pub(super) fn stop(&self) {
        self.token.cancel();
        self.cancel(Kind::Update);
        self.cancel(Kind::IdleClean);
        self.queue.close();
    }
}
