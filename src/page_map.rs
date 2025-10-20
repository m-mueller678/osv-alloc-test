use ahash::RandomState;
use radium::marker::{Atomic, BitOps, NumericOps};
use radium::{Atom, Radium};
use std::alloc::Allocator;
#[cfg(feature = "hash_map_debug")]
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::ops::{Shl, Shr};
use std::sync::atomic::Ordering::Relaxed;
#[cfg(feature = "hash_map_debug")]
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
    + TryFrom<u64>
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
            + Hash
            + TryFrom<u64>,
    > BetterAtom for T
{
}

fn check_width(val: impl BetterAtom, bits: u32) {
    debug_assert!(val | mask(bits) == mask(bits));
}

pub struct SmallCountHashMap<T: BetterAtom, A: Allocator, const C: u32, const V: u32, const K: u32>
{
    slot_index_mask: usize,
    slots: Vec<Atom<T>, A>,
    random_state: RandomState,
    #[cfg(feature = "hash_map_debug")]
    lock: std::sync::Mutex<std::collections::BTreeMap<T, (T, T), A>>,
}

impl<T: BetterAtom, A: Allocator + Clone, const C: u32, const V: u32, const K: u32>
    SmallCountHashMap<T, A, C, V, K>
{
    pub fn with_num_slots_in(mut s: usize, allocator: A) -> Self {
        assert!((C + V + K) as usize <= std::mem::size_of::<T>() * 8);
        s = s.next_power_of_two();
        let mut slots = Vec::with_capacity_in(s, allocator.clone());
        for _ in 0..s {
            slots.push(Atom::new(T::from(0)));
        }
        SmallCountHashMap {
            slot_index_mask: s - 1,
            slots,
            random_state: RandomState::with_seed(0xee61096f95490820),
            #[cfg(feature = "hash_map_debug")]
            lock: Mutex::new(BTreeMap::new_in(allocator)),
        }
    }

    pub fn decrement(&self, k: T) -> Option<T> {
        #[cfg(feature = "hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "hash_map_debug")]
        let debug = &mut lock.get_mut(&k).unwrap();
        let mut i = self.target_slot(k);
        loop {
            let found = self.slots[i].load(Relaxed);
            if (found >> (K + V)) != T::from(0) && (found & mask(K)) == k {
                let old_val = self.slots[i].fetch_sub(T::from(1) << (K + V), Relaxed);
                let old_count = old_val >> (K + V);
                #[cfg(feature = "hash_map_debug")]
                assert_eq!(old_count, debug.1);
                let ret = if old_count == T::from(1) {
                    let v = (old_val >> K) & mask(V);
                    #[cfg(feature = "hash_map_debug")]
                    assert_eq!(v, debug.0);
                    Some(v)
                } else {
                    None
                };
                #[cfg(feature = "hash_map_debug")]
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
        #[cfg(feature = "hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "hash_map_debug")]
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

    pub fn increment_at(&self, index: usize, _k: T, amount: T) {
        #[cfg(feature = "hash_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "hash_map_debug")]
        let old_debug_count = {
            let x = &mut lock.get_mut(&_k).unwrap().1;
            *x = *x + amount;
            *x - amount
        };
        let _old = self.slots[index].fetch_add(amount << (K + V), Relaxed);
        #[cfg(feature = "hash_map_debug")]
        {
            assert!((old_debug_count >> K) & mask(V) < (T::from(1) << V));
            assert_eq!(old_debug_count, _old >> (K + V));
            assert_eq!(_k, _old & mask(K));
        }
    }

    fn target_slot(&self, k: T) -> usize {
        self.random_state.hash_one(k) as usize & self.slot_index_mask
    }
}

pub struct PageMap<A: Allocator> {
    pub base_page: Page<Size2MiB>,
    inner: SmallCountHashMap<u64, A, 16, 21, 27>,
}

impl<A: Allocator + Clone> PageMap<A> {
    pub fn new_in(num_slots: usize, base_page: Page<Size2MiB>, allocator: A) -> Self {
        PageMap {
            base_page,
            inner: SmallCountHashMap::with_num_slots_in(num_slots, allocator),
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
        self.inner.increment_at(index, page - self.base_page, 1)
    }
}
pub fn mask<T: BetterAtom>(bits: u32) -> T {
    (T::from(1) << bits) - T::from(1)
}
