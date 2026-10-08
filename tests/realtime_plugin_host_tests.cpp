#include "audioprocess/plugin_host.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/realtime_graph_executor.h"
#include "audioprocess/realtime_bridge.h"
#include <nlohmann/json.hpp>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#include <array>
#include <algorithm>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <cstdlib>
#include <new>

// Observes ordinary C++ new/new[] on this host thread; does not intercept DLL
// malloc/OS allocation or claim to verify arbitrary third-party native code.
namespace allocation_probe { thread_local bool enabled{}; thread_local std::size_t count{}; }
void* operator new(std::size_t size) {
    if(allocation_probe::enabled) ++allocation_probe::count;
    if(auto* memory=std::malloc(size ? size : 1)) return memory;
    throw std::bad_alloc{};
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* memory) noexcept { std::free(memory); }
void operator delete[](void* memory) noexcept { std::free(memory); }
void operator delete(void* memory,std::size_t) noexcept { std::free(memory); }
void operator delete[](void* memory,std::size_t) noexcept { std::free(memory); }

namespace {
using namespace audioprocess;
using Json=nlohmann::json;
namespace fs=std::filesystem;
void check(bool value,const char* message) { if (!value) throw std::runtime_error(message); }
std::string utf8(const fs::path& path) { const auto text=path.u8string(); return {reinterpret_cast<const char*>(text.data()),text.size()}; }
std::string hash(const fs::path& path) {
    BCRYPT_ALG_HANDLE alg{}; BCRYPT_HASH_HANDLE state{};
    check(BCryptOpenAlgorithmProvider(&alg,BCRYPT_SHA256_ALGORITHM,nullptr,0)>=0,"hash initialization");
    check(BCryptCreateHash(alg,&state,nullptr,0,nullptr,0,0)>=0,"hash creation");
    std::ifstream input(path,std::ios::binary); check(bool(input),"fixture read");
    std::array<unsigned char,4096> bytes{};
    while(input) { input.read(reinterpret_cast<char*>(bytes.data()),bytes.size());
        if(input.gcount()) check(BCryptHashData(state,bytes.data(),static_cast<ULONG>(input.gcount()),0)>=0,"hash update"); }
    std::array<unsigned char,32> digest{};
    check(BCryptFinishHash(state,digest.data(),static_cast<ULONG>(digest.size()),0)>=0,"hash finish");
    BCryptDestroyHash(state); BCryptCloseAlgorithmProvider(alg,0);
    std::string out; constexpr char hex[]="0123456789abcdef";
    for(auto b:digest) { out+=hex[b>>4]; out+=hex[b&15]; } return out;
}
Json read(const fs::path& path) { std::ifstream in(path); Json out; in>>out; return out; }
void write(const fs::path& path,const Json& value) { std::ofstream out(path,std::ios::binary); out<<value.dump(); check(bool(out),"fixture write"); }
struct Temp {
    fs::path root=fs::temp_directory_path()/("audio-realtime-plugin-host-"+std::to_string(GetCurrentProcessId())+"-"+std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
    Temp() { fs::create_directories(root/"workspace"); fs::create_directories(root/"data"); }
    ~Temp() { std::error_code error; fs::remove_all(root,error); }
};
PluginHostOptions options(const fs::path& package,Temp& temp,const std::string& name) {
    auto path=temp.root/(name+".json");
    write(path,{{"schema_version",1},{"packages",Json::array({{{"root",utf8(package)},{"manifest_sha256",hash(package/"manifest.json")}}})}});
    return {path,hash(path),temp.root/"data",temp.root/"workspace"};
}
NodeRegistry registry() { NodeRegistry out; register_realtime_nodes(out); return out; }
GraphDefinition graph() { auto out=make_realtime_graph("test-input","test-output",-6.0f); out.nodes[1].type_id="org.audiograph.realtime_example.gain_v1"; return out; }
void run(const fs::path& supplied,const fs::path& fixture) {
    Temp temp;
    const auto package=temp.root/"package";
    fs::copy(supplied,package,fs::copy_options::recursive);
    if(!fixture.empty()) fs::copy_file(fixture,package/"org.audiograph.realtime_example.plugin.dll",fs::copy_options::overwrite_existing);
    auto fixture_manifest=read(package/"manifest.json");
    for(auto& file:fixture_manifest["files"]) if(file["path"]=="org.audiograph.realtime_example.plugin.dll") file["sha256"]=hash(package/"org.audiograph.realtime_example.plugin.dll");
    write(package/"manifest.json",fixture_manifest);
    // Mutated packages are only discovered, so their DLLs are never loaded/pinned.
    const auto original=read(package/"nodes.json");
    const auto manifest_original=read(package/"manifest.json");
    for(int mutation=0;mutation<10;++mutation) {
        const auto bad=temp.root/("bad-"+std::to_string(mutation)); fs::copy(package,bad,fs::copy_options::recursive);
        auto nodes=original; auto manifest=manifest_original; auto& node=nodes["nodes"][0];
        switch(mutation) {
        case 0: node["outputs"][0]["type"]="Audio"; break;
        case 1: node["inputs"][0]["required"]=false; break;
        case 2: node["outputs"].push_back(node["outputs"][0]); break;
        case 3: node["realtime_capabilities"]["maximum_block_frames"]=0; break;
        case 4: node["realtime_capabilities"]["supports_variable_blocks"]=false; break;
        case 5: node["realtime_capabilities"]["format"]["channels"]=2; break;
        case 6: node["realtime_capabilities"]["extra"]=true; break;
        case 7: manifest["capabilities"]=Json::array({{{"id","ag.whole_sync/1"},{"version",1}}}); break;
        case 8: manifest["capabilities"].push_back({{"id","ag.whole_sync/1"},{"version",1}}); break;
        case 9: node["plugin"]["capabilities"][0]["version"]=2; break;
        }
        write(bad/"nodes.json",nodes);
        for(auto& file:manifest["files"]) if(file["path"]=="nodes.json") file["sha256"]=hash(bad/"nodes.json");
        write(bad/"manifest.json",manifest);
        auto reg=registry(); const auto report=Json::parse(register_plugin_nodes(reg,options(bad,temp,"bad-snapshot-"+std::to_string(mutation))));
        check(report["available"].empty() && report["errors"].size()==1,"Malformed realtime descriptor admitted");
    }
    {
        const auto mixed=temp.root/"mixed"; fs::copy(package,mixed,fs::copy_options::recursive);
        auto nodes=original; auto manifest=manifest_original; auto whole=nodes["nodes"][0];
        whole["typeId"]="org.audiograph.realtime_example.offline_test";
        whole["execution_domain"]="synchronous"; whole["realtime_role"]="none"; whole.erase("realtime_capabilities");
        whole["inputs"][0]["type"]="Audio"; whole["outputs"][0]["type"]="Audio";
        whole["plugin"]["capabilities"]=Json::array({{{"id","ag.whole_sync/1"},{"version",1}}});
        nodes["nodes"].push_back(whole); manifest["capabilities"].push_back({{"id","ag.whole_sync/1"},{"version",1}});
        write(mixed/"nodes.json",nodes);
        for(auto& file:manifest["files"]) if(file["path"]=="nodes.json") file["sha256"]=hash(mixed/"nodes.json");
        write(mixed/"manifest.json",manifest);
        auto mixed_registry=registry();
        const auto mixed_report=Json::parse(register_plugin_nodes(mixed_registry,options(mixed,temp,"mixed-snapshot")));
        check(mixed_report["errors"].empty() && mixed_report["available"].size()==1,"Valid mixed capability union rejected");
        const auto reversed=temp.root/"mixed-reversed"; fs::copy(mixed,reversed,fs::copy_options::recursive);
        std::reverse(manifest["capabilities"].begin(),manifest["capabilities"].end());
        write(reversed/"manifest.json",manifest);
        auto reversed_registry=registry();
        const auto reversed_report=Json::parse(register_plugin_nodes(reversed_registry,options(reversed,temp,"mixed-reversed-snapshot")));
        check(reversed_report["errors"].empty() && reversed_report["available"].size()==1,"Reverse capability ordering rejected");
        bool mismatch_rejected=false;
        try { auto mismatch=RealtimeGraphExecutor::compile(graph(),mixed_registry); mismatch.prepare(); }
        catch(const std::exception&) { mismatch_rejected=true; }
        check(mismatch_rejected,"Runtime description mismatch accepted");
    }
    const auto dll=package/L"org.audiograph.realtime_example.plugin.dll";
    check(GetModuleHandleW(dll.filename().c_str())==nullptr,"Unexpected module loaded before discovery");
    auto reg=registry(); const auto report=Json::parse(register_plugin_nodes(reg,options(package,temp,"good")));
    check(report["errors"].empty() && report["available"].size()==1,"Realtime package discovery failed");
    const auto metadata=Json::parse(node_description_json(reg.descriptor(graph().nodes[1].type_id)));
    check(metadata["node"]["plugin"]["capabilities"][0]["id"]=="ag.realtime_audio/1","Catalog capability provenance lost");
    auto plan=RealtimeGraphExecutor::compile(graph(),reg);
    check(GetModuleHandleW(dll.filename().c_str())==nullptr,"Compile loaded plugin DLL");
    plan.prepare({48000,1},256);
    check(GetModuleHandleW(dll.filename().c_str())!=nullptr,"Prepare failed to load plugin DLL");
    const auto module=GetModuleHandleW(dll.filename().c_str());
    using Control=void (__cdecl*)(std::uint32_t);
    using Count=std::uint32_t (__cdecl*)();
    const auto control=reinterpret_cast<Control>(GetProcAddress(module,"ag_rt_test_control"));
    const auto creates=reinterpret_cast<Count>(GetProcAddress(module,"ag_rt_test_creates"));
    const auto destroys=reinterpret_cast<Count>(GetProcAddress(module,"ag_rt_test_destroys"));
    if(!fixture.empty()) check(control && creates && destroys,"Test-only fixture controls missing");
    const float gain=std::pow(10.0f,-6.0f/20.0f);
    std::array<float,257> samples{};
    for(const std::size_t count:{1u,7u,256u}) {
        samples.fill(0.25f); check(plan.process(std::span(samples).first(count)).status==RealtimeProcessStatus::Ok,"Variable block failed");
        for(std::size_t i=0;i<count;++i) check(std::abs(samples[i]-0.25f*gain)<1e-6f,"Gain sample mismatch");
    }
    samples.fill(0.25f); allocation_probe::count=0; allocation_probe::enabled=true;
    const auto callback_result=plan.process(std::span(samples).first(256));
    allocation_probe::enabled=false;
    check(callback_result.status==RealtimeProcessStatus::Ok && allocation_probe::count==0,"Host realtime callback allocated");
    check(plan.process(samples).status==RealtimeProcessStatus::InvalidBlock,"Over-capacity block accepted");
    samples[0]=std::numeric_limits<float>::quiet_NaN(); check(plan.process(std::span(samples).first(1)).status==RealtimeProcessStatus::NonFiniteInput,"NaN input accepted");
    bool rejected=false; try { plan.prepare({48000,1},65537); } catch(...) { rejected=true; }
    check(rejected,"Invalid prepare succeeded"); samples.fill(0.25f);
    check(plan.process(std::span(samples).first(7)).status==RealtimeProcessStatus::Ok,"Failed prepare destroyed old plan");
    if(fixture.empty()) return; // Above checks also run against the unmodified production sample.
    const auto created=creates(),destroyed=destroys(); control(1);
    rejected=false; try { plan.prepare({48000,1},256); } catch(...) { rejected=true; }
    check(rejected && creates()==created+1 && destroys()==destroyed+1,"Failed native prepare did not destroy exactly once");
    control(0); samples.fill(0.25f);
    check(plan.process(std::span(samples).first(7)).status==RealtimeProcessStatus::Ok,"Native prepare failure discarded valid old plan");
    for(std::uint32_t mode:{2u,3u,4u}) {
        control(mode); RealtimeBridge bridge({48000,4096,256,false},&plan);
        std::array<float,1024> captured{}; captured.fill(0.25f); std::array<float,256> output{}; output.fill(1.0f);
        bridge.capture(captured.data(),static_cast<std::uint32_t>(captured.size())); bridge.render(output.data(),static_cast<std::uint32_t>(output.size()),1);
        check(bridge.faulted(),"Plugin failure was not latched");
        check(std::all_of(output.begin(),output.end(),[](float s){return s==0.0f;}),"Plugin failure did not mute complete render block");
        check(bridge.process_fault().status==(mode==4 ? RealtimeProcessStatus::NonFiniteOutput : RealtimeProcessStatus::NodeFailed),"Native status mapping incorrect");
    }
    for(std::uint32_t mode:{5u,6u}) {
        control(mode); const auto before=destroys(); rejected=false;
        try { plan.prepare({48000,1},256); } catch(...) { rejected=true; }
        check(rejected && destroys()==before,"Failed create transferred ownership or destroyed invalid pointer");
    }
    control(0);
    const auto before=destroys(); { auto fresh=RealtimeGraphExecutor::compile(graph(),reg); fresh.prepare(); }
    check(destroys()==before+1,"Successful instance not destroyed exactly once after plan lifetime");
}
}
int main(int argc,char** argv) {
    try { check(argc==2 || argc==3,"Usage: realtime-plugin-host-tests PACKAGE_DIRECTORY [FIXTURE_DLL]"); run(fs::absolute(path_from_utf8(argv[1])),argc==3 ? fs::absolute(path_from_utf8(argv[2])) : fs::path{}); std::cout<<"realtime plugin host tests passed\n"; return 0; }
    catch(const std::exception& error) { std::cerr<<error.what()<<'\n'; return 1; }
}
