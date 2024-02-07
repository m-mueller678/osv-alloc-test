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

fn mask(bits:u32)->u64{
    1u64<<bits -1
}

impl PageMap {
    pub fn decrement_and_remove_0(&self, page: Page<Size2MiB>) -> Option<PhysFrame<Size2MiB>> {
        assert!(PAGE_SHIFT==0);
        assert!(COUNT_SHIFT+COUNT_BITS==64);
        let target_page=page - self.base_page;
        let mut target_slot = self.target_slot(target_page);
        let mut scan_slot = target_slot;
        loop {
            let found = self.load(scan_slot);
            if (found>>COUNT_SHIFT)!=0 && (found & mask(PAGE_BITS)) == target_page{
                let old_val = self.slots[scan_slot].fetch_sub(1<<COUNT_SHIFT,Relaxed);
                if old_val>>COUNT_SHIFT == 1{
                    let frame = old_val >> FRAME_SHIFT & (1<<FRAME_BITS -1);
                    return Some(PhysFrame::from_start_address(PhysAddr::new(frame<<21)).unwrap())
                }else{
                    return None
                }
            }else{
                scan_slot+=1;
            }
        }
    }

    pub fn insert(
        &self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        count: usize,
    ){

        let
        let mut to_insert = PageRecord::new();
        to_insert.set_count(count.try_into().unwrap());
        to_insert.set_frame(
            (frame.start_address().as_u64() / Size2MiB::SIZE)
                .try_into()
                .unwrap(),
        );
        to_insert.set_page((page - self.base_page).try_into().unwrap());
        let mut target_slot = self.target_slot(to_insert);
        let first_target_slot = target_slot;
        let mut scan_slot = target_slot;
        loop {
            let Ok(update_result):Result<_,()> = self.update(target_slot, |p| {
                if p.count() == 0 {
                    Ok((to_insert, None))
                } else {
                    let other_target_slot = self.target_slot(p);
                    if self.psl(target_slot, scan_slot) < self.psl(other_target_slot, scan_slot) {
                        Ok((p.lock(), Some((target_slot, to_insert))))
                    } else {
                        Ok((to_insert.lock(), Some((other_target_slot, p))))
                    }
                }
            }) else {
                unreachable!()
            };
            if let Some((new_target, new_record)) = update_result {
                target_slot = new_target;
                to_insert = new_record;
                scan_slot += 1;
            }else{
                break;
            }
        }
        for i in first_target_slot..scan_slot{
            self.unlock(i);
        }
    }

    fn target_slot(&self, page: u64) -> usize {
        self.random_state.hash_one(page) as usize & self.slot_index_mask
    }

    fn load(&self, i: usize) -> PageRecord {
        PageRecord::from_u64(self.slots[i].load(Relaxed))
    }

    fn unlock(&self,i:usize){
        debug_assert!(self.load(i).is_locked());
        self.slots[i].fetch_sub(1<<63,Relaxed);
    }

    fn update<F: FnMut(PageRecord) -> Result<(PageRecord, A), B>, A, B>(
        &self,
        i: usize,
        mut f: F,
    ) -> Result<A, B> {
        loop {
            let mut curr = PageRecord::from_u64(self.slots[i].load(Relaxed));
            while !curr.is_locked() {
                match f(curr) {
                    Ok((n, a)) => match self.slots[i].compare_exchange_weak(
                        curr.to_u64(),
                        n.to_u64(),
                        Relaxed,
                        Relaxed,
                    ) {
                        Ok(_) => return Ok(a),
                        Err(found) => {
                            curr = PageRecord::from_u64(found);
                        }
                    },
                    Err(b) => return Err(b),
                }
            }
            yield_now();
        }
    }

    fn psl(&self, target_slot: usize, actual_slot: usize) -> usize {
        (actual_slot - target_slot) & self.slot_index_mask
    }
}
