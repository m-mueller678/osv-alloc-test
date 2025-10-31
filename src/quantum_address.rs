use std::num::NonZeroUsize;

use x86_64::VirtAddr;

use crate::util::{align_down_const, VIRTUAL_QUANTUM_SIZE};

/// The starting address of a virtual quantum
#[derive(Clone, Copy)]
pub struct QuantumAddress(NonZeroUsize);

impl QuantumAddress {
    pub fn start(self) -> usize {
        self.0.get()
    }

    pub fn from_start(addr: usize) -> Self {
        if cfg!(debug_assertions) {
            VirtAddr::new(addr as u64);
            VirtAddr::new((addr + VIRTUAL_QUANTUM_SIZE) as u64);
            assert!(addr.is_multiple_of(VIRTUAL_QUANTUM_SIZE));
        }
        QuantumAddress(NonZeroUsize::new(addr).unwrap())
    }

    pub fn containing(addr: usize) -> Self {
        Self::from_start(align_down_const::<VIRTUAL_QUANTUM_SIZE>(addr))
    }
}
