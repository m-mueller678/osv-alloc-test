use crate::page_map::mask;
use crate::SystemInterface;
use std::marker::PhantomData;
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::ptr::{addr_of_mut, replace};
use x86_64::structures::paging::{PageSize, PhysFrame, Size2MiB};
use x86_64::VirtAddr;

unsafe impl<S: PageSize, Sys: SystemInterface, const C: usize> Send for FrameList<S, Sys, C> {}

pub struct FrameList<S: PageSize, Sys: SystemInterface, const C: usize> {
    head: *mut ListFrame<S, Sys, C>,
    sys: Sys,
}

#[allow(type_alias_bounds)]
pub type FrameList2M<Sys: SystemInterface> = FrameList<Size2MiB, Sys, { 512 * 512 - 2 }>;

struct ListFrame<S: PageSize, Sys: SystemInterface, const C: usize> {
    count: usize,
    next: *mut ListFrame<S, Sys, C>,
    frames: [MaybeUninit<PhysFrame<S>>; C],
    sys: PhantomData<Sys>,
}

impl<S: PageSize, Sys: SystemInterface, const C: usize> FrameList<S, Sys, C> {
    pub fn new(sys: Sys) -> Self {
        assert_eq!(size_of::<ListFrame<S, Sys, C>>(), S::SIZE as usize);
        FrameList {
            head: ptr::null_mut(),
            sys,
        }
    }
    pub fn push(&mut self, f: PhysFrame<S>) {
        unsafe {
            if !self.head.is_null() {
                let ff = &mut *self.head;
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
        if self.head.is_null() {
            return None;
        }
        unsafe {
            let next = {
                let ff = &mut *self.head;
                if ff.count > 0 {
                    ff.count -= 1;
                    return Some(Self::check_frame(ff.frames[ff.count].assume_init_read()));
                }
                ff.next
            };
            let frame = replace(&mut self.head, next);
            Some(Self::check_frame(
                PhysFrame::from_start_address(self.sys.paddr(VirtAddr::from_ptr(frame))).unwrap(),
            ))
        }
    }

    fn check_frame(a: PhysFrame<S>) -> PhysFrame<S> {
        debug_assert!(a.start_address().as_u64() & mask::<u64>(21) == 0);
        // in principle, a could be at a higher address. But most likely it was corrupted.
        debug_assert!(a.start_address().as_u64() < (1 << 46));
        a
    }

    pub fn merge_into_vec(&mut self, dst: &mut Vec<PhysFrame<S>, Sys::Alloc>) {
        while let Some(x) = self.pop() {
            dst.push(x);
        }
    }

    pub fn steal_from_vec(
        &mut self,
        src: &mut Vec<PhysFrame<S>, Sys::Alloc>,
        count: usize,
    ) -> Result<(), ()> {
        if src.len() < count {
            return Err(());
        }
        for _ in 0..count {
            self.push(src.pop().unwrap())
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    unsafe fn push_first(&mut self, f: PhysFrame<S>) {
        let old = self.head;
        self.head = self
            .sys
            .vaddr(f.start_address())
            .as_mut_ptr::<ListFrame<S, Sys, C>>();
        *addr_of_mut!((*self.head).count) = 0;
        *addr_of_mut!((*self.head).next) = old;
    }
}
