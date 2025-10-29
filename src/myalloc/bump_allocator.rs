use crate::{
    constants::PAGE_SIZE,
    myalloc::{align_down, wrapping_less_than, LocalCommon},
    unsafe_assert, GlobalData, SystemInterface,
};
use std::{
    alloc::Layout,
    marker::PhantomData,
    mem,
    ops::Deref,
    ptr::NonNull,
    sync::atomic::{
        AtomicUsize,
        Ordering::{Acquire, Relaxed, Release},
    },
};
use x86_64::{structures::paging::PhysFrame, VirtAddr};

pub struct SmallAllocator<S: SystemInterface, G: Deref<Target = GlobalData<S>>> {
    bump: usize,
    bump_limit: usize,
    _p: PhantomData<fn() -> G>,
}

struct BumpHeader {
    count: AtomicUsize,
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>> SmallAllocator<S, G> {
    #[inline]
    pub fn alloc(&mut self, common: &mut LocalCommon<S, G>, layout: Layout) -> Option<NonNull<u8>> {
        let size = layout.size();
        unsafe_assert!(size > 0);
        unsafe_assert!(size <= PAGE_SIZE / 2);
        loop {
            let new_bump = unsafe { align_down(self.bump.wrapping_sub(size), layout.align()) };
            let bump_start = unsafe { align_down(self.bump_limit, PAGE_SIZE) as usize };
            if std::hint::unlikely(wrapping_less_than(bump_start, self.bump_limit)) {
                self.claim_frame(common);
                continue;
            }
            self.bump = new_bump;
            unsafe {
                let bump_header = &*(bump_start as *const BumpHeader);
                bump_header.count.fetch_add(1, Relaxed);
                return Some(NonNull::new_unchecked(
                    VirtAddr::new_unsafe(self.bump as u64).as_mut_ptr(),
                ));
            }
        }
    }

    #[inline]
    pub unsafe fn dealloc(&mut self, common: &mut LocalCommon<S, G>, ptr: *mut u8, layout: Layout) {
        let size = layout.size();
        unsafe_assert!(size > 0);
        unsafe_assert!(size <= PAGE_SIZE / 2);
        let counter = align_down(ptr.addr(), PAGE_SIZE) as *const BumpHeader;
        Self::decrement_counter(common, counter);
    }

    #[inline]
    unsafe fn decrement_counter(common: &mut LocalCommon<S, G>, header: *const BumpHeader) {
        let release = (*header).count.fetch_sub(1, Release) == 1;
        if std::hint::unlikely(release) {
            Self::release_frame(common, header);
        }
    }

    unsafe fn release_frame(common: &mut LocalCommon<S, G>, header: *const BumpHeader) {
        (*header).count.load(Acquire);
        unsafe {
            let vaddr = VirtAddr::new_unsafe(header.addr() as u64);
            let paddr = common.global.sys.paddr(vaddr);
            common
                .available_frames
                .push(PhysFrame::from_start_address_unchecked(paddr));
            common
                .available_frames
                .release_extra_to_vec(&common.global.available_frames);
        }
    }

    fn claim_frame(&mut self, common: &mut LocalCommon<S, G>) -> Result<(), ()> {
        if self.bump != 0 {
            unsafe {
                Self::decrement_counter(
                    common,
                    (align_down(self.bump, PAGE_SIZE)) as *const BumpHeader,
                );
            }
        }
        common
            .available_frames
            .steal_from_vec(&common.global.available_frames, 1)?;
        let frame = common.available_frames.pop().ok_or(())?;
        let vaddr = common.global.sys.vaddr(frame.start_address());
        unsafe {
            (*vaddr.as_ptr::<BumpHeader>()).count.store(1, Relaxed);
        }
        self.bump = vaddr.as_u64() as usize + PAGE_SIZE;
        self.bump_limit =
            vaddr.as_u64() as usize + mem::size_of::<BumpHeader>().next_multiple_of(64);
        Ok(())
    }
}
