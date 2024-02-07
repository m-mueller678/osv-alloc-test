use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;
use std::thread::yield_now;
use ahash::AHasher;
use modular_bitfield::prelude::*;
use x86_64::structures::paging::{Page, Size2MiB, PhysFrame, PageSize};

const FRAME_BITS:u32=19;
const COUNT_BITS:u32=16;
const PAGE_BITS:u32=63-COUNT_BITS-FRAME_BITS;

const PAGE_SHIFT:u32=0;
const FRAME_SHIFT:u32=PAGE_BITS;
const COUNT_SHIFT:u32=FRAME_SHIFT+FRAME_BITS;

type PageField=B28;
type FrameField=B19;
type CountField=B16;

#[bitfield(bits=64)]
#[repr(align(8))]
#[derive(Copy)]
struct PageRecord{
    page:PageField,
    frame:FrameField,
    count:CountField,
    locked:bool,
}

impl PageRecord{
    fn locked(mut self)->Self{
        self.set_locked(true);
        self
    }

    fn to_u64(self)->u64{
        u64::from_ne_bytes(self.into_bytes())
    }

    fn from_u64(x:u64)->Self{
        Self::from_bytes(x.to_ne_bytes())
    }
}

struct PageMap{
    base_page:Page<Size2MiB()>,
    slot_index_mask:usize,
    slots:Vec<AtomicU64>,
    random_state:ahash::RandomState,
}

const MAX_ALLOCS_PER_PAGE:usize = 1<<COUNT_BITS-1;

impl PageMap{
    pub fn decrement_and_remove_0(&self,page:Page::<Size2MiB>)->Option<PhysFrame<Size2MiB>>{
        unimplemented!()
    }

    pub fn insert(&self,page:Page::<Size2MiB>,frame:PhysFrame<Size2MiB>,count:usize)->Option<PhysFrame<Size2MiB>>{
        let page_index = page - self.base_page;
        let frame_index = frame.start_address().as_u64() / Size2MiB::SIZE;
        debug_assert!(page_index < 1<<PAGE_BITS);
        debug_assert!(frame_index < 1<<FRAME_BITS);
        debug_assert!(count<1<<COUNT_BITS);
        let mut to_insert = page_index | ((count as u64) << FRAME_BITS | frame_index)<<PAGE_BITS;
        let mut target_slot = self.target_slot(record);
        let mut scan_slot=target_slot;
        loop{
            let update_result = self.update(target_slot,|p|{
                if p.count()==0{
                    Ok((to_insert,None))
                }else{
                    let other_target_slot = self.target_slot(p);
                    if self.psl(target_slot,scan_slot) < self.psl(other_target_slot,scan_slot){
                        Ok((p.locked(),Some((target_slot,to_insert))))
                    }else{
                        Ok((to_insert.locked(),(other_target_slot,p)))
                    }
                }
            });
            let present = PageRecord::load(&self.slots[target_slot]);
            if present.count() == 0{

            }else{
                debug_assert!(present.page()!=page_index);
            }
        }
    }

    fn target_slot(&self,record:PageRecord)->usize{
        self.random_state.hash_one(record.page()) as usize & self.slot_index_mask;
    }

    fn load(&self,i:usize)->PageRecord{
        PageRecord::from_u64(self.slots[i].load(Relaxed))
    }

    fn update<F:FnMut(PageRecord)->Result<(PageRecord,A),B>,A,B>(&self,i:usize,mut f:F,old:PageRecord,new:PageRecord)->Result<A,B>{
        loop{
            let mut curr=PageRecord::from_u64(self.slots[i].load(Relaxed));
            while! curr.locked(){
                match f(curr){
                    Ok((n,a))=>match self.slots[i].compare_exchange_weak(curr.to_u64(),n.to_u64(),Relaxed,Relaxed){
                        Ok(_)=>return Ok(a),
                        Err(found)=>{ curr=PageRecord::from_u64(found); }
                    }
                    Err(b)=>Err(b),
                }
            }
            yield_now();
        }
    }

    fn psl(&self,target_slot:usize,actual_slot:usize)->usize{
        (actual_slot-target_slot) & self.slot_index_mask
    }
}