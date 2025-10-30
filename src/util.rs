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

use x86_64::{
    structures::paging::{Page, Size2MiB},
    VirtAddr,
};

pub const VIRTUAL_QUANTUM_BITS: u32 = 24;
pub const VIRTUAL_QUANTUM_SIZE: usize = 1 << VIRTUAL_QUANTUM_BITS;
pub const MAX_SMALL_SIZE: usize = 16 << 20;
pub const ADDRESS_BIT_MASK: usize = usize::MAX >> 16;
pub const PAGE_SIZE_LOG: u32 = 21;
pub const PAGE_SIZE: usize = 1 << PAGE_SIZE_LOG;

pub unsafe fn page_from_addr(addr: VirtAddr) -> Page<Size2MiB> {
    if cfg!(debug_assertions) {
        Page::from_start_address(addr).unwrap()
    } else {
        unsafe { Page::from_start_address_unchecked(addr) }
    }
}

pub unsafe fn vaddr_unchecked(addr: usize) -> VirtAddr {
    if cfg!(debug_assertions) {
        VirtAddr::new(addr as u64)
    } else {
        VirtAddr::new_unsafe(addr as u64)
    }
}

/// the starting address of a virtaul quantum, masked to lower half of addresses and shifted by [VIRTUAL_QUANTUM_BITS]
pub struct QuantumShifted(pub usize);

pub fn address_to_quantum(a: VirtAddr) -> QuantumShifted {
    QuantumShifted((a.as_u64() as usize & ADDRESS_BIT_MASK) >> VIRTUAL_QUANTUM_BITS)
}

#[inline(always)]
pub fn wrapping_less_than(a: usize, b: usize) -> bool {
    (a.wrapping_sub(b) as isize) < 0
}

/// # Safety
/// b must be a power of two.
#[inline(always)]
pub unsafe fn align_down(a: usize, b: usize) -> usize {
    unsafe_assert!(b.is_power_of_two());
    a & (b - 1)
}

#[inline(always)]
pub fn align_down_const<const ALIGN: usize>(a: usize) -> usize {
    let mask = const {
        assert!(ALIGN.is_power_of_two());
        ALIGN - 1
    };
    a & ALIGN
}
