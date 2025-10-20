#![allow(clippy::result_unit_err)]
#![allow(clippy::missing_safety_doc)]
#![feature(thread_local)]
#![feature(generic_atomic)]
#![feature(sync_unsafe_cell)]
#![feature(new_zeroed_alloc)]
#![feature(cold_path)]
#![feature(allocator_api)]
#![feature(alloc_layout_extra)]

mod buddymap;
mod frame_list;
mod myalloc;
mod page_map;
mod profiling;
mod system_interface;

use std::alloc::Layout;

pub use myalloc::{GlobalData, LocalData};
pub use system_interface::SystemInterface;

#[repr(isize)]
#[allow(clippy::enum_clike_unportable_variant)]
enum LogAllocMessage {
    Dirty = 1_000_000_000_000_000_000,
    PreFlush,
    PostFlush,
    Recycle,
    RecycleBackoff,
}
#[cfg(feature = "log_allocations")]
mod log_allocs;
#[cfg(not(feature = "log_allocations"))]
mod log_allocs {
    pub fn log_alloc(_size: isize) {}
    pub fn flush_alloc_log(_flush_id: u64) {}
}
#[cfg(feature = "local_api_clib")]
mod static_lib;
#[cfg(feature = "global_api_clib")]
mod static_lib_global;

#[cfg(feature = "puffin_profiling")]
use puffin::{profile_function, profile_scope};

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
