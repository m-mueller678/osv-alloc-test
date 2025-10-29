#![allow(clippy::result_unit_err)]
#![allow(clippy::missing_safety_doc)]
#![feature(thread_local)]
#![feature(generic_atomic)]
#![feature(sync_unsafe_cell)]
#![feature(cold_path)]
#![feature(allocator_api)]
#![feature(alloc_layout_extra)]
#![feature(likely_unlikely)]

mod buddymap;
mod frame_list;
mod myalloc;
mod page_map;
mod system_interface;

use std::alloc::Layout;

pub use myalloc::{GlobalData, LocalData};
pub use system_interface::SystemInterface;

pub unsafe trait TestAlloc: Send {
    unsafe fn alloc(&mut self, layout: Layout) -> *mut u8;
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout);
}

macro_rules! unsafe_assert {
    ($x:expr) => {
        if cfg!(debug_assertions) {
            assert!($x);
        } else {
            unsafe {
                std::hint::assert_unchecked($x);
            }
        }
    };
}
pub(crate) use unsafe_assert;
