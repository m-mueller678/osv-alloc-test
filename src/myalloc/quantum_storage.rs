use std::ops::Range;
use rand::Rng;
use crate::buddymap::BuddyTower;
use crate::myalloc::VIRTUAL_QUANTUM_BITS;

#[derive(Default)]
pub struct QuantumStorage {
    available_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
    released_quanta: BuddyTower<{ 48 - VIRTUAL_QUANTUM_BITS as usize }>,
}

impl QuantumStorage {
    pub fn alloc(&self, level: u32, rng: &mut impl Rng) -> Option<u32> {
        if let Some(x)  = self.available_quanta.remove(level, rng){
            return Some(x);
        }
        self.available_quanta.steal_all_and_flush(&self.released_quanta);
        self.available_quanta.remove(level,rng)
    }

    pub fn dealloc_clean(&self, level: u32, q: u32) {
        debug_assert!(q<1u32<<31);
        self.available_quanta.insert(level, q)
    }
    pub fn dealloc_dirty(&self, level: u32, q: u32) {
        debug_assert!(q<1u32<<31);
        self.released_quanta.insert(level, q)
    }

    pub fn from_range(range: Range<u32>) -> Self {
        QuantumStorage {
            available_quanta: BuddyTower::from_range(range),
            released_quanta: Default::default(),
        }
    }
}