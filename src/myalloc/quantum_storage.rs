use crate::myalloc::VIRTUAL_QUANTUM_BITS;
use crate::{buddymap::BuddyTower, SystemInterface};
use rand::Rng;
use std::ops::Range;
use std::sync::Mutex;
use tracing::{error, info};

pub struct QuantumStorage<S: SystemInterface> {
    available_quanta: BuddyTower<S, { 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    released_quanta: BuddyTower<S, { 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    transfer_buffer: Mutex<Vec<u32, S::Alloc>>,
    sys: S,
}

impl<S: SystemInterface> QuantumStorage<S> {
    pub fn alloc(&self, level: u32, rng: &mut impl Rng) -> Option<u32> {
        for _ in 0..32 {
            if let Some(x) = self.available_quanta.remove(level, rng) {
                return Some(x);
            }
            self.recycle();
        }
        error!("failed to reclaim sufficient virtual memory");
        None
    }

    fn recycle(&self) {
        if let Ok(mut tb) = self.transfer_buffer.try_lock() {
            self.sys.trace_recycle();
            self.available_quanta
                .steal_all_and_flush(&self.released_quanta, &mut tb);
        } else {
            self.sys.trace_recycle_backoff();
            // recycling in progress, just wait for it to be done.
            drop(self.transfer_buffer.lock());
        }
    }

    pub fn dealloc_clean(&self, level: u32, q: u32) {
        debug_assert!(q < 1u32 << 31);
        self.available_quanta.insert(level, q)
    }
    pub fn dealloc_dirty(&self, level: u32, q: u32) {
        debug_assert!(q < 1u32 << 31);
        self.released_quanta.insert(level, q)
    }

    pub fn from_range(sys: S, range: Range<u32>) -> Self {
        info!("quantum range: {:?}", &range);
        QuantumStorage {
            available_quanta: BuddyTower::from_range(sys, range.clone()),
            released_quanta: BuddyTower::new(sys, range.len(), range.start),
            transfer_buffer: Mutex::new(Vec::with_capacity_in(range.len() / 2, sys.allocator())),
            sys,
        }
    }
}
