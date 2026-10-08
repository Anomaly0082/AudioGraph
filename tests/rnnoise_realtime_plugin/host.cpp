#include <audiograph/plugin.h>
#include <rnnoise.h>
#define NOMINMAX
#include <windows.h>
#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>
namespace {
void require(bool x,const char* message) { if(!x) throw std::runtime_error(message); }
template<size_t N> ag_string s(const char (&text)[N]) { return {text,N-1}; }
template<class T> struct Guard { T table; std::array<unsigned char,32> tail; };
template<class T> bool untouched(const T& value) {
 const auto* p=reinterpret_cast<const unsigned char*>(&value);
 return std::all_of(p,p+sizeof(value),[](unsigned char c){return c==0xa5;});
}
ag_status AG_CALL description(void* user,ag_string json) { *static_cast<std::string*>(user)=std::string(json.data,static_cast<size_t>(json.size)); return AG_OK; }
ag_realtime_prepare_info preparation() { return {sizeof(ag_realtime_prepare_info),0,48000,1,65536,0}; }
struct Session {
 ag_realtime_audio_api api; ag_instance* instance=nullptr;
 explicit Session(ag_realtime_audio_api a,bool prepared=true):api(a) {
  ag_create_info info{sizeof(info),0,nullptr,0,0,nullptr};
  require(api.create(&info,nullptr,&instance)==AG_OK && instance,"create");
  if(prepared) { auto p=preparation(); require(api.prepare(instance,&p,nullptr)==AG_OK,"prepare"); }
 }
 ~Session(){if(instance)api.destroy(instance);}
 Session(const Session&)=delete; Session& operator=(const Session&)=delete;
 ag_realtime_status process(float* data,size_t n) {
  ag_realtime_block b{sizeof(b),0,n,n,data}; auto status=api.process(instance,&b);
  require(b.frame_count==n && b.sample_count==n && b.samples==data,"shape preserved"); return status;
 }
};
// Oracle uses public RNNoise only, independently frames and delays the result.
// It never pads or flushes an incomplete frame at session destruction.
std::vector<float> reference(const std::vector<float>& input) {
 require(rnnoise_get_frame_size()==480,"frame size");
 auto* state=static_cast<DenoiseState*>(std::malloc(static_cast<size_t>(rnnoise_get_size())));
 require(state!=nullptr,"reference allocation");
 if(rnnoise_init(state,nullptr)!=0){std::free(state);throw std::runtime_error("reference init");}
 std::vector<float> result(input.size(),0); std::array<float,480> in{},out{};
 for(size_t offset=0;offset+480<=input.size();offset+=480) {
  for(size_t i=0;i<480;++i) in[i]=input[offset+i]*32768.f;
  rnnoise_process_frame(state,out.data(),in.data());
  for(size_t i=0;i<480 && offset+480+i<result.size();++i) result[offset+480+i]=out[i]/32768.f;
 }
 std::free(state);return result;
}
std::vector<float> run(Session& session,const std::vector<float>& input,const std::vector<size_t>& chunks) {
 auto result=input;size_t offset=0,index=0;
 while(offset<result.size()) {size_t n=std::min(chunks[index++%chunks.size()],result.size()-offset);
  require(session.process(result.data()+offset,n)==AG_RT_OK,"process");offset+=n;}
 return result;
}
void equal(const std::vector<float>& a,const std::vector<float>& b) {
 require(a.size()==b.size(),"length");for(size_t i=0;i<a.size();++i)
  require(std::isfinite(a[i]) && std::fabs(a[i]-b[i])<=2e-6f,"reference mismatch");
}
void invalid(ag_realtime_audio_api api) {
 ag_named_value parameter{};parameter.id=s("unknown");parameter.value.type=AG_NUMBER;
 ag_create_info info{sizeof(info),0,&parameter,1,0,nullptr};ag_instance* failed=reinterpret_cast<ag_instance*>(1);
 require(api.create(&info,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"parameters rejected");
 info.parameters=nullptr;info.parameter_count=0;info.reserved=1;failed=reinterpret_cast<ag_instance*>(1);
 require(api.create(&info,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"reserved create");
 require(api.create(nullptr,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"null create");
 info.reserved=0;info.struct_size=sizeof(info)-1;failed=reinterpret_cast<ag_instance*>(1);
 require(api.create(&info,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"short create");
 info.struct_size=sizeof(info);info.reserved2=1;failed=reinterpret_cast<ag_instance*>(1);
 require(api.create(&info,nullptr,&failed)==AG_INVALID_ARGUMENT && !failed,"reserved2 create");
 info.reserved2=0;require(api.create(&info,nullptr,nullptr)==AG_INVALID_ARGUMENT,"null instance output");
 Session fresh(api,false);float sample=0;
 require(fresh.process(&sample,1)==AG_RT_NOT_PREPARED,"unprepared");
 for(unsigned k=0;k<7;++k) {auto p=preparation();
  switch(k){case 0:p.sample_rate=44100;break;case 1:p.channel_count=2;break;case 2:p.maximum_frames=0;break;
   case 3:p.maximum_frames=65537;break;case 4:p.reserved=1;break;case 5:p.reserved2=1;break;default:p.struct_size=sizeof(p)-1;}
  require(api.prepare(fresh.instance,&p,nullptr)==AG_INVALID_ARGUMENT,"bad prepare");
  require(fresh.process(&sample,1)==AG_RT_NOT_PREPARED,"failed prepare state");
 }
 require(api.prepare(fresh.instance,nullptr,nullptr)==AG_INVALID_ARGUMENT,"null prepare");
 auto p=preparation();require(api.prepare(fresh.instance,&p,nullptr)==AG_OK,"prepare after errors");
 require(api.prepare(fresh.instance,&p,nullptr)==AG_INVALID_ARGUMENT,"repeat prepare");
 for(unsigned k=0;k<6;++k) {ag_realtime_block b{sizeof(b),0,1,1,&sample};
  switch(k){case 0:b.frame_count=b.sample_count=0;break;case 1:b.frame_count=b.sample_count=65537;break;
   case 2:b.sample_count=2;break;case 3:b.samples=nullptr;break;case 4:b.reserved=1;break;default:b.struct_size=sizeof(b)-1;}
  require(api.process(fresh.instance,&b)==AG_RT_INVALID_BLOCK,"bad block");
 }
 require(api.process(fresh.instance,nullptr)==AG_RT_INVALID_BLOCK,"null block");
 for(float bad:{1.01f,-1.01f,std::numeric_limits<float>::max(),std::numeric_limits<float>::infinity(),
     -std::numeric_limits<float>::infinity(),std::numeric_limits<float>::quiet_NaN()}) {
  Session rejected(api),control(api);std::vector<float> values(961,.125f);values.back()=bad;auto saved=values;
  require(rejected.process(values.data(),values.size())==(std::isfinite(bad)?AG_RT_NODE_FAILED:AG_RT_NON_FINITE_INPUT),"bad input status");
  require(std::memcmp(values.data(),saved.data(),values.size()*sizeof(float))==0,"preflight input unchanged");
  values.assign(1441,.125f);equal(run(rejected,values,{127,480}),run(control,values,{127,480}));
 }
 Session oneFrame(api,false);p.maximum_frames=1;require(api.prepare(oneFrame.instance,&p,nullptr)==AG_OK,"capacity one");
 float pair[2]{};require(oneFrame.process(pair,2)==AG_RT_INVALID_BLOCK,"capacity enforced");require(oneFrame.process(pair,1)==AG_RT_OK,"minimum block");
}
}
int main(int argc,char** argv) {
 HMODULE module=nullptr;
 try {
  require(argc==2,"DLL argument");int n=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,argv[1],-1,nullptr,0);
  require(n>0,"UTF8 path");std::wstring path(static_cast<size_t>(n),L'\0');
  require(MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,argv[1],-1,path.data(),n)>0,"path conversion");
  module=LoadLibraryExW(path.c_str(),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);require(module!=nullptr,"load DLL");
  auto entry=reinterpret_cast<ag_get_api_fn>(GetProcAddress(module,"ag_plugin_get_api"));require(entry!=nullptr,"entry");
  Guard<ag_plugin_api> base;std::memset(&base,0xa5,sizeof(base));
  require(entry(1,1,sizeof(base),&base.table)==AG_UNSUPPORTED && untouched(base),"base version guard");
  require(entry(0,2,sizeof(base),&base.table)==AG_UNSUPPORTED && untouched(base),"base minor guard");
  require(entry(0,1,sizeof(base),nullptr)==AG_INVALID_ARGUMENT,"null base output");
  require(entry(0,1,sizeof(base.table)-1,&base.table)==AG_INVALID_ARGUMENT && untouched(base),"base size guard");
  require(entry(0,1,sizeof(base),&base.table)==AG_OK && untouched(base.tail),"base tail guard");
  require(std::string(base.table.plugin_id.data,static_cast<size_t>(base.table.plugin_id.size))=="org.audiograph.rnnoise","plugin id");
  require(std::string(base.table.plugin_version.data,static_cast<size_t>(base.table.plugin_version.size))=="0.1.0","plugin version");
  require(base.table.describe_nodes && base.table.get_node_api,"base callbacks");std::string json;
  require(base.table.describe_nodes(&json,description)==AG_OK && json.find("org.audiograph.rnnoise.realtime_v1")!=std::string::npos,"description");
  Guard<ag_realtime_audio_api> rt;std::memset(&rt,0xa5,sizeof(rt));auto query=base.table.get_node_api;auto type=s("org.audiograph.rnnoise.realtime_v1");
  require(query(type,s("unknown"),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && untouched(rt),"unknown capability");
  require(query(type,s(AG_WHOLE_SYNC_ID),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && untouched(rt),"whole sync");
  require(query(s("unknown"),s(AG_REALTIME_AUDIO_ID),1,sizeof(rt),&rt.table)==AG_UNSUPPORTED && untouched(rt),"unknown type");
  require(query(type,s(AG_REALTIME_AUDIO_ID),2,sizeof(rt),&rt.table)==AG_UNSUPPORTED && untouched(rt),"capability version");
  require(query(type,s(AG_REALTIME_AUDIO_ID),1,sizeof(rt.table)-1,&rt.table)==AG_INVALID_ARGUMENT && untouched(rt),"capability size");
  require(query(type,s(AG_REALTIME_AUDIO_ID),1,sizeof(rt),nullptr)==AG_INVALID_ARGUMENT,"null capability output");
  require(query(type,s(AG_REALTIME_AUDIO_ID),1,sizeof(rt),&rt.table)==AG_OK && untouched(rt.tail),"capability tail");
  auto api=rt.table;require(api.struct_size==sizeof(api) && api.version==1 && api.create && api.prepare && api.process && api.destroy,"callbacks");invalid(api);
  std::vector<float> input(480*12+173);uint32_t seed=123456789;
  for(size_t i=0;i<input.size();++i){seed=1664525u*seed+1013904223u;input[i]=.3f*std::sin(static_cast<float>(i)*.071f)+.04f*(static_cast<float>(seed>>8)/16777216.f-.5f);}
  input[0]=1.f;input[1]=-1.f;const auto expected=reference(input);
  require(std::any_of(expected.begin(),expected.end(),[](float x){return std::abs(x)>1e-3f;}),"non-silent oracle required to reject all-zero implementations");
  require(std::all_of(expected.begin(),expected.begin()+480,[](float x){return x==0;}),"initial 480 zero");
  for(size_t chunk:{1u,127u,256u,479u,480u,481u,960u,65536u}){Session session(api);equal(run(session,input,{chunk}),expected);}
  {Session session(api);equal(run(session,input,{1,127,256,479,480,481,960,65536}),expected);}
  {Session a(api),b(api);auto other=input;std::reverse(other.begin(),other.end());auto oa=input,ob=other;
   for(size_t i=0;i<input.size();++i){require(a.process(&oa[i],1)==AG_RT_OK,"instance A");require(b.process(&ob[i],1)==AG_RT_OK,"instance B");}
   equal(oa,expected);equal(ob,reference(other));}
  {Session reset(api);equal(run(reset,input,{481}),expected);}
  {Session silence(api);std::vector<float> zero(1921);auto out=run(silence,zero,{127,481});equal(out,reference(zero));require(std::all_of(out.begin(),out.end(),[](float x){return x==0;}),"silence");}
  {Session maximum(api);std::vector<float> large(65536,.125f);equal(run(maximum,large,{65536}),reference(large));}
  {Session timing(api);std::vector<float> block(480);std::array<double,100> times{};
   for(auto& time:times){std::copy_n(input.begin(),480,block.begin());auto start=std::chrono::steady_clock::now();require(timing.process(block.data(),480)==AG_RT_OK,"timing");time=std::chrono::duration<double,std::micro>(std::chrono::steady_clock::now()-start).count();}
   std::sort(times.begin(),times.end());std::cout<<"Process microseconds median="<<times[50]<<" p95="<<times[95]<<" max="<<times.back()<<"; diagnostic only, no deadline guarantee\n";}
  FreeLibrary(module);module=nullptr;std::cout<<"RNNoise SDK contracts passed. No devices, flush, or allocation instrumentation.\n";return 0;
 }catch(const std::exception& e){if(module)FreeLibrary(module);std::cerr<<e.what()<<'\n';return 1;}
}
