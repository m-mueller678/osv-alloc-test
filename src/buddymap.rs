use crate::page_map::mask;
use crate::system_interface::SystemInterface;

use itertools::Itertools;
use rand::distributions::Distribution;
use rand::distributions::Uniform;
use rand::Rng;
use std::alloc::Allocator;
use std::ops::Range;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{info, warn};

const QUANTUM_ID_BITS: u32 = 27;
const TRANSFER_BUFFER_LEVEL_BITS: u32 = 32 - QUANTUM_ID_BITS;

pub struct BuddyMap<A: Allocator + Default> {
    pairs: Vec<AtomicU64, A>,
    index_distribution: Uniform<usize>,
}

impl<A: Allocator + Default> BuddyMap<A> {
    pub fn new(slot_count: usize) -> Self {
        let len = (slot_count).div_ceil(64);
        let mut pairs = Vec::with_capacity_in(len, A::default());
        for _ in 0..len {
            pairs.push(AtomicU64::new(0));
        }
        BuddyMap {
            pairs,
            index_distribution: Uniform::new(0, len),
        }
    }
    pub fn insert(&self, quantum: u32) -> bool {
        let word = (quantum / 64) as usize;
        let bit = quantum % 64;
        let buddy_bit = bit ^ 1;
        let mut ret = false;
        self.pairs[word]
            .fetch_update(Relaxed, Relaxed, |x| {
                debug_assert!(x & (1 << bit) == 0);
                if (x >> buddy_bit) & 1 != 0 {
                    ret = true;
                    Some(x ^ (1 << buddy_bit))
                } else {
                    ret = false;
                    Some(x ^ 1 << bit)
                }
            })
            .ok();
        ret
    }

    pub fn remove(&self, rng: &mut impl Rng) -> Option<u32> {
        let mut i: usize = self.index_distribution.sample(rng);
        let mut bit = 0;
        for _ in 0..16 {
            let taken = self.pairs[i]
                .fetch_update(Relaxed, Relaxed, |x| {
                    if x == 0 {
                        None
                    } else {
                        bit = x.trailing_zeros();
                        Some(x ^ (1 << bit))
                    }
                })
                .is_ok();
            if taken {
                return Some(i as u32 * 64 + bit);
            } else {
                i += 1;
                if i == self.pairs.len() {
                    i = 0;
                }
            }
        }
        None
    }
}

pub struct BuddyTower<S: SystemInterface, const H: usize> {
    base_quantum: u32,
    maps: [BuddyMap<S::Alloc>; H],
}

impl<S: SystemInterface, const H: usize> BuddyTower<S, H> {
    pub fn new(quantum_count: usize, base_quantum: u32) -> Self {
        assert!(H < (1usize << TRANSFER_BUFFER_LEVEL_BITS));
        info!("quantum_count={quantum_count}");
        BuddyTower {
            base_quantum,
            maps: array_init::array_init(|i| BuddyMap::new(quantum_count.div_ceil(1 << i))),
        }
    }

    pub fn insert(&self, mut level: u32, first_quantum: u32) {
        let first_quantum = first_quantum - self.base_quantum;
        debug_assert!(first_quantum % (1 << level) == 0);
        while self.maps[level as usize].insert(first_quantum >> level) {
            //eprintln!("found buddy on level {level:2}");
            level += 1;
        }
        //eprintln!("inserted to level {level:2}");
    }

    pub fn remove(&self, level: u32, rng: &mut impl Rng) -> Option<u32> {
        //eprintln!("removing from level {level:2}");
        let mut taken_from = level;
        while (taken_from as usize) < self.maps.len() {
            if let Some(mut buddy_id) = self.maps[taken_from as usize].remove(rng) {
                //eprintln!("found {buddy_id:16b} on level {taken_from:2}");
                while taken_from > level {
                    taken_from -= 1;
                    buddy_id *= 2;
                    //eprintln!("insert excess {:16b} on level {taken_from:2}",buddy_id + 1);
                    let found_buddy = self.maps[taken_from as usize].insert(buddy_id + 1);
                    debug_assert!(!found_buddy);
                }
                return Some((buddy_id << taken_from) + self.base_quantum);
            } else {
                taken_from += 1;
            }
        }
        //eprintln!("none found");
        None
    }

    pub fn from_range(range: Range<u32>) -> Self {
        assert!((range.end - 1) < (1u32 << QUANTUM_ID_BITS));
        let ret = Self::new(range.len(), range.start);
        for x in range {
            ret.insert(0, x);
        }
        ret.print_counts();
        ret
    }

    pub fn print_counts(&self) {
        info!(
            "quantum counts: {:?}",
            self.maps
                .iter()
                .enumerate()
                .map(|(i, l)| (
                    i,
                    l.pairs
                        .iter()
                        .map(|x| x.load(Relaxed).count_ones())
                        .sum::<u32>()
                ))
                .format(",")
        );
    }

    pub fn steal_all_and_flush(&self, other: &Self, transfer_buffer: &mut Vec<u32, S::Alloc>) {
        debug_assert!(transfer_buffer.is_empty());
        for l in 0..H {
            for (i, x) in other.maps[l].pairs.iter().enumerate() {
                let mut taken = x.swap(0, Ordering::Relaxed);
                while taken != 0 {
                    if transfer_buffer.len() == transfer_buffer.capacity() {
                        // this should never happen with properly sized transfer vector
                        warn!("ran out of transfer buffer space");
                        self.insert_transfer_vector(transfer_buffer);
                    }
                    let bit = taken.trailing_zeros();
                    taken ^= 1 << bit;
                    let quantum_id = (i as u32 * 64 + bit) << l;
                    let transfer_encoded =
                        (l as u32) << QUANTUM_ID_BITS | (quantum_id + self.base_quantum);
                    transfer_buffer.push(transfer_encoded);
                }
            }
        }
        self.insert_transfer_vector(transfer_buffer);
    }

    fn insert_transfer_vector(&self, transfer_buffer: &mut Vec<u32, S::Alloc>) {
        S::global_tlb_flush();
        for x in &mut *transfer_buffer {
            self.insert(*x >> QUANTUM_ID_BITS, *x & mask::<u32>(QUANTUM_ID_BITS))
        }
        transfer_buffer.clear();
    }
}
