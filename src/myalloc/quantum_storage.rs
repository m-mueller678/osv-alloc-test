use crate::{
    quantum_address::QuantumAddress,
    util::{unsafe_assert, VIRTUAL_QUANTUM_SIZE},
    SystemInterface,
};
use buddy_bitmap::BuddyTower;
use log::{error, warn};
use rand::Rng;
use std::sync::{atomic::Ordering::Relaxed, Mutex};
use std::{ops::Range, sync::atomic::AtomicUsize};

pub struct QuantumStorage<S: SystemInterface> {
    quantum_base: AtomicUsize,
    available_quanta: BuddyTower<S::Alloc>,
    released_quanta: BuddyTower<S::Alloc>,
    transfer_buffer: Mutex<Vec<u32, S::Alloc>>,
    sys: S,
}

const QUANTUM_ID_BITS: u32 = 27;
const QUANTUM_ID_MASK: u32 = (1 << QUANTUM_ID_BITS) - 1;
const TRANSFER_BUFFER_LEVEL_BITS: u32 = 32 - QUANTUM_ID_BITS;

impl<S: SystemInterface> QuantumStorage<S> {
    pub fn alloc(&self, level: u32, rng: &mut impl Rng) -> Option<QuantumAddress> {
        for _ in 0..32 {
            if let Some(x) = self.available_quanta.remove(level, rng, 8 * 64 * 16) {
                let base = self.quantum_base.load(Relaxed);
                // if a quantum was found, the storage must have been initialised.
                debug_assert!(base != 0);
                let addr = x + base;
                unsafe_assert!(addr != 0);
                return Some(QuantumAddress::from_start(addr));
            }
            self.recycle();
        }
        error!("failed to reclaim sufficient virtual memory");
        None
    }

    fn recycle(&self) {
        if let Ok(mut tb) = self.transfer_buffer.try_lock() {
            let insert_transfer_vector = |transfer_buffer: &mut Vec<u32, S::Alloc>| {
                self.sys.global_tlb_flush();
                for &x in &*transfer_buffer {
                    let level = x >> QUANTUM_ID_BITS;
                    let quantum_id = x & QUANTUM_ID_MASK;
                    self.released_quanta.insert(quantum_id as usize, level)
                }
                transfer_buffer.clear();
            };
            assert!(tb.is_empty());
            let levels = self.released_quanta.levels();
            assert!(levels <= (1 << TRANSFER_BUFFER_LEVEL_BITS));
            for level in 0..levels {
                for quantum in self.released_quanta.drain_level(level) {
                    if tb.len() < tb.capacity() {
                        warn!("transfer vector full!");
                        insert_transfer_vector(&mut tb);
                    }
                    let transfer_encoded = ((level as u32) << QUANTUM_ID_BITS) | quantum as u32;
                    tb.push(transfer_encoded);
                }
            }
            self.sys.trace_recycle();
            insert_transfer_vector(&mut tb);
        } else {
            self.sys.trace_recycle_backoff();
            // recycling in progress, just wait for it to be done.
            drop(self.transfer_buffer.lock());
        }
    }

    pub fn dealloc_clean(&self, level: u32, quantum: QuantumAddress) {
        let index = (quantum.start() - self.quantum_base.load(Relaxed)) / VIRTUAL_QUANTUM_SIZE;
        debug_assert!(index < 1 << 31);
        self.available_quanta.insert(index, level)
    }

    pub fn dealloc_dirty(&self, level: u32, quantum: QuantumAddress) {
        let index = (quantum.start() - self.quantum_base.load(Relaxed)) / VIRTUAL_QUANTUM_SIZE;
        debug_assert!(index < 1 << 31);
        self.released_quanta.insert(index, level)
    }

    pub fn from_range(sys: S, range: Range<QuantumAddress>) -> Self {
        assert!(range.start.start().is_multiple_of(VIRTUAL_QUANTUM_SIZE));
        assert!(range.end.start().is_multiple_of(VIRTUAL_QUANTUM_SIZE));
        let quantum_count = (range.end.start() - range.start.start()) / VIRTUAL_QUANTUM_SIZE;
        QuantumStorage {
            quantum_base: AtomicUsize::new(range.start.start()),
            available_quanta: BuddyTower::new(quantum_count, sys.allocator()),
            released_quanta: BuddyTower::new(quantum_count, sys.allocator()),
            transfer_buffer: Mutex::new(Vec::with_capacity_in(quantum_count / 2, sys.allocator())),
            sys,
        }
    }
}
