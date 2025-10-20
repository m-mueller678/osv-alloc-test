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
mod system_interface;

use std::alloc::Layout;

pub use myalloc::{GlobalData, LocalData};
pub use system_interface::SystemInterface;

#[cfg(feature = "local_api_clib")]
mod static_lib;
#[cfg(feature = "global_api_clib")]
mod static_lib_global;

pub unsafe trait TestAlloc: Send {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}
