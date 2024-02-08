use ahash::AHasher;
use modular_bitfield::prelude::*;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::thread::yield_now;
use x86_64::PhysAddr;
use x86_64::structures::paging::{Page, PageSize, PhysFrame, Size2MiB};

const FRAME_BITS: u32 = 19;
const COUNT_BITS: u32 = 16;
const PAGE_BITS: u32 = 63 - COUNT_BITS - FRAME_BITS;

const PAGE_SHIFT: u32 = 0;
const FRAME_SHIFT: u32 = PAGE_BITS;
const COUNT_SHIFT: u32 = FRAME_SHIFT + FRAME_BITS;

impl PageRecord {
    fn lock(mut self) -> Self {
        self.set_is_locked(true);
        self
    }

    fn to_u64(self) -> u64 {
        u64::from_ne_bytes(self.into_bytes())
    }

    fn from_u64(x: u64) -> Self {
        Self::from_bytes(x.to_ne_bytes())
    }
}

struct PageMap {
    base_page: Page<Size2MiB>,
    slot_index_mask: usize,
    slots: Vec<AtomicU64>,
    random_state: ahash::RandomState,
}

const MAX_ALLOCS_PER_PAGE: usize = 1 << COUNT_BITS - 1;

fn mask(bits: u32) -> u64 {
    1u64 << bits - 1
}

fn check_width(val: u64, bits: u32) {
    debug_assert!(val | mask(bits) == mask(bits));
}

impl PageMap {
    pub fn decrement_and_remove_0(&self, page: Page<Size2MiB>) -> Option<PhysFrame<Size2MiB>> {
        assert!(PAGE_SHIFT == 0);
        assert!(COUNT_SHIFT + COUNT_BITS == 64);
        let target_page = page - self.base_page;
        let mut i = self.target_slot(target_page);
        loop {
            let found = self.load(i);
            if (found >> COUNT_SHIFT) != 0 && (found & mask(PAGE_BITS)) == target_page {
                let old_val = self.slots[i].fetch_sub(1 << COUNT_SHIFT, Relaxed);
                return if old_val >> COUNT_SHIFT == 1 {
                    let frame = old_val >> FRAME_SHIFT & (1 << FRAME_BITS - 1);
                    Some(PhysFrame::from_start_address(PhysAddr::new(frame << 21)).unwrap())
                } else {
                    None
                };
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    pub fn insert(
        &self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        count: usize,
    ) {
        assert!(COUNT_SHIFT + COUNT_BITS == 64);
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
                if self.slots[i].compare_exchange(x, record, Relaxed, Relaxed).is_ok() {
                    break;
                } else {
                    continue;
                }
            } else {
                i = (i + 1) & self.slot_index_mask;
            }
        }
    }

    fn target_slot(&self, page: u64) -> usize {
        self.random_state.hash_one(page) as usize & self.slot_index_mask
    }
}
