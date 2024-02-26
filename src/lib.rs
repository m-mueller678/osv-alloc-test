#![allow(clippy::result_unit_err)]
#![allow(clippy::missing_safety_doc)]

use std::alloc::Layout;

pub mod buddymap;
pub mod frame_allocator;

pub mod frame_list;
pub mod myalloc;
pub mod no_frame_allocator;
pub mod page_map;
pub mod paging;
mod static_lib;
pub mod util;

pub unsafe trait TestAlloc: Send {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}

#[cfg(feature = "tikv-jemallocator")]
unsafe impl TestAlloc for tikv_jemallocator::Jemalloc {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        std::alloc::GlobalAlloc::alloc(self, layout)
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        std::alloc::GlobalAlloc::dealloc(self, ptr, layout)
    }
}
