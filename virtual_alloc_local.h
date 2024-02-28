#pragma once
#include <stdbool.h>
#include <stdint.h>

typedef struct{
    uint64_t _private[11];
} VirtualAllocHandle;

// initializes the allocator.
// call this once before calling any of the other functions.
void virtual_alloc_init_global(uint64_t physical_size, uint64_t virtual_size);

// Creates a handle to the allocator.
// A handle is bound to the thread it was created on and must not be accessed from other threads.
// It is safe to move this handle around via `memcpy`.
// Currently, destruction of handles is not implemented.
// Leaking it leaks up to 2MiB of physical memory and 16MiB of virtual address space.
bool virtual_alloc_init_handle(VirtualAllocHandle *dst, uint64_t seed);

// allocate `size` bytes of memory, aligned to `align` bytes.
void *virtual_alloc_alloc(VirtualAllocHandle *local, uint64_t size, uint64_t align);

// deallocate memory using a handle.
// The size and alignment must exactly match the values passed during allocation.
// It is safe to deallocate memory using a different handle than was used for the allocation.
void virtual_alloc_free(VirtualAllocHandle *local, uint64_t size, uint64_t align, void *ptr);
