use crate::buddymap::BuddyTower;
use crate::log_allocs::log_alloc;
use crate::myalloc::VIRTUAL_QUANTUM_BITS;
use crate::LogAllocMessage;
use rand::Rng;
use std::ops::Range;
use std::sync::Mutex;
use tracing::{error, info};

pub struct QuantumStorage {
    available_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    released_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    transfer_buffer: Mutex<Vec<u32>>,
}

impl QuantumStorage {
    pub fn alloc(&self, level: u32, rng: &mut impl Rng) -> Option<u32> {
        for _ in 0..32 {
            if let Some(x) = self.available_quanta.remove(level, rng) {
                return Some(x);
            }
            self.recycle(level);
        }
        error!("failed to reclaim sufficient virtual memory");
        None
    }

    fn recycle(&self, level: u32) {
        if let Ok(mut tb) = self.transfer_buffer.try_lock() {
            println!("recycling triggered: {level}");
            log_alloc(LogAllocMessage::Recycle as isize);
            self.available_quanta
                .steal_all_and_flush(&self.released_quanta, &mut tb);
        } else {
            println!("recycling back-off: {level}");
            log_alloc(LogAllocMessage::RecycleBackoff as isize);
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

    pub fn from_range(range: Range<u32>) -> Self {
        info!("quantum range: {:?}", &range);
        QuantumStorage {
            available_quanta: BuddyTower::from_range(range.clone()),
            released_quanta: BuddyTower::new(range.len(), range.start),
            transfer_buffer: Mutex::new(Vec::with_capacity(range.len() / 2)),
        }
    }
}
