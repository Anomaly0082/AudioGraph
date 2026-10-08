// Compile the exact sample implementation into a test executable so all its
// C++ allocation/deallocation operators are observed without altering the DLL.
#include <audiograph/plugin.h>
#include <cstdlib>
#include <new>
static bool watch=false;
static unsigned allocations=0, deallocations=0;
void* operator new(std::size_t size) {
    if(watch) ++allocations;
    if(void* p=std::malloc(size ? size : 1)) return p;
    throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* p) noexcept { if(watch) ++deallocations; std::free(p); }
void operator delete[](void* p) noexcept { ::operator delete(p); }
void operator delete(void* p,std::size_t) noexcept { ::operator delete(p); }
void operator delete[](void* p,std::size_t) noexcept { ::operator delete(p); }
int main() {
    ag_plugin_api plugin{};
    if(ag_plugin_get_api(0,1,sizeof(plugin),&plugin)!=AG_OK) return 1;
    constexpr char type[]="org.audiograph.realtime_example.gain_v1";
    constexpr char cap[]=AG_REALTIME_AUDIO_ID;
    ag_realtime_audio_api api{};
    if(plugin.get_node_api({type,sizeof(type)-1},{cap,sizeof(cap)-1},1,sizeof(api),&api)!=AG_OK) return 2;
    ag_create_info info{}; info.struct_size=sizeof(info);
    ag_instance* instance=nullptr;
    if(api.create(&info,nullptr,&instance)!=AG_OK) return 3;
    ag_realtime_prepare_info prepare{sizeof(prepare),0,48000,1,1024,0};
    if(api.prepare(instance,&prepare,nullptr)!=AG_OK) { api.destroy(instance); return 4; }
    float samples[1024]{};
    ag_realtime_block block{sizeof(block),0,1024,1024,samples};
    bool ok=true; watch=true;
    for(unsigned i=0;i<1000;++i) if(api.process(instance,&block)!=AG_RT_OK) ok=false;
    watch=false;
    api.destroy(instance);
    return ok && !allocations && !deallocations ? 0 : 5;
}
