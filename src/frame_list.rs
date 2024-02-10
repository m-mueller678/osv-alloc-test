use crate::mask;
use crate::paging::{paddr, vaddr};
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::ptr::{addr_of_mut, replace};
use crossbeam_queue::ArrayQueue;
use x86_64::structures::paging::{PageSize, PhysFrame, Size2MiB, Size4KiB};
use x86_64::VirtAddr;

unsafe impl<S: PageSize, const C: usize> Send for FrameList<S, C> {}

pub struct FrameList<S: PageSize, const C: usize>(*mut ListFrame<S, C>);

pub type FrameList4K = FrameList<Size4KiB, { 512 - 2 }>;
pub type FrameList2M = FrameList<Size2MiB, { 512 * 512 - 2 }>;

struct ListFrame<S: PageSize, const C: usize> {
    count: usize,
    next: *mut ListFrame<S, C>,
    frames: [MaybeUninit<PhysFrame<S>>; C],
}

impl<S: PageSize, const C: usize> Default for FrameList<S, C> {
    fn default() -> Self {
        assert_eq!(size_of::<ListFrame<S, C>>(), S::SIZE as usize);
        FrameList(ptr::null_mut())
    }
}

impl<S: PageSize, const C: usize> FrameList<S, C> {
    pub fn push(&mut self, f: PhysFrame<S>) {
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

    pub fn pop(&mut self) -> Option<PhysFrame<S>> {
        if self.0.is_null() {
            return None;
        }
        unsafe {
            let next = {
                let ff = &mut *self.0;
                if ff.count > 0 {
                    ff.count -= 1;
                    return Some(Self::check_frame(ff.frames[ff.count].assume_init_read()));
                }
                ff.next
            };
            let frame = replace(&mut self.0, next);
            Some(Self::check_frame(
                PhysFrame::from_start_address(paddr(VirtAddr::from_ptr(frame))).unwrap(),
            ))
        }
    }

    fn check_frame(a: PhysFrame<S>) -> PhysFrame<S> {
        debug_assert!(a.start_address().as_u64() & mask::<u64>(21) == 0);
        // in principle, a could be at a higher address. But most likely it was corrupted.
        debug_assert!(a.start_address().as_u64() < (1 << 46));
        a
    }

    pub fn merge_into(&mut self, dst: &ArrayQueue<PhysFrame<S>>) {
        while let Some(x) = self.pop() {
            dst.push(x).unwrap();
        }
    }

    pub fn steal(&mut self, src: &ArrayQueue<PhysFrame<S>>, count: usize) -> Result<(), ()> {
        if src.len() < count {
            return Err(());
        }
        for _ in 0..count {
            match src.pop(){
                Some(x)=>self.push(x),
                None=>{
                    self.merge_into(src);
                    return Err(())
                }
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_null()
    }

    unsafe fn push_first(&mut self, f: PhysFrame<S>) {
        let old = self.0;
        self.0 = vaddr(f.start_address()).as_mut_ptr::<ListFrame<S, C>>();
        *addr_of_mut!((*self.0).count) = 0;
        *addr_of_mut!((*self.0).next) = old;
    }
}
