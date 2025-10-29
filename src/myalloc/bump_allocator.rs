use crate::{
    myalloc::{align_down, wrapping_less_than, LocalCommon, VIRTUAL_QUANTUM_BITS},
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
        Ordering::{Acquire, Release},
    },
};
use x86_64::{
    structures::paging::{Page, PageSize, Size2MiB},
    VirtAddr,
};

struct BumpAllocator<S: SystemInterface, G: Deref<Target = GlobalData<S>>, const SIZE_LOG: u32> {
    bump: usize,
    _p: PhantomData<fn() -> G>,
}

struct BumpHeader {
    count: AtomicUsize,
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>, const SIZE_LOG: u32>
    BumpAllocator<S, G, SIZE_LOG>
{
    const LIMIT: usize = 64;
    const SIZE: usize = 1 << SIZE_LOG;
    const PAGE_SIZE_LOG: u32 = Size2MiB::SIZE.trailing_zeros();
    const QUANTUM_COUNT_LOG: u32 = SIZE_LOG.saturating_sub(VIRTUAL_QUANTUM_BITS);
    const PAGE_COUNT_LOG: u32 = SIZE_LOG.checked_sub(Self::PAGE_SIZE_LOG).unwrap();

    fn alloc(&mut self, common: &mut LocalCommon<S, G>, layout: Layout) -> Option<NonNull<u8>> {
        let size = layout.size();
        unsafe_assert!(size > 0);
        unsafe_assert!(size <= Self::SIZE / 2);
        loop {
            let new_bump = unsafe { align_down(self.bump.wrapping_sub(size), layout.align()) };
            let bump_start = unsafe { align_down(self.bump, Self::SIZE) as usize };
            let min_addr = bump_start + Self::LIMIT;
            if std::hint::unlikely(wrapping_less_than(bump_start, min_addr)) {
                self.renew_bump();
                continue;
            }
            self.bump = new_bump;
            return unsafe {
                Some(NonNull::new_unchecked(
                    VirtAddr::new_unsafe(self.bump as u64).as_mut_ptr(),
                ))
            };
        }
    }

    unsafe fn decrement_counter(common: &mut LocalCommon<S, G>, header: *const BumpHeader) {
        {
            let header = &*header;
            if std::hint::likely(header.count.fetch_sub(1, Release) != 1) {
                return;
            }
            header.count.load(Acquire);
        };
        for page in (header.addr()..)
            .step_by(1 << Self::PAGE_SIZE_LOG)
            .take(1 << Self::PAGE_COUNT_LOG)
        {
            unsafe {
                let page = Page::from_start_address_unchecked(VirtAddr::new_unsafe(page as u64));
                let frame = common.global.sys.unmap(page);
                common.available_frames.push(frame);
            }
        }
    }

    fn renew_bump(&mut self) {}

    fn renew_bump(&mut self, common: &mut LocalCommon<S, G>) -> Result<(), ()> {
        let q = common
            .global
            .quantum_storage
            .alloc(Self::QUANTUM_COUNT_LOG, &mut common.rng)
            .ok_or(())?;
        if self.bump != 0 {
            common.global.decrement_quantum(q);
            unsafe {
                Self::decrement_counter(
                    common,
                    (align_down(self.bump, 1 << Self::SIZE)) as *const BumpHeader,
                );
            }
        }
        common
            .available_frames
            .steal_from_vec(&common.global.available_frames, 1 << Self::PAGE_COUNT_LOG)?;

        self.min_address = VirtAddr::new((q as u64) << VIRTUAL_QUANTUM_BITS).as_u64();
        self.bump = VirtAddr::new((q as u64 + 1) << VIRTUAL_QUANTUM_BITS).as_u64();
        debug_assert!(self.min_address | ADDRESS_BIT_MASK == self.bump | ADDRESS_BIT_MASK);
        self.current_quantum_index = self.global.pages_per_quantum.insert(q, 0, 1);
        self.current_page = Page::from_start_address(VirtAddr::new(self.bump)).unwrap() - 1;
        self.current_page_index =
            self.global
                .map_and_insert(self.current_page, self.available_frames.pop().unwrap(), 1);
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
fn vaddr_from_ref<T>(x: &T) {
    assert!(mem::size_of::<T>() > 0);
    unsafe { VirtAddr::new_unsafe((x as *const T).addr() as u64) }
}
