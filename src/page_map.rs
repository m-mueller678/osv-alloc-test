use ahash::RandomState;
use radium::marker::{Atomic, BitOps, NumericOps};
use radium::{Atom, Radium};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{Shl, Shr};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Mutex;
use x86_64::structures::paging::{Page, PhysFrame, Size2MiB};
use x86_64::PhysAddr;

pub trait BetterAtom:
    Atomic
    + NumericOps
    + BitOps
    + Shl<u32, Output = Self>
    + Shr<u32, Output = Self>
    + From<u8>
    + Debug
    + Hash
{
}
impl<
        T: Atomic
            + NumericOps
            + BitOps
            + Shl<u32, Output = Self>
            + Shr<u32, Output = Self>
            + From<u8>
            + Debug
            + Hash,
    > BetterAtom for T
{
}

fn mask<T: BetterAtom>(bits: u32) -> T {
    (T::from(1) << bits) - T::from(1)
}

fn check_width(val: impl BetterAtom, bits: u32) {
    debug_assert!(val | mask(bits) == mask(bits));
}

pub struct SmallCountHashMap<T: BetterAtom, const C: u32, const V: u32, const K: u32> {
    slot_index_mask: usize,
    slots: Vec<Atom<T>>,
    random_state: RandomState,
    #[cfg(feature = "small_hash_map_debug")]
    lock: Mutex<BTreeMap<T, (T, T)>>,
}

impl<T: BetterAtom, const C: u32, const V: u32, const K: u32> SmallCountHashMap<T, C, V, K> {
    pub fn with_num_slots(mut s: usize) -> Self {
        assert!((C + V + K) as usize <= std::mem::size_of::<T>() * 8);
        s = s.next_power_of_two();
        SmallCountHashMap {
            slot_index_mask: s - 1,
            slots: (0..s).map(|_| Atom::new(T::from(0))).collect(),
            random_state: RandomState::with_seed(0xee61096f95490820),
            #[cfg(feature = "small_hash_map_debug")]
            lock: Default::default(),
        }
    }

    pub fn decrement(&self, k: T) -> Option<T> {
        #[cfg(feature = "small_hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "small_hash_map_debug")]
        let debug = &mut lock.get_mut(&k).unwrap();
        let mut i = self.target_slot(k);
        loop {
            let found = self.slots[i].load(Relaxed);
            if (found >> (K + V)) != T::from(0) && (found & mask(K)) == k {
                let old_val = self.slots[i].fetch_sub(T::from(1) << (K + V), Relaxed);
                let old_count = old_val >> (K + V);
                #[cfg(feature = "small_hash_map_debug")]
                assert_eq!(old_count, debug.1);
                let ret = if old_count == T::from(1) {
                    let v = (old_val >> K) & mask(V);
                    #[cfg(feature = "small_hash_map_debug")]
                    assert_eq!(v, debug.0);
                    Some(v)
                } else {
                    None
                };
                #[cfg(feature = "small_hash_map_debug")]
                {
                    debug.1 = debug.1 - T::from(1);
                    if debug.1 == T::from(0) {
                        lock.remove(&k);
                    }
                }
                return ret;
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    pub fn insert(&self, k: T, v: T, c: T) -> usize {
        #[cfg(feature = "small_hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "small_hash_map_debug")]
        {
            assert!(lock.insert(k, (v, c)).is_none());
        }
        check_width(k, K);
        check_width(v, V);
        check_width(c, C);
        let record = ((c << V) | v) << K | k;
        let mut i = self.target_slot(k);
        loop {
            let x = self.slots[i].load(Relaxed);
            if x >> (K + V) == T::from(0) {
                if self.slots[i]
                    .compare_exchange(x, record, Relaxed, Relaxed)
                    .is_ok()
                {
                    return i;
                } else {
                    continue;
                }
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    pub fn increment_at(&self, index: usize, k: T) {
        #[cfg(feature = "small_hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "small_hash_map_debug")]
        let old_debug_count = {
            let x = &mut lock.get_mut(&k).unwrap().1;
            *x = *x + T::from(1);
            *x - T::from(1)
        };
        let old = self.slots[index].fetch_add(T::from(1) << (K + V), Relaxed);
        #[cfg(feature = "small_hash_map_debug")]
        {
            assert_eq!(old_debug_count, old >> (K + V));
            assert_eq!(k, old & mask(K));
        }
    }

    fn target_slot(&self, k: T) -> usize {
        self.random_state.hash_one(k) as usize & self.slot_index_mask
    }
}

pub struct PageMap {
    pub base_page: Page<Size2MiB>,
    inner: SmallCountHashMap<u64, 16, 21, 27>,
}

impl PageMap {
    pub fn new(num_slots: usize, base_page: Page<Size2MiB>) -> Self {
        PageMap {
            base_page,
            inner: SmallCountHashMap::with_num_slots(num_slots),
        }
    }

    pub fn decrement(&self, page: Page<Size2MiB>) -> Option<PhysFrame<Size2MiB>> {
        self.inner
            .decrement(page - self.base_page)
            .map(|f| PhysFrame::containing_address(PhysAddr::new(f << 21)))
    }

    pub fn insert(&self, page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>, count: usize) -> usize {
        self.inner.insert(
            page - self.base_page,
            frame.start_address().as_u64() >> 21,
            count as u64,
        )
    }

    pub fn increment_at(&self, index: usize, page: Page<Size2MiB>) {
        self.inner.increment_at(index, page - self.base_page)
    }
}
