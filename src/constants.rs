pub const VIRTUAL_QUANTUM_BITS: u32 = 24;
pub const VIRTUAL_QUANTUM_SIZE: usize = 1 << VIRTUAL_QUANTUM_BITS;

pub const MAX_SMALL_SIZE: usize = 16 << 20;

pub const ADDRESS_BIT_MASK: u64 = (!0u64) >> 16;

pub const PAGE_SIZE_LOG: u32 = 21;
pub const PAGE_SIZE: usize = 1 << PAGE_SIZE_LOG;
