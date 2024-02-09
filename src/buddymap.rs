use rand::Rng;
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::mem::take;
use std::ops::Range;
use std::sync::Mutex;
use x86_64::structures::paging::mapper::MapperFlushAll;

#[derive(Default)]
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

impl<const H: usize> Default for BuddyTower<H> {
    fn default() -> Self {
        BuddyTower {
            maps: (0..H)
                .map(|_| BuddyMap::default())
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|_| ())
                .unwrap(),
        }
    }
}

impl<const H: usize> BuddyTower<H> {
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
        dbg!(&range);
        let ret = Self::default();
        for x in range {
            ret.insert(0, x);
        }
        ret
    }

    pub fn print_counts(&self) {
        for (i, l) in self.maps.iter().enumerate() {
            print!("{i:2}:{:4}, ", l.pairs.lock().unwrap().len())
        }
        println!();
    }

    pub fn steal_all_and_flush(&self, other: &Self) {
        let stolen: Vec<_> = other
            .maps
            .iter()
            .map(|x| take(&mut *x.pairs.lock().unwrap()))
            .collect();
        MapperFlushAll::new().flush_all();
        assert_eq!(stolen.len(), H);
        for (level, buddies) in stolen.into_iter().enumerate() {
            for b in buddies {
                self.insert(level as u32, (b.0 << 1 | b.1 as u32) << level)
            }
        }
    }
}
