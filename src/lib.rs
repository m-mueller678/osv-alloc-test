#![allow(clippy::result_unit_err)]
#![allow(clippy::missing_safety_doc)]
#![feature(thread_local)]
#![feature(generic_atomic)]
#![feature(sync_unsafe_cell)]
#![feature(cold_path)]
#![feature(allocator_api)]
#![feature(alloc_layout_extra)]
#![feature(likely_unlikely)]
#![feature(unsafe_cell_access)]

mod frame_list;
mod myalloc;
mod quantum_address;
mod system_interface;
mod util;

#[cfg(feature = "global_api_clib")]
mod static_lib_global;

use std::{alloc::Layout, ptr::NonNull};

pub use myalloc::{GlobalData, LocalData};
pub use system_interface::SystemInterface;

pub unsafe trait TestAlloc: Send {
    unsafe fn alloc(&mut self, layout: Layout) -> Option<NonNull<u8>>;
    unsafe fn dealloc(&mut self, ptr: NonNull<u8>, size: usize);
}
