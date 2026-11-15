use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock {
    fn now_ms(&self) -> u64;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ManualClock(Rc<Cell<u64>>);

impl ManualClock {
    pub fn new(start_ms: u64) -> Self {
        Self(Rc::new(Cell::new(start_ms)))
    }

    pub fn advance(&self, ms: u64) {
        self.0.set(self.0.get() + ms);
    }

    pub fn set(&self, ms: u64) {
        self.0.set(ms);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}
