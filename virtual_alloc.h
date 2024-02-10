#include <stdbool.h>
#include <stdint.h>

typedef struct{
    uint64_t _private[11];
} VirtualAllocHandle;

void virtual_alloc_init_global(uint64_t physical_size, uint64_t virtual_size);

bool virtual_alloc_init_handle(VirtualAllocHandle *dst, uint64_t seed);

void *virtual_alloc_alloc(VirtualAllocHandle *local, uint64_t size, uint64_t align);

void virtual_alloc_free(VirtualAllocHandle *local, uint64_t size, uint64_t align, void *ptr);
