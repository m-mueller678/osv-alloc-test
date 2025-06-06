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
pub mod profiling;
#[repr(isize)]
#[allow(clippy::enum_clike_unportable_variant)]
pub enum LogAllocMessage {
    Dirty = 1_000_000_000_000_000_000,
    PreFlush,
    PostFlush,
    Recycle,
    RecycleBackoff,
}
#[cfg(feature = "log_allocations")]
pub mod log_allocs;
#[cfg(not(feature = "log_allocations"))]
pub mod log_allocs {
    pub fn log_alloc(_size: isize) {}
    pub fn flush_alloc_log(_flush_id: u64) {}
}
#[cfg(feature = "local_api_clib")]
mod static_lib;
#[cfg(feature = "global_api_clib")]
mod static_lib_global;
pub mod util;

#[cfg(feature = "puffin_profiling")]
pub use puffin::{profile_function, profile_scope};

#[cfg(not(feature = "puffin_profiling"))]
#[macro_export]
macro_rules! profile_function {
    () => {};
    ($data:expr) => {};
}

#[cfg(not(feature = "puffin_profiling"))]
#[macro_export]
macro_rules! profile_scope {
    () => {};
    ($data:expr) => {};
}

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
