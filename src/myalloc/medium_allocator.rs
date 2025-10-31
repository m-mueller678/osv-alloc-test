use crate::{
    frame_list::FrameList2M,
    myalloc::LocalCommon,
    quantum_address::QuantumAddress,
    util::{
        align_down, align_down_const, page_from_addr, unsafe_assert, vaddr_unchecked,
        wrapping_less_than, PAGE_SIZE, VIRTUAL_QUANTUM_SIZE,
    },
    GlobalData, SystemInterface,
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
use x86_64::{structures::paging::Page, VirtAddr};

pub struct MediumAllocator<S: SystemInterface, G: Deref<Target = GlobalData<S>>> {
    bump: usize,
    _p: PhantomData<fn() -> G>,
}

struct BumpFooter {
    /// the page of the byte pointed to by bump has one count extra for the allocator.
    /// pages below that page in the bump region are set to 1.
    counts: [AtomicUsize; PAGES_PER_QUANTUM],
    page_count: AtomicUsize,
}

const PAGES_PER_QUANTUM: usize = VIRTUAL_QUANTUM_SIZE / PAGE_SIZE;

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>> Drop for MediumAllocator<S, G> {
    fn drop(&mut self) {
        assert!(self.bump == 0);
    }
}

impl<S: SystemInterface, G: Deref<Target = GlobalData<S>>> MediumAllocator<S, G> {
    #[inline]
    pub const fn new() -> Self {
        MediumAllocator {
            bump: 0,
            _p: PhantomData,
        }
    }

    #[inline]
    pub fn deinit(&mut self, common: &mut LocalCommon<S, G>) {
        if std::hint::likely(self.bump != 0) {
            unsafe { Self::decrement_page_counter(common, self.bump) };
            self.bump = 0;
        }
    }

    /// # Safety
    /// layout size must be in range 1..=VIRTUAL_QUANTUM_SIZE/2
    #[inline]
    pub unsafe fn alloc(
        &mut self,
        common: &mut LocalCommon<S, G>,
        layout: Layout,
    ) -> Option<NonNull<u8>> {
        unsafe_assert!(layout.size() > 0);
        unsafe_assert!(layout.size() <= VIRTUAL_QUANTUM_SIZE / 2);
        loop {
            let new_bump =
                unsafe { align_down(self.bump.wrapping_sub(layout.size()), layout.align()) };
            let mut page_limit = align_down_const::<PAGE_SIZE>(self.bump);
            if wrapping_less_than(new_bump, page_limit) {
                let bump_limit = align_down_const::<VIRTUAL_QUANTUM_SIZE>(self.bump);
                assert!(layout.align() <= PAGE_SIZE);
                if std::hint::unlikely(wrapping_less_than(new_bump, bump_limit)) {
                    self.claim_quantum(common)?;
                    continue;
                }
                let allocation_end = self.bump + layout.size();
                if std::hint::unlikely(allocation_end <= page_limit) {
                    unsafe {
                        Self::decrement_page_counter(common, self.bump);
                    }
                }
                let new_page_limit = align_down_const::<PAGE_SIZE>(new_bump);
                unsafe_assert!(new_page_limit < page_limit);
                let missing_pages = (page_limit - new_page_limit) / PAGE_SIZE;
                common
                    .available_frames
                    .steal_from_vec(&common.global.available_frames, missing_pages)?;
                while page_limit > new_page_limit {
                    page_limit -= PAGE_SIZE;
                    unsafe_assert!(page_limit.is_multiple_of(PAGE_SIZE));
                    unsafe {
                        common.global.sys.map(
                            Page::from_start_address_unchecked(VirtAddr::new_unsafe(
                                page_limit as u64,
                            )),
                            common.available_frames.pop().unwrap(),
                        );
                    }
                }
            }
            self.bump = new_bump;
            let page_index = page_limit / PAGE_SIZE % PAGES_PER_QUANTUM;
            unsafe {
                (*find_footer(new_bump)).counts[page_index].fetch_add(1, Relaxed);
                return Some(NonNull::new_unchecked(
                    VirtAddr::new_unsafe(self.bump as u64).as_mut_ptr(),
                ));
            }
        }
    }

    /// # Safety
    /// ptr must be allocated with size
    #[inline]
    pub unsafe fn dealloc(common: &mut LocalCommon<S, G>, ptr: *mut u8, size: usize) {
        let mut addr = ptr.addr();
        let end = addr + size;
        unsafe_assert!(addr < end);
        while addr < end {
            Self::decrement_page_counter(common, addr);
            addr += PAGE_SIZE
        }
    }

    #[inline]
    unsafe fn decrement_page_counter(common: &mut LocalCommon<S, G>, address_in_page: usize) {
        let footer = find_footer(address_in_page);
        let page_index = address_in_page / PAGE_SIZE % PAGES_PER_QUANTUM;
        if unsafe { &*footer }.counts[page_index].fetch_sub(1, Release) == 1 {
            Self::on_page_counter_zero(common, address_in_page);
        }
    }

    unsafe fn on_page_counter_zero(common: &mut LocalCommon<S, G>, address_in_page: usize) {
        let footer = find_footer(address_in_page);
        let page_index = address_in_page / PAGE_SIZE % PAGES_PER_QUANTUM;
        let dealloc_quantum = unsafe {
            (*footer).counts[page_index].load(Acquire);
            (*footer).page_count.fetch_sub(1, Acquire) == 1
        };
        if page_index < PAGES_PER_QUANTUM - 1 {
            Self::dealloc_page(common, address_in_page);
        }
        if dealloc_quantum {
            Self::dealloc_page(common, address_in_page | (VIRTUAL_QUANTUM_SIZE - 1));
            common
                .global
                .quantum_storage
                .dealloc_dirty(0, QuantumAddress::containing(address_in_page));
        }
    }

    unsafe fn dealloc_page(common: &mut LocalCommon<S, G>, address_in_page: usize) {
        let page = align_down_const::<PAGE_SIZE>(address_in_page);
        let page = unsafe { page_from_addr(vaddr_unchecked(page)) };
        let frame = unsafe { common.global.sys.unmap(page) };
        common.available_frames.push(frame).unwrap();
        common
            .available_frames
            .release_extra_to_vec(&common.global.available_frames);
    }

    fn claim_quantum(&mut self, common: &mut LocalCommon<S, G>) -> Option<()> {
        self.deinit(common);
        let quantum = common.global.quantum_storage.alloc(0, &mut common.rng)?;
        let last_page = quantum.start() + (PAGES_PER_QUANTUM - 1) * PAGE_SIZE;
        let last_page = unsafe { page_from_addr(vaddr_unchecked(last_page)) };
        let Some(frame) = common.available_frames.pop_with_refill(
            &common.global.available_frames,
            FrameList2M::<S>::DEFAULT_REFILL_SIZE,
        ) else {
            common.global.quantum_storage.dealloc_clean(0, quantum);
            return None;
        };
        unsafe { common.global.sys.map(last_page, frame) };
        let footer = unsafe { &*find_footer(last_page.start_address().as_u64() as usize) };
        for c in &footer.counts {
            c.store(1, Relaxed);
        }
        footer.page_count.store(footer.counts.len(), Relaxed);
        self.bump = align_down_const::<64>((footer as *const BumpFooter).addr());
        Some(())
    }
}

#[inline]
fn find_footer(addr: usize) -> *const BumpFooter {
    let max_addr = addr | (VIRTUAL_QUANTUM_SIZE - 1);
    let address = max_addr - (mem::size_of::<BumpFooter>()) - 1;
    address as *const BumpFooter
}
