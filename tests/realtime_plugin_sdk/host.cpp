#include <audiograph/plugin.h>
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>
extern "C" int ag_realtime_c11_smoke(const ag_realtime_audio_api*);
namespace {
void require(bool ok,const char* message) { if(!ok) throw std::runtime_error(message); }
template<size_t N> ag_string s(const char (&text)[N]) { return {text,N-1}; }
template<class T> struct guarded { T table; std::array<unsigned char,32> tail; };
template<class T> bool bytes(const T& t,unsigned char b) {
    const auto* p=reinterpret_cast<const unsigned char*>(&t);
    return std::all_of(p,p+sizeof(t),[b](unsigned char x){return x==b;});
}
ag_status AG_CALL description(void* user,ag_string json) { *static_cast<std::string*>(user)=std::string(json.data,static_cast<size_t>(json.size)); return AG_OK; }
ag_status AG_CALL error(void* user,const ag_error* e) { ++*static_cast<unsigned*>(user); return e && e->message.data ? AG_OK : AG_INVALID_ARGUMENT; }
}
int main(int argc,char** argv) {
    HMODULE module=nullptr;
    try {
        require(argc==2,"plugin path required");
        int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,argv[1],-1,nullptr,0); require(n>0,"path conversion");
        std::wstring path(static_cast<size_t>(n),L'\0'); MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,argv[1],-1,path.data(),n);
        module=LoadLibraryExW(path.c_str(),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS); require(module!=nullptr,"LoadLibraryExW");
        auto entry=reinterpret_cast<ag_get_api_fn>(GetProcAddress(module,"ag_plugin_get_api")); require(entry!=nullptr,"entry");
        guarded<ag_plugin_api> base; std::memset(&base,0xa5,sizeof(base));
        require(entry(1,1,sizeof(base),&base.table)==AG_UNSUPPORTED && bytes(base,0xa5),"base version guard");
        require(entry(0,1,sizeof(base.table)-1,&base.table)==AG_INVALID_ARGUMENT && bytes(base,0xa5),"base size guard");
        require(entry(0,1,sizeof(base),&base.table)==AG_OK && bytes(base.tail,0xa5),"base tail guard");
        require(base.table.struct_size==64 && !base.table.reserved && base.table.describe_nodes && base.table.get_node_api,"base layout");
        std::string json; require(base.table.describe_nodes(&json,description)==AG_OK && json.find("AudioStream")!=std::string::npos,"description");
        guarded<ag_realtime_audio_api> rt; std::memset(&rt,0xa5,sizeof(rt));
        auto type=s("org.audiograph.realtime_example.gain_v1"); auto query=base.table.get_node_api;
        require(query(type,s("unknown"),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && bytes(rt,0xa5),"unknown capability guard");
        require(query(type,s(AG_WHOLE_SYNC_ID),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && bytes(rt,0xa5),"whole-sync rejected");
        require(query(s("unknown"),s(AG_REALTIME_AUDIO_ID),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && bytes(rt,0xa5),"type guard");
        require(query(type,s(AG_REALTIME_AUDIO_ID),2,sizeof(rt),&rt.table)==AG_UNSUPPORTED && bytes(rt,0xa5),"capability version guard");
        require(query(type,s(AG_REALTIME_AUDIO_ID),1,sizeof(rt.table)-1,&rt.table)==AG_INVALID_ARGUMENT && bytes(rt,0xa5),"table size guard");
        require(query(type,s(AG_REALTIME_AUDIO_ID),1,sizeof(rt),&rt.table)==AG_OK && bytes(rt.tail,0xa5),"realtime tail guard");
        auto api=rt.table; require(api.struct_size==40 && api.version==1 && api.create && api.prepare && api.process && api.destroy,"realtime layout");
        require(ag_realtime_c11_smoke(&api)==0,"C11 smoke");
        ag_named_value parameter{}; parameter.id=s("gain_db"); parameter.value.type=AG_NUMBER;
        ag_create_info info{sizeof(info),0,&parameter,1,0,nullptr};
        unsigned reports=0; ag_error_sink sink{sizeof(sink),0,&reports,error};
        for(double db : {-25.0,13.0,std::numeric_limits<double>::infinity(),std::numeric_limits<double>::quiet_NaN()}) {
            parameter.value.data.number=db; ag_instance* failed=reinterpret_cast<ag_instance*>(1);
            require(api.create(&info,&sink,&failed)==AG_INVALID_ARGUMENT && !failed,"bad gain create");
        }
        require(reports==4,"control errors reported");
        parameter.id=s("unknown"); ag_instance* failed=reinterpret_cast<ag_instance*>(1);
        require(api.create(&info,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"unknown parameter"); parameter.id=s("gain_db");
        parameter.value.data.number=-6.020599913279624;
        ag_instance* instance=nullptr; require(api.create(&info,nullptr,&instance)==AG_OK && instance,"create");
        float single=1; ag_realtime_block block{sizeof(block),0,1,1,&single};
        require(api.process(instance,&block)==AG_RT_NOT_PREPARED,"not prepared");
        ag_realtime_prepare_info prep{sizeof(prep),0,44100,1,65536,0};
        require(api.prepare(instance,&prep,&sink)==AG_INVALID_ARGUMENT,"bad format");
        require(api.process(instance,&block)==AG_RT_NOT_PREPARED,"failed prepare state");
        prep.sample_rate=48000; prep.maximum_frames=0; require(api.prepare(instance,&prep,nullptr)==AG_INVALID_ARGUMENT,"zero capacity");
        prep.maximum_frames=65537; require(api.prepare(instance,&prep,nullptr)==AG_INVALID_ARGUMENT,"capacity limit");
        prep.maximum_frames=65536; require(api.prepare(instance,&prep,nullptr)==AG_OK,"prepare");
        require(api.prepare(instance,&prep,nullptr)==AG_INVALID_ARGUMENT,"session immutable");
        std::vector<float> samples(65536);
        for(unsigned repeat=0;repeat<3;++repeat) for(uint64_t frames : {1ull,17ull,512ull,65536ull}) {
            std::fill(samples.begin(),samples.end(),0.75f); block={sizeof(block),0,frames,frames,samples.data()};
            require(api.process(instance,&block)==AG_RT_OK,"variable process");
            require(block.frame_count==frames && block.sample_count==frames && block.samples==samples.data(),"block shape");
            for(uint64_t i=0;i<frames;++i) require(std::fabs(samples[static_cast<size_t>(i)]-0.375f)<1e-6f,"gain result");
        }
        std::fill(samples.begin(),samples.end(),0); require(api.process(instance,&block)==AG_RT_OK && samples[0]==0,"silence");
        block.frame_count=block.sample_count=0; require(api.process(instance,&block)==AG_RT_INVALID_BLOCK,"zero block");
        block.frame_count=block.sample_count=65537; require(api.process(instance,&block)==AG_RT_INVALID_BLOCK,"oversize block");
        block.frame_count=1; block.sample_count=2; require(api.process(instance,&block)==AG_RT_INVALID_BLOCK,"count mismatch"); block.sample_count=1;
        for(float bad : {std::numeric_limits<float>::infinity(),std::numeric_limits<float>::quiet_NaN()}) {
            samples[0]=bad; require(api.process(instance,&block)==AG_RT_NON_FINITE_INPUT,"nonfinite input");
        }
        api.destroy(instance); parameter.value.data.number=12; require(api.create(&info,nullptr,&instance)==AG_OK,"new session");
        require(api.process(instance,&block)==AG_RT_NOT_PREPARED,"new state fresh");
        require(api.prepare(instance,&prep,nullptr)==AG_OK,"new prepare"); samples[0]=std::numeric_limits<float>::max();
        require(api.process(instance,&block)==AG_RT_NON_FINITE_OUTPUT,"finite overflow"); api.destroy(instance);
        FreeLibrary(module); std::cout<<"Realtime SDK contracts passed\n"; return 0;
    } catch(const std::exception& e) { if(module) FreeLibrary(module); std::cerr<<e.what()<<'\n'; return 1; }
}
