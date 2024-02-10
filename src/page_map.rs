use crate::mask;
use ahash::RandomState;
use next_gen::generator;
use radium::marker::{Atomic, BitOps, NumericOps};
use radium::{Atom, Radium};
use rand::prelude::*;
use rand::Rng;
use std::fmt::Debug;
use std::hash::Hash;
use std::mem::size_of;
use std::ops::{Shl, Shr};
use std::sync::atomic::Ordering::Relaxed;
use std::thread::yield_now;
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

fn check_width(val: impl BetterAtom, bits: u32) {
    debug_assert!(val | crate::mask(bits) == crate::mask(bits));
}

pub struct SmallCountHashMap<T: BetterAtom, const C: u32, const V: u32, const K: u32> {
    slot_index_mask: usize,
    slots: Vec<Atom<T>>,
    random_state: RandomState,
    #[cfg(feature = "hash_map_debug")]
    lock: std::sync::Mutex<std::collections::BTreeMap<T, (T, T)>>,
}

impl<T: BetterAtom, const C: u32, const V: u32, const K: u32> SmallCountHashMap<T, C, V, K> {
    pub fn with_num_slots(mut s: usize) -> Self {
        assert!((C + V + K) as usize <= std::mem::size_of::<T>() * 8);
        s = s.next_power_of_two();
        SmallCountHashMap {
            slot_index_mask: s - 1,
            slots: (0..s).map(|_| Atom::new(T::from(0))).collect(),
            random_state: RandomState::with_seed(0xee61096f95490820),
            #[cfg(feature = "hash_map_debug")]
            lock: Default::default(),
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
            if (found >> (K + V)) != T::from(0) && (found & crate::mask(K)) == k {
                let old_val = self.slots[i].fetch_sub(T::from(1) << (K + V), Relaxed);
                let old_count = old_val >> (K + V);
                #[cfg(feature = "hash_map_debug")]
                assert_eq!(old_count, debug.1);
                let ret = if old_count == T::from(1) {
                    let v = (old_val >> K) & crate::mask(V);
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
            assert_eq!(old_debug_count, _old >> (K + V));
            assert_eq!(_k, _old & crate::mask(K));
        }
    }

    fn target_slot(&self, k: T) -> usize {
        self.random_state.hash_one(k) as usize & self.slot_index_mask
    }
}

pub struct RhHash<T: BetterAtom, const K: u32> {
    index_mask: usize,
    slots: Vec<Atom<T>>,
    random_state: RandomState,
    #[cfg(feature = "hash_map_debug")]
    lock: std::sync::Mutex<ahash::AHashMap<T, T>>,
}

impl<T: BetterAtom, const K: u32> RhHash<T, K> {
    pub fn new(slot_count: usize) -> Self {
        assert!(slot_count.is_power_of_two());
        RhHash {
            index_mask: slot_count.saturating_sub(1),
            slots: (0..slot_count)
                .map(|_| Atom::<T>::new(T::from(0)))
                .collect(),
            random_state: RandomState::with_seed(0xee61096f95490820),
            #[cfg(feature = "hash_map_debug")]
            lock: std::sync::Mutex::new(ahash::AHashMap::with_hasher(RandomState::with_seed(
                0xee61096f95490820,
            ))),
        }
    }
    pub fn update<A, F: FnMut(T) -> (A, T)>(&self, k: T, mut f: F) -> A {
        #[cfg(feature = "hash_map_debug")]
        let mut l = self.lock.lock().unwrap();

        debug_assert!(k | crate::mask(K) == crate::mask(K));
        let mut fa = |x: T| {
            let r = f(x);
            debug_assert!(r.1 & crate::mask(K) == k);
            debug_assert!(r.1 & Self::l_mask() == T::from(0));
            r
        };

        let target_slot = self.target_slot(k);
        let mut self_psl = 0;
        let mut scan_slot = target_slot;
        loop {
            let peeked = self.slots[scan_slot].load(Relaxed);
            if peeked & Self::l_mask() != T::from(0) {
                Self::do_yield();
                continue;
            }
            if peeked & crate::mask(K) == k {
                #[cfg(feature = "hash_map_debug")]
                assert_eq!(l.get(&k), Some(&peeked));
                let (ret, new) = fa(peeked);
                if new & Self::v_mask() == T::from(0) {
                    #[cfg(feature = "hash_map_debug")]
                    l.remove(&k);
                    match self.slots[scan_slot].compare_exchange_weak(
                        peeked,
                        Self::l_mask(),
                        Relaxed,
                        Relaxed,
                    ) {
                        Ok(_) => {
                            self.unlock_range(target_slot, scan_slot);
                            self.remove(scan_slot);
                            return ret;
                        }
                        Err(_) => continue,
                    }
                } else {
                    #[cfg(feature = "hash_map_debug")]
                    l.insert(k, new);
                    match self.slots[scan_slot].compare_exchange_weak(peeked, new, Relaxed, Relaxed)
                    {
                        Ok(_) => {
                            self.unlock_range(target_slot, scan_slot);
                            return ret;
                        }
                        Err(_) => continue,
                    }
                }
            } else if peeked == T::from(0) {
                #[cfg(feature = "hash_map_debug")]
                assert!(l.get(&k).is_none());
                let (ret, new) = fa(k);
                if new & Self::v_mask() == T::from(0) {
                    self.unlock_range(target_slot, scan_slot);
                    return ret;
                } else {
                    l.insert(k, new);
                    match self.slots[scan_slot].compare_exchange_weak(peeked, new, Relaxed, Relaxed)
                    {
                        Ok(_) => {
                            self.unlock_range(target_slot, scan_slot);
                            return ret;
                        }
                        Err(_) => continue,
                    }
                }
            } else {
                let peek_locked = self.slots[scan_slot].fetch_or(Self::l_mask(), Relaxed);
                if peek_locked & Self::l_mask() != T::from(0) {
                    Self::do_yield();
                    continue;
                }
                let other_psl = self.psl(self.target_slot(peek_locked & crate::mask(K)), scan_slot);
                if self_psl > other_psl {
                    #[cfg(feature = "hash_map_debug")]
                    assert!(l.get(&k).is_none());
                    let (ret, new) = fa(k);
                    if new & Self::v_mask() != T::from(0) {
                        #[cfg(feature = "hash_map_debug")]
                        l.insert(k, new);
                        self.slots[scan_slot].store(new | Self::l_mask(), Relaxed);
                        self.propagate_insert(
                            self.next(scan_slot),
                            other_psl + 1,
                            peek_locked,
                            target_slot,
                        );
                    } else {
                        self.slots[scan_slot].fetch_and(!Self::l_mask(), Relaxed);
                        self.unlock_range(target_slot, scan_slot);
                    }
                    return ret;
                } else {
                    self_psl += 1;
                    scan_slot = self.next(scan_slot);
                    continue;
                }
            }
        }
    }

    pub fn remove_any(&self, rng: &mut impl Rng, mut max_scan: usize) -> T {
        loop {
            let mut i = rng.gen::<usize>() & self.index_mask;
            loop {
                if max_scan == 0 {
                    return T::from(0);
                }
                max_scan -= 1;
                let peek = self.slots[i].load(Relaxed);
                if peek != T::from(0) {
                    if peek & Self::l_mask() != T::from(0) {
                        break;
                    }
                    if self.slots[i]
                        .compare_exchange_weak(peek, Self::l_mask(), Relaxed, Relaxed)
                        .is_ok()
                    {
                        self.remove(i);
                        #[cfg(feature = "hash_map_debug")]
                        assert_eq!(
                            self.lock.lock().unwrap().remove(&(peek & mask::<T>(K))),
                            Some(peek)
                        );
                        return peek;
                    } else {
                        break;
                    }
                }
                i = self.next(i)
            }
        }
    }

    fn do_yield() {
        yield_now();
    }

    fn propagate_insert(&self, mut i: usize, mut psl: usize, mut displaced: T, unlock_from: usize) {
        loop {
            let peek_locked = self.slots[i].fetch_or(Self::l_mask(), Relaxed);
            if peek_locked & Self::l_mask() != T::from(0) {
                Self::do_yield();
                continue;
            }
            if peek_locked == T::from(0) {
                self.slots[i].store(displaced, Relaxed);
                self.unlock_range(unlock_from, i);
                return;
            }
            let other_psl = self.psl(self.target_slot(peek_locked & crate::mask(K)), i);
            if psl > other_psl {
                self.slots[i].store(Self::l_mask() | displaced, Relaxed);
                displaced = peek_locked;
                psl = other_psl;
            }
            i = self.next(i);
            psl += 1;
        }
    }

    fn bits() -> u32 {
        size_of::<T>() as u32 * 8
    }

    fn v_mask() -> T {
        crate::mask::<T>(Self::bits() - K - 1) << K
    }

    fn l_mask() -> T {
        T::from(1) << (Self::bits() - 1)
    }

    fn unlock_range(&self, start: usize, end: usize) {
        let mut i = start;
        while i != end {
            let pre = self.slots[i].fetch_and(!Self::l_mask(), Relaxed);
            debug_assert!(pre & Self::l_mask() != T::from(0));
            i = self.next(i);
        }
    }

    fn remove(&self, mut i: usize) {
        loop {
            let ni = self.next(i);
            let peek = self.slots[ni].load(Relaxed);
            if peek == T::from(0) {
                self.slots[i].store(T::from(0), Relaxed);
                return;
            }
            if peek & Self::l_mask() != T::from(0) {
                Self::do_yield();
                continue;
            }
            let next_locked = self.slots[ni].fetch_or(Self::l_mask(), Relaxed);
            if next_locked & Self::l_mask() != T::from(0) {
                Self::do_yield();
                continue;
            }
            if next_locked == T::from(0) || self.target_slot(next_locked & crate::mask(K)) == ni {
                self.slots[i].store(T::from(0), Relaxed);
                self.slots[ni].fetch_and(!Self::l_mask(), Relaxed);
                return;
            } else {
                self.slots[i].store(next_locked, Relaxed);
                i = ni;
            }
        }
    }

    fn next(&self, i: usize) -> usize {
        (i + 1) & self.index_mask
    }

    fn target_slot(&self, k: T) -> usize {
        debug_assert!(k | crate::mask(K) == crate::mask(K));
        self.random_state.hash_one(k) as usize & self.index_mask
    }

    fn psl(&self, target_slot: usize, slot: usize) -> usize {
        slot.wrapping_sub(target_slot) & self.index_mask
    }

    pub fn count(&self) -> usize {
        self.slots
            .iter()
            .filter(|x| x.load(Relaxed) & !Self::l_mask() != T::from(0))
            .count()
    }

    // pub fn drain(&self)->impl '_+Iterator<Item=u32>{
    //     let generator = Self::drain_inner;
    //     mk_gen!(let x=generator(self));
    //     x
    // }

    #[generator(yield(T))]
    pub fn drain(this: &Self) -> Option<T> {
        let mut locked_from = 0;
        let mut i = 0;
        let mut may_stop = false;
        loop {
            let peek_lock = this.slots[i].fetch_or(Self::l_mask(), Relaxed);
            if peek_lock & Self::l_mask() != T::from(0) {
                Self::do_yield();
                continue;
            }
            let ni = this.next(i);
            if ni == 0 {
                may_stop = true;
            }
            if peek_lock == T::from(0) {
                this.unlock_range(locked_from, ni);
                locked_from = ni;
                i = ni;
                if may_stop {
                    return None;
                }
            } else {
                yield_!(peek_lock);
                i = ni;
            }
        }
    }
}

#[test]
fn test_rh() {
    const BITS: u32 = 32;
    const SIZE: u64 = 512;
    const LIFETIME: u64 = SIZE - 1;
    const ITER: u64 = 20;

    let rh = RhHash::<u64, 30>::new(SIZE as usize);

    for i in 0..(ITER * SIZE) {
        for j in 0..=LIFETIME.min(i) {
            rh.update(i - j, |x| {
                let k = i - j;
                assert_eq!(x, if j == 0 { 0 } else { i } << BITS | k);
                let v = if j == LIFETIME { 0 } else { i + 1 };
                ((), v << BITS | k)
            });
            // for x in &rh.slots{
            //     eprint!("0x{:016x?}, ",x.load(Relaxed));
            // }
            // eprintln!();
        }
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
        self.inner.increment_at(index, page - self.base_page, 1)
    }
}
