use std::mem::MaybeUninit;
use std::ptr::{addr_of, addr_of_mut};
use std::sync::atomic::AtomicU64;
use x86_64::structures::paging::{PageSize, PhysFrame};
use x86_64::VirtAddr;
use crate::paging::{paddr, vaddr};

#[derive(Default)]
struct FrameList<S:PageSize>(
    *mut ListFrame<S>,
);

struct ListFrame<S:PageSize>{
    count:usize,
    next: *mut ListFrame<S>,
    frames: [MaybeUninit<PhysFrame<S>>,S::SIZE/8-2],
}

impl<S:PageSize> FrameList<S>{
    fn push(&mut self,f:PhysFrame<S>){
        unsafe {
            if !self.0.is_null() {
                let ff = &mut *self.0;
                if ff.count < ff.frames.len() {
                    ff.frames[ff.count].write(f);
                    ff.count += 1;
                    return;
                }
            }
            self.push_first(f)
        }
    }

    fn pop(&mut self)->Option<PhysFrame>{
        if self.0.is_null(){return None}
        unsafe{
            {
                let ff = &mut *self.0;
                if ff.count > 0 {
                    ff.count -= 1;
                    return Some(ff.frames[ff.count].assume_init_read());
                }
            }
            Some(PhysFrame::from_start_address(paddr(VirtAddr::from_ptr(self.0))).unwrap())
        }
    }

    fn merge_into_vec(&mut self,dst:&mut Vec<PhysFrame<S>>){
        todo!()
    }

    fn steal_from_vec(&mut self,src:&mut Vec<PhysFrame<S>>,count:usize){
        debug_assert!(self.0.is_null());
        todo!()
    }

    unsafe fn push_first(&mut self,f:PhysFrame<S>){
        let old=self.0;
        self.0 = vaddr(f.start_address()).as_mut_ptr::<ListFrame<S>>();
        *addr_of_mut!((*self.0).count)=0;
        *addr_of_mut!((*self.0).next)=old;
    }
}