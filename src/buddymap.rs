use rand::Rng;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub struct BuddyMap {
    pairs: Mutex<BTreeMap<u32, bool>>,
}

impl BuddyMap {
    pub fn insert(&self, buddy: u32) -> bool {
        match self.pairs.lock().unwrap().entry(buddy / 2) {
            Entry::Occupied(x) => {
                debug_assert!(*x.get() as u32 != buddy % 2);
                x.remove();
                true
            }
            Entry::Vacant(x) => {
                x.insert(buddy % 2 != 0);
                false
            }
        }
    }

    pub fn remove(&self, _rng: &mut impl Rng) -> Option<u32> {
        self.pairs
            .lock()
            .unwrap()
            .pop_first()
            .map(|x| x.0 << 1 | x.1 as u32)
    }
}

pub struct BuddyTower<const H: usize> {
    maps: [BuddyMap; H],
}

impl<const H: usize> BuddyTower<H> {
    pub fn insert(&self, mut level: u32, first_quantum: u32) {
        debug_assert!(first_quantum % (1 << level) == 0);
        while self.maps[level as usize].insert(first_quantum >> level) {
            level += 1;
        }
    }

    pub fn remove(&self, level: u32, rng: &mut impl Rng) -> Option<u32> {
        let mut taken_from = level;
        while (taken_from as usize) < self.maps.len() {
            if let Some(mut buddy_id) = self.maps[taken_from as usize].remove(rng) {
                while taken_from > level {
                    taken_from -= 1;
                    buddy_id *= 2;
                    self.maps[taken_from as usize].insert(buddy_id + 1);
                }
                return Some(buddy_id << taken_from);
            } else {
                taken_from += 1;
            }
        }
        None
    }
}
