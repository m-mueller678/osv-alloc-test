use ahash::RandomState;
use static_assertions::const_assert;
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
    #[cfg(debug_assertions)]
    lock: Mutex<()>,
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
            #[cfg(debug_assertions)]
            lock: Mutex::new(()),
        }
    }

    pub fn decrement(&self, page: Page<Size2MiB>) -> Option<PhysFrame<Size2MiB>> {
        #[cfg(debug_assertions)]
        let _g = self.lock.lock().unwrap();

        const_assert!(PAGE_SHIFT == 0);
        const_assert!(COUNT_SHIFT + COUNT_BITS == 64);
        let page = page - self.base_page;
        let mut i = self.target_slot(page);
        loop {
            let found = self.slots[i].load(Relaxed);
            if (found >> COUNT_SHIFT) != 0 && (found & mask(PAGE_BITS)) == page {
                let old_val = self.slots[i].fetch_sub(1 << COUNT_SHIFT, Relaxed);
                return if old_val >> COUNT_SHIFT == 1 {
                    let frame = (old_val >> FRAME_SHIFT) & mask(FRAME_BITS);
                    Some(PhysFrame::from_start_address(PhysAddr::new(frame << 21)).unwrap())
                } else {
                    None
                };
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    pub fn insert(&self, page: Page<Size2MiB>, frame: PhysFrame<Size2MiB>, count: usize) -> usize {
        #[cfg(debug_assertions)]
        let _g = self.lock.lock().unwrap();
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
            if x >> COUNT_BITS == 0 {
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
        #[cfg(debug_assertions)]
        let _g = self.lock.lock().unwrap();

        const_assert!(PAGE_SHIFT == 0);
        let old = self.slots[index].fetch_add(1 << COUNT_SHIFT, Relaxed);
        debug_assert!(page - self.base_page == old & mask(PAGE_BITS));
    }

    fn target_slot(&self, page: u64) -> usize {
        self.random_state.hash_one(page) as usize & self.slot_index_mask
    }
}
