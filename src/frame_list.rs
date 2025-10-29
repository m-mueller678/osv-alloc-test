use crate::{unsafe_assert, SystemInterface};
use std::marker::PhantomData;
use std::mem::{size_of, MaybeUninit};
use std::ptr::NonNull;
use std::sync::Mutex;
use x86_64::structures::paging::{PageSize, PhysFrame, Size2MiB};
use x86_64::VirtAddr;

unsafe impl<S: PageSize, Sys: SystemInterface, const C: usize> Send for FrameList<S, Sys, C> {}

pub struct FrameList<S: PageSize, Sys: SystemInterface, const C: usize> {
    head: Option<NonNull<ListFrame<S, Sys, C>>>,
    sys: Sys,
}

#[allow(type_alias_bounds)]
pub type FrameList2M<Sys: SystemInterface> = FrameList<Size2MiB, Sys, { 512 * 512 - 1 }>;

struct ListFrame<S: PageSize, Sys: SystemInterface, const C: usize> {
    count: usize,
    frames: [MaybeUninit<PhysFrame<S>>; C],
    sys: PhantomData<Sys>,
}

impl<S: PageSize, Sys: SystemInterface, const C: usize> FrameList<S, Sys, C> {
    pub fn new(sys: Sys) -> Self {
        assert_eq!(size_of::<ListFrame<S, Sys, C>>(), S::SIZE as usize);
        FrameList { head: None, sys }
    }
    pub fn push(&mut self, f: PhysFrame<S>) {
        if let Some(mut head) = self.head {
            let list = unsafe { head.as_mut() };
            assert!(list.count < list.frames.len());
            list.frames[list.count].write(f);
            list.count += 1;
        } else {
            let vaddr = self.sys.vaddr(f.start_address());
            unsafe_assert!(!vaddr.is_null());
            self.head = Some(NonNull::new(vaddr.as_mut_ptr()).unwrap());
        }
    }

    pub const CAPACITY: usize = C + 1;

    pub fn pop(&mut self) -> Option<PhysFrame<S>> {
        let head = unsafe { self.head?.as_mut() };
        if head.count == 0 {
            return self.head.take().map(|x| {
                let vaddr = unsafe { VirtAddr::new_unsafe(x.as_ptr().addr() as u64) };
                let paddr = self.sys.paddr(vaddr);
                unsafe_assert!(paddr.is_aligned(S::SIZE));
                PhysFrame::from_start_address(paddr).unwrap()
            });
        }
        head.count -= 1;
        Some(unsafe { head.frames[head.count].assume_init_read() })
    }

    pub fn release_extra_to_vec(&mut self, dst: &Mutex<Vec<PhysFrame<S>, Sys::Alloc>>) {
        if self.count() > 4 {
            let mut dst = dst.lock().unwrap();
            while self.count() > 1 {
                dst.push(self.pop().unwrap());
            }
        }
    }

    pub fn count(&self) -> usize {
        if let Some(head) = self.head {
            unsafe { head.as_ref().count + 1 }
        } else {
            0
        }
    }

    pub fn steal_from_vec(
        &mut self,
        src: &Mutex<Vec<PhysFrame<S>, Sys::Alloc>>,
        target_count: usize,
    ) -> Result<(), ()> {
        if self.count() >= target_count {
            return Ok(());
        }
        let mut src = src.lock().unwrap();
        while self.count() < target_count {
            self.push(src.pop().ok_or(())?);
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }
}
