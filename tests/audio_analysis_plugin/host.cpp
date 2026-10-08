#include <windows.h>
#include <audiograph/plugin.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
constexpr char type_id[]="org.audiograph.analysis.audio_metrics_v1";
constexpr std::array<const char*,4> ports{"rms","rms_dbfs","clipping_ratio","silence_ratio"};
void check(bool b,const char* m){if(!b)throw std::runtime_error(m);}
ag_string str(const char* s){return {s,std::strlen(s)};}
bool eq(ag_string s,const char* v){return s.data&&s.size==std::strlen(v)&&!std::memcmp(s.data,v,static_cast<size_t>(s.size));}
struct Module{HMODULE h;explicit Module(const std::filesystem::path& p):h(LoadLibraryExW(p.c_str(),nullptr,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)){check(h!=nullptr,"DLL load failed");}~Module(){FreeLibrary(h);}};
struct Queries{unsigned cancel_calls{},time_calls{},cancel_at{},timeout_at{};bool throw_cancel{},throw_time{};};
// Deliberately misbehaving callbacks exercise the plugin's exception boundary.
// Ordinary hosts must contain their own exceptions as required by the SDK.
uint32_t AG_CALL cancelled(void* u){auto& q=*static_cast<Queries*>(u);++q.cancel_calls;if(q.throw_cancel)throw std::runtime_error("cancel fault");return q.cancel_at&&q.cancel_calls>=q.cancel_at;}
uint64_t AG_CALL remaining(void* u){auto& q=*static_cast<Queries*>(u);++q.time_calls;if(q.throw_time)throw std::runtime_error("deadline fault");return q.timeout_at&&q.time_calls>=q.timeout_at?0:AG_NO_DEADLINE;}
struct Outputs{std::array<double,4> values{};std::array<bool,4> seen{};unsigned calls{},reject_at{};bool throw_emit{};};
ag_status AG_CALL emit(void* u,ag_string port,const ag_value* v){auto& o=*static_cast<Outputs*>(u);++o.calls;if(o.throw_emit)throw std::runtime_error("emit fault");if(o.calls==o.reject_at)return AG_EXECUTION_FAILED;if(!v||v->type!=AG_NUMBER||v->reserved||!std::isfinite(v->data.number))return AG_INVALID_ARGUMENT;for(size_t i=0;i<4;++i)if(eq(port,ports[i])){if(o.seen[i])return AG_INVALID_ARGUMENT;o.seen[i]=true;o.values[i]=v->data.number;return AG_OK;}return AG_INVALID_ARGUMENT;}
ag_status AG_CALL throwing_error(void*,const ag_error*){throw std::runtime_error("report fault");}
ag_create_info info(const ag_instance_environment& env,const ag_named_value* p=nullptr,uint32_t n=0){ag_create_info c{};c.struct_size=sizeof(c);c.environment=&env;c.parameters=p;c.parameter_count=n;return c;}
ag_named_value param(double db){ag_named_value p{};p.id=str("silence_threshold_db");p.value.type=AG_NUMBER;p.value.data.number=db;return p;}
struct Instance{ag_whole_sync_api api;ag_instance* h{};Instance(ag_whole_sync_api a,const ag_create_info& c):api(a){check(api.create(&c,nullptr,&h)==AG_OK&&h,"create failed");}~Instance(){api.destroy(h);}Instance(const Instance&)=delete;};
struct Call{
 Queries queries;Outputs outputs;ag_named_value input{};ag_call_context context{};ag_output_sink sink{};
 Call(const std::vector<float>& a,uint32_t rate=48000,uint32_t channels=1){input.id=str("audio");input.value.type=AG_AUDIO;input.value.data.audio={rate,channels,a.size()/channels,a.size(),a.data()};context.struct_size=sizeof(context);context.user=&queries;context.is_cancelled=cancelled;context.remaining_ms=remaining;context.max_input_bytes=a.size()*4;context.max_output_bytes=32;context.max_inputs=1;context.max_outputs=4;sink.struct_size=sizeof(sink);sink.user=&outputs;sink.emit=emit;}
 ag_status run(const Instance& i){return i.api.run(i.h,&input,1,&context,&sink,nullptr);}
};
// Reference math deliberately uses no plugin implementation headers or helpers.
std::array<double,4> expected(const std::vector<float>& a,uint32_t rate,uint32_t channels,double db){
 long double sum=0;size_t clipped=0,quiet=0;for(float x:a){sum+=static_cast<long double>(x)*x;clipped+=x < -1.0f || x > 32767.0f/32768.0f;}
 const size_t frames=a.size()/channels,window=std::max<size_t>(1,(static_cast<uint64_t>(rate)+25)/50);const long double limit=std::pow(10.0L,static_cast<long double>(db)/20);
 for(size_t f=0;f<frames;f+=window){const size_t n=std::min(window,frames-f);long double power=0;for(size_t k=f*channels;k<(f+n)*channels;++k)power+=static_cast<long double>(a[k])*a[k];if(std::sqrt(power/(n*channels))<=limit)quiet+=n;}
 const double rms=static_cast<double>(std::sqrt(sum/a.size()));return {rms,rms==0?-120:std::max(-120.0,20*std::log10(rms)),static_cast<double>(clipped)/a.size(),static_cast<double>(quiet)/frames};
}
void verify(ag_whole_sync_api api,const ag_instance_environment& env,const std::vector<float>& a,uint32_t rate=48000,uint32_t channels=1,double db=-60,bool defaults=false){
 const auto original=a;auto p=param(db);Instance i(api,info(env,defaults?nullptr:&p,defaults?0:1));Call c(a,rate,channels);check(c.run(i)==AG_OK,"run failed");check(c.outputs.calls==4&&std::all_of(c.outputs.seen.begin(),c.outputs.seen.end(),[](bool b){return b;}),"outputs missing/duplicate");auto want=expected(a,rate,channels,db);
 for(size_t k=0;k<4;++k)check(std::fabs(c.outputs.values[k]-want[k])<=std::max(std::fabs(want[k])*1e-11,k==0?1e-300:1e-11),"reference metric mismatch");check(!std::memcmp(a.data(),original.data(),a.size()*4),"borrowed input changed");Call reuse(a,rate,channels);check(reuse.run(i)==AG_INVALID_ARGUMENT&&reuse.outputs.calls==0,"instance reused");
}
void create_tests(ag_whole_sync_api api,const ag_instance_environment& env){
 auto p=param(-60);auto c=info(env,&p,1);auto bad=[&](const ag_create_info* v,const ag_error_sink* e=nullptr){ag_instance* h=reinterpret_cast<ag_instance*>(uintptr_t{1});check(api.create(v,e,&h)==AG_INVALID_ARGUMENT&&!h,"invalid create accepted/handle uncleared");};
 bad(nullptr);bad(reinterpret_cast<const ag_create_info*>(UINTPTR_MAX-7));c.struct_size--;bad(&c);c=info(env,&p,1);c.reserved=1;bad(&c);c.reserved=0;c.reserved2=1;bad(&c);c=info(env,nullptr,1);bad(&c);c=info(env,&p,2);bad(&c);c=info(env,&p,1);
 for(double db:{-121.,1.,std::numeric_limits<double>::infinity(),std::numeric_limits<double>::quiet_NaN()}){p.value.data.number=db;bad(&c);}
 p=param(-60);p.value.type=AG_TEXT;bad(&c);p=param(-60);p.value.reserved=1;bad(&c);p=param(-60);p.id=str("unknown");bad(&c);p.id={nullptr,20};bad(&c);p.id={reinterpret_cast<const char*>(UINTPTR_MAX-2),20};bad(&c);
 c=info(env,reinterpret_cast<const ag_named_value*>(UINTPTR_MAX-7),1);bad(&c);alignas(ag_named_value)std::array<unsigned char,sizeof(ag_named_value)+8> bytes{};c=info(env,reinterpret_cast<const ag_named_value*>(bytes.data()+1),1);bad(&c);
 p=param(-60);c=info(env,&p,1);alignas(ag_create_info)std::array<unsigned char,sizeof(ag_create_info)+8> cb{};std::memcpy(cb.data()+1,&c,sizeof(c));bad(reinterpret_cast<const ag_create_info*>(cb.data()+1));
 auto broken=env;broken.reserved=1;c=info(broken);bad(&c);broken=env;broken.struct_size--;bad(&c);c=info(env,&p,1);c.environment=reinterpret_cast<const ag_instance_environment*>(UINTPTR_MAX-7);bad(&c);
 alignas(ag_instance_environment)std::array<unsigned char,sizeof(ag_instance_environment)+8> eb{};std::memcpy(eb.data()+1,&env,sizeof(env));c.environment=reinterpret_cast<const ag_instance_environment*>(eb.data()+1);bad(&c);
 c=info(env,&p,1);p.value.data.number=1;ag_error_sink errors{};errors.struct_size=sizeof(errors);errors.report=throwing_error;bad(&c,&errors);
 p=param(-60);c=info(env,&p,1);errors.report=nullptr;bad(&c,&errors);errors.report=throwing_error;errors.struct_size--;bad(&c,&errors);errors.struct_size=sizeof(errors);errors.reserved=1;bad(&c,&errors);
 check(api.create(&c,nullptr,nullptr)==AG_INVALID_ARGUMENT,"null output handle accepted");
 for(double db:{-120.,0.}){p=param(db);Instance accepted(api,info(env,&p,1));}
}
void run_tests(ag_whole_sync_api api,const ag_instance_environment& env){
 const std::vector<float> a(64,0.5f);
 {Call c(a);check(api.run(nullptr,&c.input,1,&c.context,&c.sink,nullptr)==AG_INVALID_ARGUMENT&&c.outputs.calls==0,"null instance accepted");}
 for(unsigned n=0;n<24;++n){Instance i(api,info(env));Call c(a);ag_status want=AG_INVALID_ARGUMENT;
 switch(n){case 0:c.context.struct_size--;break;case 1:c.context.reserved=1;break;case 2:c.sink.struct_size--;break;case 3:c.sink.reserved=1;break;case 4:c.sink.emit=nullptr;break;case 5:c.input.value.reserved=1;break;case 6:c.input.value.type=AG_NUMBER;break;case 7:c.input.id=str("wrong");break;case 8:c.input.value.data.audio.samples=nullptr;break;case 9:c.input.value.data.audio.sample_rate=0;break;case 10:c.input.value.data.audio.channel_count=0;break;case 11:c.input.value.data.audio.sample_count--;break;case 12:c.input.value.data.audio.frame_count=UINT64_MAX;c.input.value.data.audio.channel_count=2;break;case 13:c.input.value.data.audio.samples=reinterpret_cast<const float*>(UINTPTR_MAX-1);break;case 14:c.input.value.data.audio.samples=reinterpret_cast<const float*>(reinterpret_cast<const unsigned char*>(a.data())+1);break;case 15:c.input.id={nullptr,5};break;case 16:c.input.id={reinterpret_cast<const char*>(UINTPTR_MAX-2),5};break;case 17:c.context.max_input_bytes--;want=AG_RESOURCE_LIMIT;break;case 18:c.context.max_inputs=0;want=AG_RESOURCE_LIMIT;break;case 19:c.context.max_outputs=3;want=AG_RESOURCE_LIMIT;break;case 20:c.context.max_output_bytes=31;want=AG_RESOURCE_LIMIT;break;case 21:c.context.is_cancelled=nullptr;break;case 22:c.context.remaining_ms=nullptr;break;case 23:c.input.value.data.audio.frame_count=0;c.input.value.data.audio.sample_count=0;break;}
 check(c.run(i)==want&&c.outputs.calls==0,"invalid run/budget emitted or accepted");}
 for(float x:{std::numeric_limits<float>::infinity(),-std::numeric_limits<float>::infinity(),std::numeric_limits<float>::quiet_NaN()}){std::vector<float> a_bad(8193,0);a_bad.back()=x;Instance i(api,info(env));Call c(a_bad);check(c.run(i)==AG_INVALID_ARGUMENT&&c.outputs.calls==0,"nonfinite accepted");}
 for(unsigned n=0;n<9;++n){Instance i(api,info(env));Call c(a);auto* input=&c.input;auto* context=&c.context;auto* sink=&c.sink;uint32_t count=1;alignas(ag_named_value)std::array<unsigned char,sizeof(ag_named_value)+8> ib{};alignas(ag_call_context)std::array<unsigned char,sizeof(ag_call_context)+8> cb{};alignas(ag_output_sink)std::array<unsigned char,sizeof(ag_output_sink)+8> sb{};
 switch(n){case 0:input=nullptr;break;case 1:count=0;break;case 2:count=2;break;case 3:context=nullptr;break;case 4:sink=nullptr;break;case 5:input=reinterpret_cast<ag_named_value*>(UINTPTR_MAX-7);break;case 6:std::memcpy(ib.data()+1,input,sizeof(*input));input=reinterpret_cast<ag_named_value*>(ib.data()+1);break;case 7:std::memcpy(cb.data()+1,context,sizeof(*context));context=reinterpret_cast<ag_call_context*>(cb.data()+1);break;case 8:std::memcpy(sb.data()+1,sink,sizeof(*sink));sink=reinterpret_cast<ag_output_sink*>(sb.data()+1);break;}
 check(api.run(i.h,input,count,context,sink,nullptr)==AG_INVALID_ARGUMENT&&c.outputs.calls==0,"invalid run pointers accepted");}
}
void cooperative_tests(ag_whole_sync_api api,const ag_instance_environment& env){const std::vector<float>a(4096*12,0.5f);
 for(unsigned n=0;n<8;++n){Instance i(api,info(env));Call c(a);ag_status want=AG_INTERNAL_ERROR;switch(n){case 0:c.queries.cancel_at=1;want=AG_CANCELLED;break;case 1:c.queries.cancel_at=3;want=AG_CANCELLED;break;case 2:c.queries.timeout_at=1;want=AG_DEADLINE_EXCEEDED;break;case 3:c.queries.timeout_at=3;want=AG_DEADLINE_EXCEEDED;break;case 4:c.queries.throw_cancel=true;break;case 5:c.queries.throw_time=true;break;case 6:c.outputs.throw_emit=true;break;case 7:c.outputs.reject_at=2;want=AG_EXECUTION_FAILED;break;}check(c.run(i)==want,"cancel/deadline/callback fault status wrong");check(c.outputs.calls==(n==6?1u:n==7?2u:0u),"continued after rejection/cancellation");}
 Instance i(api,info(env));Call c(a);check(c.run(i)==AG_OK,"poll run failed");check(c.queries.cancel_calls>=a.size()/4096&&c.queries.time_calls>=a.size()/4096,"polling interval exceeds 4096 samples");}
ag_status AG_CALL describe(void* u,ag_string json){if(!json.data||json.size>65536)return AG_INVALID_ARGUMENT;static_cast<std::string*>(u)->assign(json.data,static_cast<size_t>(json.size));return AG_OK;}
}
int main(int argc,char** argv){try{
 check(argc==2,"Usage: ag_audio_analysis_sdk_host ABSOLUTE_DLL");auto path=std::filesystem::u8path(argv[1]);check(path.is_absolute()&&std::filesystem::is_regular_file(path),"DLL path must be absolute");Module module(path);auto entry=reinterpret_cast<ag_get_api_fn>(GetProcAddress(module.h,"ag_plugin_get_api"));check(entry!=nullptr,"entry missing");
 alignas(ag_plugin_api)std::array<unsigned char,sizeof(ag_plugin_api)+16> bytes{};bytes.fill(0xA5);auto* table=reinterpret_cast<ag_plugin_api*>(bytes.data());check(entry(AG_ABI_MAJOR+1,AG_ABI_MINOR,sizeof(*table),table)==AG_UNSUPPORTED,"major accepted");check(entry(AG_ABI_MAJOR,AG_ABI_MINOR+1,sizeof(*table),table)==AG_UNSUPPORTED,"minor accepted");check(entry(AG_ABI_MAJOR,AG_ABI_MINOR,sizeof(*table)-1,table)==AG_INVALID_ARGUMENT,"undersized table accepted");check(std::all_of(bytes.begin(),bytes.end(),[](auto x){return x==0xA5;}),"failed table query writes");check(entry(AG_ABI_MAJOR,AG_ABI_MINOR,sizeof(*table),table)==AG_OK,"ABI query failed");check(std::all_of(bytes.begin()+sizeof(*table),bytes.end(),[](auto x){return x==0xA5;}),"table guard writes");ag_plugin_api plugin{};std::memcpy(&plugin,table,sizeof(plugin));check(plugin.struct_size==sizeof(plugin)&&!plugin.reserved&&eq(plugin.plugin_id,"org.audiograph.analysis")&&eq(plugin.plugin_version,"0.1.0")&&plugin.describe_nodes&&plugin.get_node_api,"metadata wrong");
 ag_whole_sync_api api{};alignas(ag_whole_sync_api)std::array<unsigned char,sizeof(api)+16> selected{};auto query=[&](const char* type,const char* cap,uint32_t ver,uint32_t size){selected.fill(0xA5);return plugin.get_node_api(str(type),str(cap),ver,size,selected.data());};check(query("unknown",AG_WHOLE_SYNC_ID,1,sizeof(api))==AG_UNSUPPORTED,"unknown type accepted");check(query(type_id,AG_REALTIME_AUDIO_ID,1,sizeof(api))==AG_UNSUPPORTED,"realtime accepted");check(query(type_id,AG_WHOLE_SYNC_ID,2,sizeof(api))==AG_UNSUPPORTED,"cap version accepted");check(query(type_id,AG_WHOLE_SYNC_ID,1,sizeof(api)-1)==AG_INVALID_ARGUMENT,"undersized capability accepted");check(std::all_of(selected.begin(),selected.end(),[](auto x){return x==0xA5;}),"failed capability writes");check(query(type_id,AG_WHOLE_SYNC_ID,1,sizeof(api))==AG_OK,"capability failed");check(std::all_of(selected.begin()+sizeof(api),selected.end(),[](auto x){return x==0xA5;}),"capability guard writes");std::memcpy(&api,selected.data(),sizeof(api));check(api.struct_size==sizeof(api)&&api.version==1&&api.create&&api.run&&api.destroy,"cap table wrong");
 std::string json;check(plugin.describe_nodes(&json,describe)==AG_OK,"describe failed");std::ifstream file(path.parent_path()/"nodes.json",std::ios::binary);check(static_cast<bool>(file),"nodes.json absent");const std::string packaged((std::istreambuf_iterator<char>(file)),std::istreambuf_iterator<char>());check(json==packaged&&json.find(type_id)!=std::string::npos&&json.find("silence_threshold_db")!=std::string::npos,"description mismatch");
 const auto root_bytes=path.parent_path().u8string();const std::string root(reinterpret_cast<const char*>(root_bytes.data()),root_bytes.size());ag_instance_environment env{};env.struct_size=sizeof(env);env.resource_root={root.data(),root.size()};env.data_root=env.resource_root;
 create_tests(api,env);run_tests(api,env);cooperative_tests(api,env);verify(api,env,std::vector<float>(2000,0.5f),48000,1,-60,true);verify(api,env,{0.25f,-0.5f,0.75f,-1});const float endpoint=32767.0f/32768.0f;verify(api,env,{-1,std::nextafter(-1.0f,-2.0f),std::nextafter(-1.0f,0.0f),endpoint,std::nextafter(endpoint,0.0f),std::nextafter(endpoint,1.0f),1,2});
 for(uint32_t rate:{44100u,48000u,1u,49u,12345u,UINT32_MAX}){verify(api,env,std::vector<float>(37,0),rate);verify(api,env,std::vector<float>(37,0.5f),rate);}
 for(uint32_t rate:{44100u,48000u,12345u}){const size_t window=(static_cast<uint64_t>(rate)+25)/50;std::vector<float> mixed(window*2+window/3,0);std::fill(mixed.begin()+window,mixed.begin()+window*2,0.5f);verify(api,env,mixed,rate);}
 std::vector<float> stereo(1920);for(size_t k=1;k<stereo.size();k+=2)stereo[k]=0.0012f;verify(api,env,stereo,48000,2);verify(api,env,{std::numeric_limits<float>::max(),-std::numeric_limits<float>::max()});verify(api,env,{std::numeric_limits<float>::denorm_min(),-std::numeric_limits<float>::denorm_min()});
 std::vector<float> sine(48000);for(size_t k=0;k<sine.size();++k)sine[k]=static_cast<float>(0.5*std::sin(2*3.14159265358979323846*1000*static_cast<double>(k)/48000));verify(api,env,sine);
 verify(api,env,std::vector<float>(960,1.0f),48000,1,0);verify(api,env,std::vector<float>(960,0.0f),48000,1,-120);
 auto p=param(-20);Instance first(api,info(env)),second(api,info(env,&p,1));const std::vector<float> quiet(100,0.01f);Call a(quiet),b(quiet);check(a.run(first)==AG_OK&&b.run(second)==AG_OK&&a.outputs.values[3]==0&&b.outputs.values[3]==1,"instances share parameters");std::cout<<"Audio analysis SDK DLL tests passed\n";return 0;
 }catch(const std::exception& e){std::cerr<<"Audio analysis SDK test failed: "<<e.what()<<'\n';return 1;}}
