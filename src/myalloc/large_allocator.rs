use std::{alloc::Layout, num::NonZeroUsize, ops::Deref, ptr::NonNull};

use crate::{
    frame_list::FrameList2M,
    myalloc::LocalCommon,
    quantum_address::QuantumAddress,
    util::{page_from_addr, unsafe_assert, vaddr_unchecked, PAGE_SIZE, VIRTUAL_QUANTUM_BITS},
    GlobalData, SystemInterface,
};

#[inline]
fn large_alloc_level(size: usize) -> u32 {
    size.next_power_of_two()
        .trailing_zeros()
        .saturating_sub(VIRTUAL_QUANTUM_BITS)
}

#[inline]
pub fn alloc_large<S: SystemInterface, G: Deref<Target = GlobalData<S>>>(
    common: &mut LocalCommon<S, G>,
    layout: Layout,
) -> Option<NonNull<u8>> {
    assert!(layout.align() <= PAGE_SIZE);
    let level = large_alloc_level(layout.size());
    let quantum = common
        .global
        .quantum_storage
        .alloc(level, &mut common.rng)?;
    let start = quantum.start();
    let end = start + layout.size().next_multiple_of(PAGE_SIZE);
    let mut to_map = start;
    unsafe_assert!(to_map < end);
    while to_map < end {
        let remaining_frames = (end - to_map) / PAGE_SIZE;
        let Some(frame) = common.available_frames.pop_with_refill(
            &common.global.available_frames,
            remaining_frames.min(FrameList2M::<S>::CAPACITY),
        ) else {
            std::hint::cold_path();
            while to_map > start {
                to_map -= PAGE_SIZE;
                unsafe {
                    let page = page_from_addr(vaddr_unchecked(to_map));
                    let frame = common.global.sys.unmap(page);
                    common
                        .available_frames
                        .push_with_spill(frame, &common.global.available_frames);
                }
            }
            common
                .available_frames
                .release_extra_to_vec(&common.global.available_frames);
            common.global.quantum_storage.dealloc_clean(level, quantum);
            return None;
        };
        let page = unsafe { page_from_addr(vaddr_unchecked(to_map)) };
        unsafe { common.global.sys.map(page, frame) };
        to_map += PAGE_SIZE;
    }
    unsafe {
        Some(NonNull::with_exposed_provenance(
            NonZeroUsize::new_unchecked(start),
        ))
    }
}

#[inline]
pub fn dealloc_large<S: SystemInterface, G: Deref<Target = GlobalData<S>>>(
    common: &mut LocalCommon<S, G>,
    ptr: *mut u8,
    size: usize,
) {
    let level = large_alloc_level(size);
    let mut to_unmap = ptr.addr();
    let end = (to_unmap + size).next_multiple_of(PAGE_SIZE);
    unsafe_assert!(to_unmap < end);
    while to_unmap < end {
        unsafe {
            let page = page_from_addr(vaddr_unchecked(to_unmap));
            let frame = common.global.sys.unmap(page);
            common
                .available_frames
                .push_with_spill(frame, &common.global.available_frames);
        }
        to_unmap += PAGE_SIZE;
    }
    common
        .available_frames
        .release_extra_to_vec(&common.global.available_frames);
    common
        .global
        .quantum_storage
        .dealloc_dirty(level, QuantumAddress::from_start(ptr.addr()));
}
