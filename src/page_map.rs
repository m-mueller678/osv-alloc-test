use ahash::RandomState;
use static_assertions::const_assert;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Mutex;
use x86_64::structures::paging::{Page, PhysFrame, Size2MiB};
use x86_64::PhysAddr;

const FRAME_BITS: u32 = 20;
const COUNT_BITS: u32 = 16;
const PAGE_BITS: u32 = 64 - COUNT_BITS - FRAME_BITS;

const PAGE_SHIFT: u32 = 0;
const FRAME_SHIFT: u32 = PAGE_BITS;
const COUNT_SHIFT: u32 = FRAME_SHIFT + FRAME_BITS;

pub struct PageMap {
    pub base_page: Page<Size2MiB>,
    slot_index_mask: usize,
    slots: Vec<AtomicU64>,
    random_state: ahash::RandomState,
    #[cfg(feature = "page_map_debug")]
    lock: Mutex<BTreeMap<Page<Size2MiB>, (PhysFrame<Size2MiB>, u64)>>,
}

pub const MAX_ALLOCS_PER_PAGE: usize = 1 << (COUNT_BITS - 1);

fn mask(bits: u32) -> u64 {
    (1u64 << bits) - 1
}

fn check_width(val: u64, bits: u32) {
    debug_assert!(val | mask(bits) == mask(bits));
}

impl PageMap {
    pub fn with_num_slots(mut s: usize, base_page: Page<Size2MiB>) -> Self {
        s = s.next_power_of_two();
        PageMap {
            base_page,
            slot_index_mask: s - 1,
            slots: (0..s).map(|_| AtomicU64::new(0)).collect(),
            random_state: RandomState::with_seed(0xee61096f95490820),
            #[cfg(feature = "page_map_debug")]
            lock: Default::default(),
        }
    }

    pub fn decrement(&self, page: Page<Size2MiB>) -> Option<PhysFrame<Size2MiB>> {
        const_assert!(PAGE_SHIFT == 0);
        const_assert!(COUNT_SHIFT + COUNT_BITS == 64);
        #[cfg(feature = "page_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "page_map_debug")]
        let debug = &mut lock.get_mut(&page).unwrap();
        let page_i = page - self.base_page;
        let mut i = self.target_slot(page_i);
        loop {
            let found = self.slots[i].load(Relaxed);
            if (found >> COUNT_SHIFT) != 0 && (found & mask(PAGE_BITS)) == page_i {
                let old_val = self.slots[i].fetch_sub(1 << COUNT_SHIFT, Relaxed);
                let old_count = old_val >> COUNT_SHIFT;
                #[cfg(feature = "page_map_debug")]
                assert_eq!(old_count, debug.1);
                let ret = if old_count == 1 {
                    let frame = (old_val >> FRAME_SHIFT) & mask(FRAME_BITS);
                    let frame = PhysFrame::from_start_address(PhysAddr::new(frame << 21)).unwrap();
                    #[cfg(feature = "page_map_debug")]
                    assert_eq!(frame, debug.0);
                    Some(frame)
                } else {
                    None
                };
                #[cfg(feature = "page_map_debug")]
                {
                    debug.1 -= 1;
                    if debug.1 == 0 {
                        lock.remove(&page);
                    }
                }
                return ret;
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    pub fn insert(&self, page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>, count: usize) -> usize {
        #[cfg(feature = "page_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "page_map_debug")]
        {
            assert!(lock.insert(page, (frame, count as u64)).is_none());
        }
        const_assert!(COUNT_SHIFT + COUNT_BITS == 64);
        let page = page - self.base_page;
        let frame = frame.start_address().as_u64() >> 21;
        let count = count as u64;
        check_width(page, PAGE_BITS);
        check_width(frame, FRAME_BITS);
        check_width(count, COUNT_BITS);
        let record = page << PAGE_SHIFT | frame << FRAME_SHIFT | count << COUNT_SHIFT;
        let mut i = self.target_slot(page);
        loop {
            let x = self.slots[i].load(Relaxed);
            if x >> COUNT_SHIFT == 0 {
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

    pub fn increment_at(&self, index: usize, page: Page<Size2MiB>) {
        const_assert!(PAGE_SHIFT == 0);
        #[cfg(feature = "page_map_debug")]
        let mut lock = self.lock.lock().unwrap();
        #[cfg(feature = "page_map_debug")]
        let old_debug_count = {
            let x = &mut lock.get_mut(&page).unwrap().1;
            *x += 1;
            *x - 1
        };
        let old = self.slots[index].fetch_add(1 << COUNT_SHIFT, Relaxed);
        #[cfg(feature = "page_map_debug")]
        {
            assert_eq!(old_debug_count, old >> COUNT_SHIFT);
            assert_eq!(page - self.base_page, old & mask(PAGE_BITS));
        }
    }

    fn target_slot(&self, page: u64) -> usize {
        self.random_state.hash_one(page) as usize & self.slot_index_mask
    }
}
