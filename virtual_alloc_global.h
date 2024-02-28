#pragma once
#include <stdbool.h>
#include <stdint.h>

// initializes the allocator.
// call this once before calling any of the other functions.
void global_virtual_alloc_init(uint64_t physical_size, uint64_t virtual_size);

// allocate `size` bytes of memory, aligned to `align` bytes.
void * global_virtual_alloc_alloc(uint64_t size, uint64_t align);

// deallocate memory.
// The size and alignment must exactly match the values passed during allocation.
void global_virtual_alloc_free(uint64_t size, uint64_t align, void *ptr);
