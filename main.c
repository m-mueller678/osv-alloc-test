#include "virtual_alloc_local.h"

int main(){
    virtual_alloc_init_global(1<<30,((uint64_t)(1))<<40);
    VirtualAllocHandle allocator;
    virtual_alloc_init_handle(&allocator,0);
    for(int i=0;i<(1<<30);++i){
        char* array = virtual_alloc_alloc(&allocator,128,8);
        for(int j=0;j<128;++j){
            array[j]=42;
        }
        virtual_alloc_free(&allocator,128,8,array);
    }

}