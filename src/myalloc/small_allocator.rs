use crate::{
    myalloc::LocalCommon,
    util::{
        align_down, align_down_const, unsafe_assert, vaddr_unchecked, wrapping_less_than, PAGE_SIZE,
    },
    GlobalData, SystemInterface,
};
use log::trace;
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
use x86_64::structures::paging::PhysFrame;

pub struct SmallAllocator<S: SystemInterface, G: Deref<Target = GlobalData<S>>> {
    bump: usize,
    _p: PhantomData<fn() -> G>,
}

struct BumpFooter {
    count: AtomicUsize,
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>> Drop for SmallAllocator<S, G> {
    fn drop(&mut self) {
        assert!(self.bump == 0);
    }
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>> SmallAllocator<S, G> {
    #[inline]
    pub const fn new() -> Self {
        SmallAllocator {
            bump: 0,
            _p: PhantomData,
        }
    }

    #[inline]
    pub fn deinit(&mut self, common: &mut LocalCommon<S, G>) {
        if self.bump != 0 {
            unsafe {
                Self::decrement_counter(common, find_footer(self.bump));
            }
            self.bump = 0;
        }
    }

    #[inline]
    pub fn alloc(&mut self, common: &mut LocalCommon<S, G>, layout: Layout) -> Option<NonNull<u8>> {
        unsafe_assert!(layout.size() > 0);
        unsafe_assert!(layout.size() <= PAGE_SIZE / 2);
        let mut claimed = false;
        loop {
            let new_bump =
                unsafe { align_down(self.bump.wrapping_sub(layout.size()), layout.align()) };
            let bump_limit = align_down_const::<PAGE_SIZE>(self.bump);
            if std::hint::unlikely(wrapping_less_than(new_bump, bump_limit)) {
                unsafe_assert!(!claimed);
                assert!(layout.align() <= PAGE_SIZE / 2);
                self.claim_frame(common);
                claimed = true;
                continue;
            }
            self.bump = new_bump;
            unsafe {
                (*find_footer(new_bump)).count.fetch_add(1, Relaxed);
                return Some(NonNull::new_unchecked(self.bump as *mut u8));
            }
        }
    }

    #[inline]
    pub unsafe fn dealloc(common: &mut LocalCommon<S, G>, ptr: *mut u8) {
        Self::decrement_counter(common, find_footer(ptr.addr()));
    }

    #[inline]
    unsafe fn decrement_counter(common: &mut LocalCommon<S, G>, footer: *const BumpFooter) {
        let release = (*footer).count.fetch_sub(1, Release) == 1;
        if std::hint::unlikely(release) {
            Self::release_frame(common, footer);
        }
    }

    unsafe fn release_frame(common: &mut LocalCommon<S, G>, footer: *const BumpFooter) {
        (*footer).count.load(Acquire);
        let page = align_down_const::<PAGE_SIZE>(footer.addr());
        let vaddr = unsafe { vaddr_unchecked(page) };
        let paddr = common.global.sys.paddr(vaddr);
        let frame = unsafe { PhysFrame::from_start_address_unchecked(paddr) };
        trace!("releasing frame {frame:?}");
        unsafe { common.available_frames.push(frame).unwrap() };
        common
            .available_frames
            .release_extra_to_vec(&common.global.available_frames);
    }

    fn claim_frame(&mut self, common: &mut LocalCommon<S, G>) -> Option<()> {
        self.deinit(common);
        let frame = common
            .available_frames
            .pop_with_refill(&common.global.available_frames, 1)?;
        trace!("claiming frame {frame:?}");
        let vaddr = common.global.sys.vaddr(frame.start_address());
        let footer = find_footer(vaddr.as_u64() as usize);
        unsafe { (*footer).count.store(1, Relaxed) };
        self.bump = align_down_const::<64>(footer.addr());
        Some(())
    }
}

#[inline]
fn find_footer(addr: usize) -> *const BumpFooter {
    let max_addr = addr | (PAGE_SIZE - 1);
    let address = max_addr - (mem::size_of::<BumpFooter>() - 1);
    address as *const BumpFooter
}
