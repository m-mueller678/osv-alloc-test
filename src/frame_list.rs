use crate::util::unsafe_assert;
use crate::SystemInterface;
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

    pub unsafe fn push(&mut self, f: PhysFrame<S>) -> Result<(), ()> {
        if let Some(mut head) = self.head {
            let list = unsafe { head.as_mut() };
            unsafe_assert!(list.count <= list.frames.len());
            if list.count == list.frames.len() {
                return Err(());
            }
            list.frames[list.count].write(f);
            list.count += 1;
        } else {
            let vaddr = self.sys.vaddr(f.start_address());
            unsafe_assert!(!vaddr.is_null());
            let mut head = NonNull::<ListFrame<S, Sys, C>>::new(vaddr.as_mut_ptr()).unwrap();
            head.as_mut().count = 0;
            self.head = Some(head);
        }
        Ok(())
    }

    pub unsafe fn push_with_spill(
        &mut self,
        f: PhysFrame<S>,
        dst: &Mutex<Vec<PhysFrame<S>, Sys::Alloc>>,
    ) {
        if self.push(f).is_err() {
            let mut dst = dst.lock().unwrap();
            while self.count() > 1 {
                dst.push(self.pop().unwrap());
            }
            self.push(f).unwrap();
        }
    }

    pub const CAPACITY: usize = C + 1;
    pub const DEFAULT_REFILL_SIZE: usize = 4;

    pub fn pop_with_refill(
        &mut self,
        src: &Mutex<Vec<PhysFrame<S>, Sys::Alloc>>,
        refill_size: usize,
    ) -> Option<PhysFrame<S>> {
        assert!(refill_size > 0);
        if let Some(x) = self.pop() {
            return Some(x);
        }
        self.steal_from_vec(src, refill_size)?;
        Some(self.pop().unwrap())
    }

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
    ) -> Option<()> {
        if self.count() >= target_count {
            return Some(());
        }
        assert!(target_count < Self::CAPACITY);
        let mut src = src.lock().unwrap();
        while self.count() < target_count {
            unsafe { self.push(src.pop()?).unwrap() };
        }
        Some(())
    }
}
