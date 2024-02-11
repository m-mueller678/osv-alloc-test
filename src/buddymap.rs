use crate::page_map::RhHash;
use crate::util::mask;
use next_gen::mk_gen;
use rand::Rng;
use std::ops::Range;
use x86_64::structures::paging::mapper::MapperFlushAll;

const QUANTUM_ID_BITS: u32 = 27;
const TRANSFER_BUFFER_LEVEL_BITS: u32 = 32 - QUANTUM_ID_BITS;
const PAIR_KEY_BITS: u32 = QUANTUM_ID_BITS - 1;
pub struct BuddyMap {
    pairs: RhHash<u32, PAIR_KEY_BITS>,
}
const BUDDY_MASK: u32 = 3u32 << PAIR_KEY_BITS;

impl BuddyMap {
    pub fn new(slot_count: usize) -> Self {
        BuddyMap {
            // TODO rhHashmap is prone to deadlocks if there are too few slots per thread.
            pairs: RhHash::new(slot_count.max(1 << 10)),
        }
    }
    pub fn insert(&self, buddy: u32) -> bool {
        self.pairs.update(buddy / 2, |x| {
            if BUDDY_MASK & x == 0 {
                (false, x | 1 << (PAIR_KEY_BITS + buddy % 2))
            } else {
                (true, x & !BUDDY_MASK)
            }
        })
    }

    pub fn remove(&self, rng: &mut impl Rng) -> Option<u32> {
        let x = self.pairs.remove_any(rng, 128);
        if x == 0 {
            return None;
        }
        let is_high = (x >> (1 + PAIR_KEY_BITS)) & 1;
        let key = x & mask::<u32>(PAIR_KEY_BITS);
        let buddy_id = key << 1 | is_high;
        Some(buddy_id)
    }
}

pub struct BuddyTower<const H: usize> {
    maps: [BuddyMap; H],
}

impl<const H: usize> BuddyTower<H> {
    pub fn new(quantum_count: usize) -> Self {
        assert!(H < (1usize << TRANSFER_BUFFER_LEVEL_BITS));
        dbg!(quantum_count);
        let l0_slots = (quantum_count / 2).next_power_of_two();
        BuddyTower {
            maps: array_init::array_init(|i| BuddyMap::new(l0_slots >> i)),
        }
    }

    pub fn insert(&self, mut level: u32, first_quantum: u32) {
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
                return Some(buddy_id << taken_from);
            } else {
                taken_from += 1;
            }
        }
        //eprintln!("none found");
        None
    }

    pub fn from_range(range: Range<u32>) -> Self {
        assert!((range.end - 1) < (1u32 << QUANTUM_ID_BITS));
        let ret = Self::new(range.len());
        for x in range {
            ret.insert(0, x);
        }
        ret.print_counts();
        ret
    }

    pub fn print_counts(&self) {
        for (i, l) in self.maps.iter().enumerate() {
            print!("{i:2}:{:4}, ", l.pairs.count())
        }
        println!();
    }

    pub fn steal_all_and_flush(&self, other: &Self, transfer_buffer: &mut Vec<u32>) {
        debug_assert!(transfer_buffer.is_empty());
        for l in 0..H {
            let gen_fn = RhHash::drain;
            mk_gen!(let gen=gen_fn(&other.maps[l].pairs));
            for x in gen {
                if transfer_buffer.len() == transfer_buffer.capacity() {
                    // this should never happen with properly sized transfer vector
                    eprintln!("ran out of transfer buffer space");
                    self.insert_transfer_vector(transfer_buffer);
                }
                let is_high = (x >> (1 + PAIR_KEY_BITS)) & 1;
                let key = x & mask::<u32>(PAIR_KEY_BITS);
                let quantum_id = (key << 1 | is_high) << l;
                let transfer_encoded = (l as u32) << QUANTUM_ID_BITS | quantum_id;
                transfer_buffer.push(transfer_encoded);
            }
        }
        self.insert_transfer_vector(transfer_buffer);
    }

    fn insert_transfer_vector(&self, transfer_buffer: &mut Vec<u32>) {
        MapperFlushAll::new().flush_all();
        for x in &mut *transfer_buffer {
            self.insert(*x >> QUANTUM_ID_BITS, *x & mask::<u32>(QUANTUM_ID_BITS))
        }
        transfer_buffer.clear()
    }
}
