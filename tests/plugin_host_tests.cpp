#include "audioprocess/plugin_host.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/graph_validator.h"
#include "audioprocess/graph_codec.h"
#include "audioprocess/prototype_nodes.h"
#include "audioprocess/sync_graph_executor.h"
#include <nlohmann/json.hpp>

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <initializer_list>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace {
namespace fs=std::filesystem;
using Json=nlohmann::json;
using namespace audioprocess;
static_assert(detail::plugin_text_output_fits(0,64ull*1024));
static_assert(!detail::plugin_text_output_fits(0,64ull*1024+1));
static_assert(detail::plugin_text_output_fits(64ull*1024-1,1));
static_assert(!detail::plugin_text_output_fits(64ull*1024-1,2));
static_assert(detail::plugin_call_output_limit(false) == 64ull*1024);
static_assert(detail::plugin_call_output_limit(true) == 256ull*1024*1024);

void check(bool condition, const char* message) { if (!condition) throw std::runtime_error(message); }
std::string utf8(const fs::path& path) {
    const auto bytes=path.u8string();
    return {reinterpret_cast<const char*>(bytes.data()),bytes.size()};
}
fs::path verbatim(const fs::path& path) {
    const auto absolute=fs::absolute(path).lexically_normal().native();
    if (absolute.rfind(L"\\\\?\\",0) == 0) return fs::path(absolute);
    return fs::path(std::wstring(L"\\\\?\\")+absolute);
}
std::string sha256(const fs::path& file) {
    BCRYPT_ALG_HANDLE algorithm{}; BCRYPT_HASH_HANDLE hash{};
    check(BCryptOpenAlgorithmProvider(&algorithm,BCRYPT_SHA256_ALGORITHM,nullptr,0) >= 0,"Cannot initialize test SHA-256");
    check(BCryptCreateHash(algorithm,&hash,nullptr,0,nullptr,0,0) >= 0,"Cannot create test hash");
    std::ifstream input(file,std::ios::binary);
    check(static_cast<bool>(input),"Cannot read test fixture");
    std::array<unsigned char,4096> buffer{};
    while (input) {
        input.read(reinterpret_cast<char*>(buffer.data()),buffer.size());
        const auto count=static_cast<ULONG>(input.gcount());
        if (count) check(BCryptHashData(hash,buffer.data(),count,0) >= 0,"Cannot hash test fixture");
    }
    std::array<unsigned char,32> digest{};
    check(BCryptFinishHash(hash,digest.data(),static_cast<ULONG>(digest.size()),0) >= 0,"Cannot finish test hash");
    BCryptDestroyHash(hash); BCryptCloseAlgorithmProvider(algorithm,0);
    constexpr char hex[]="0123456789abcdef";
    std::string result; result.reserve(64);
    for (auto byte:digest) { result.push_back(hex[byte>>4]); result.push_back(hex[byte&15]); }
    return result;
}
struct Temp {
    fs::path root;
    Temp() {
        root=fs::temp_directory_path()/("audio-plugin-host-test-"+std::to_string(GetCurrentProcessId())+"-"+
            std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
        check(fs::create_directory(root),"Cannot create isolated test directory");
        fs::create_directories(root/"workspace"); fs::create_directories(root/"data");
    }
    ~Temp() { std::error_code ignored; fs::remove_all(root,ignored); }
};
struct Snapshot {
    fs::path path;
    std::string hash;
};
Snapshot snapshot(const fs::path& target, const std::vector<fs::path>& packages) {
    Json document={{"schema_version",1},{"packages",Json::array()}};
    for (const auto& package:packages) document["packages"].push_back({
        {"root",utf8(package)},{"manifest_sha256",sha256(package/"manifest.json")}});
    std::ofstream output(target,std::ios::binary|std::ios::trunc);
    check(static_cast<bool>(output),"Cannot write snapshot fixture");
    output << document.dump(); output.close();
    return {target,sha256(target)};
}
PluginHostOptions options(const Snapshot& captured, const fs::path& temp) {
    return {verbatim(captured.path),captured.hash,verbatim(temp/"data"),verbatim(temp/"workspace")};
}
class Source final : public ISyncNode {
public:
    Source() {
        descriptor_.type_id="test.plugin_source";
        descriptor_.display_name="Test source";
        descriptor_.outputs={{"audio",DataType::Audio,true}};
    }
    const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    OutputValues execute(const InputValues&,ExecutionContext&) override {
        auto clip=std::make_shared<AudioClip>();
        clip->format={48000,1}; clip->samples={0.25f,-0.5f,0.75f,-1.0f};
        return {{"audio",AudioClipPtr{std::move(clip)}}};
    }
private:
    NodeDescriptor descriptor_{};
};
NodeRegistry registry() {
    auto result=create_prototype_node_registry();
    Source source;
    result.register_type(source.descriptor(),[](const ParameterMap&) {return std::make_unique<Source>();});
    return result;
}
GraphDefinition graph(double gain, double delay) {
    return {{
        {"source","test.plugin_source",{}},
        {"gain","org.audiograph.example.gain_v1",{{"gain_db",gain}}},
        {"mock","org.audiograph.example.mock_asr_v1",{{"delay_ms",delay}}}
    },{
        {"source","audio","gain","audio"},
        {"gain","audio","mock","audio"}
    },{
        {"audio","gain","audio"},{"text","mock","text"}
    }};
}
void expect_error(const auto& action, const char* code) {
    try { action(); } catch (const ExecutionError& error) {
        check(error.code == code,"Unexpected plugin error code"); return;
    }
    throw std::runtime_error("Expected plugin error was not raised");
}
void copy_package(const fs::path& from, const fs::path& to) {
    fs::create_directory(to);
    for (const auto& entry:fs::directory_iterator(from)) {
        if (entry.is_regular_file()) fs::copy_file(entry.path(),to/entry.path().filename());
    }
}

void run_checks(const fs::path& package) {
    Temp temp;
    const auto raw_snapshot=snapshot(temp.root/"plugins.json",{verbatim(package)});
    auto flags=options(raw_snapshot,temp.root);
    auto nodes=registry();
    const auto baseline=nodes.descriptors().size();
    check(GetModuleHandleW(L"ag_example_plugin.dll") == nullptr,"Plugin DLL loaded before registration");
    const auto report=Json::parse(register_plugin_nodes(nodes,flags));
    check(report["snapshot_id"] == flags.snapshot_sha256 && report["available"].size() == 1 &&
        report["errors"].empty(),"Valid static package registration failed");
    check(nodes.descriptors().size() == baseline+2,"Plugin descriptors were not registered");
    check(nodes.descriptor("org.audiograph.example.gain_v1").plugin.has_value(),"Plugin origin metadata missing");
    check(GetModuleHandleW(L"ag_example_plugin.dll") == nullptr,"Validation eagerly loaded plugin DLL");
    const auto proposed=graph(-6.020599913279624,0);
    (void)validate_graph(proposed,nodes);
    check(GetModuleHandleW(L"ag_example_plugin.dll") == nullptr,"Graph validation eagerly loaded plugin DLL");
    auto executor=SyncGraphExecutor::compile(proposed,nodes);
    check(GetModuleHandleW(L"ag_example_plugin.dll") == nullptr,"Graph compile eagerly loaded plugin DLL");
    const auto result=executor.execute();
    check(GetModuleHandleW(L"ag_example_plugin.dll") != nullptr,"Execution did not load plugin DLL");
    const auto& audio=std::get<AudioClipPtr>(result.value("gain","audio"));
    check(audio && audio->samples.size() == 4 && std::fabs(audio->samples[0]-0.125f) < 0.0005f,
        "Plugin gain Graph output is wrong");
    const auto& text=std::get<std::string>(result.value("mock","text"));
    check(text.find("[MOCK ASR]") != std::string::npos &&
        text.find("no speech recognition performed") != std::string::npos,
        "Plugin mock text did not disclose simulation");
    const auto repeated=executor.execute();
    check(std::get<std::string>(repeated.value("mock","text")) == text,
        "Independent Graph execution did not create fresh plugin instances");
    std::atomic_bool cancel{true};
    expect_error([&] { (void)executor.execute({&cancel}); },"cancelled");
    cancel=false;
    auto slow=SyncGraphExecutor::compile(graph(0,2000),nodes);
    std::jthread stopper([&] { std::this_thread::sleep_for(std::chrono::milliseconds(30)); cancel=true; });
    expect_error([&] { (void)slow.execute({&cancel}); },"cancelled");
    const auto data_dir=temp.root/"data"/"org.audiograph.example";
    check(fs::is_directory(data_dir),"Plugin data directory was not created at execution");

    auto mismatched=flags; mismatched.snapshot_sha256=std::string(64,'0');
    auto separate=registry();
    expect_error([&] { (void)register_plugin_nodes(separate,mismatched); },"plugin_snapshot_mismatch");
    check(separate.descriptors().size() == baseline,"Bad snapshot changed built-in registry");

    const auto duplicate=snapshot(temp.root/"duplicates.json",{verbatim(package),verbatim(package)});
    auto duplicate_registry=registry();
    const auto duplicate_report=Json::parse(register_plugin_nodes(duplicate_registry,options(duplicate,temp.root)));
    check(duplicate_report["available"].empty() && duplicate_report["errors"].size() == 2 &&
        duplicate_report["errors"][0]["code"] == "plugin_duplicate_id" &&
        duplicate_report["errors"][1]["code"] == "plugin_duplicate_id" &&
        duplicate_registry.descriptors().size() == baseline,
        "Both duplicate packages must be rejected without selecting a first winner");

    const auto bad=temp.root/"corrupt-package";
    copy_package(package,bad);
    { std::ofstream output(bad/"nodes.json",std::ios::binary|std::ios::app); output << "tamper"; }
    const auto broken=snapshot(temp.root/"broken.json",{verbatim(bad)});
    auto broken_registry=registry();
    const auto broken_report=Json::parse(register_plugin_nodes(broken_registry,options(broken,temp.root)));
    check(broken_report["available"].empty() && broken_report["errors"].size() == 1 &&
        broken_registry.descriptors().size() == baseline,"Corrupt static package displaced built-ins");

    std::string long_root="C:\\";
    for (int index=0;index<200;++index) long_root+="\xE4\xB8\xAD"; // valid UTF-8; cut at 512 would split a code point
    const auto unicode_snapshot=temp.root/"unicode-invalid.json";
    { std::ofstream output(unicode_snapshot,std::ios::binary); output << Json({
        {"schema_version",1},{"packages",Json::array({Json{{"root",long_root},{"manifest_sha256",std::string(64,'0')}}})}
    }).dump(); }
    Snapshot unicode{unicode_snapshot,sha256(unicode_snapshot)};
    auto unicode_registry=registry();
    const auto unicode_report=Json::parse(register_plugin_nodes(unicode_registry,options(unicode,temp.root)));
    check(unicode_report["available"].empty() && unicode_report["errors"].size() == 1 &&
        unicode_registry.descriptors().size() == baseline,
        "Long Unicode bad package did not yield a bounded local error");

    for (double floating_version : {1.0,1.9}) {
        const auto path=temp.root/(floating_version == 1.0 ? "float-one.json" : "float-mixed.json");
        { std::ofstream output(path,std::ios::binary); output << Json({
            {"schema_version",floating_version},{"packages",Json::array()}
        }).dump(); }
        auto version_registry=registry();
        expect_error([&] { (void)register_plugin_nodes(version_registry,options({path,sha256(path)},temp.root)); },
            "plugin_snapshot_invalid");
        check(version_registry.descriptors().size() == baseline,"Floating snapshot schema changed registry");
    }
    const auto floating_manifest=temp.root/"floating-manifest";
    copy_package(package,floating_manifest);
    Json manifest_version;
    { std::ifstream input(floating_manifest/"manifest.json",std::ios::binary); input >> manifest_version; }
    manifest_version["schema_version"]=1.9;
    { std::ofstream output(floating_manifest/"manifest.json",std::ios::binary|std::ios::trunc); output << manifest_version.dump(); }
    const auto manifest_snapshot=snapshot(temp.root/"floating-manifest-snapshot.json",{verbatim(floating_manifest)});
    auto manifest_registry=registry();
    const auto manifest_report=Json::parse(register_plugin_nodes(manifest_registry,options(manifest_snapshot,temp.root)));
    check(manifest_report["errors"].size() == 1 && manifest_report["available"].empty() &&
        manifest_registry.descriptors().size() == baseline,"Floating manifest schema was accepted");
    const auto floating_catalog=temp.root/"floating-catalog";
    copy_package(package,floating_catalog);
    Json catalog_version;
    { std::ifstream input(floating_catalog/"nodes.json",std::ios::binary); input >> catalog_version; }
    catalog_version["schema_version"]=1.0;
    { std::ofstream output(floating_catalog/"nodes.json",std::ios::binary|std::ios::trunc); output << catalog_version.dump(); }
    Json catalog_manifest;
    { std::ifstream input(floating_catalog/"manifest.json",std::ios::binary); input >> catalog_manifest; }
    for (auto& file:catalog_manifest["files"]) if (file["path"] == "nodes.json") file["sha256"]=sha256(floating_catalog/"nodes.json");
    { std::ofstream output(floating_catalog/"manifest.json",std::ios::binary|std::ios::trunc); output << catalog_manifest.dump(); }
    const auto catalog_snapshot=snapshot(temp.root/"floating-catalog-snapshot.json",{verbatim(floating_catalog)});
    auto catalog_registry=registry();
    const auto catalog_report=Json::parse(register_plugin_nodes(catalog_registry,options(catalog_snapshot,temp.root)));
    check(catalog_report["errors"].size() == 1 && catalog_report["available"].empty() &&
        catalog_registry.descriptors().size() == baseline,"Floating node catalog schema was accepted");

    const auto verbose=temp.root/"large-catalog";
    copy_package(package,verbose);
    Json catalog;
    { std::ifstream input(verbose/"nodes.json",std::ios::binary); input >> catalog; }
    const auto base_node=catalog["nodes"][0];
    for (int index=0; index<24; ++index) {
        auto node=base_node;
        node["typeId"]="org.audiograph.example.large_"+std::to_string(index);
        node["description"]=std::string(1800,'d');
        catalog["nodes"].push_back(std::move(node));
    }
    { std::ofstream output(verbose/"nodes.json",std::ios::binary|std::ios::trunc); output << catalog.dump(); }
    Json manifest;
    { std::ifstream input(verbose/"manifest.json",std::ios::binary); input >> manifest; }
    for (auto& file:manifest["files"]) if (file["path"] == "nodes.json") file["sha256"]=sha256(verbose/"nodes.json");
    { std::ofstream output(verbose/"manifest.json",std::ios::binary|std::ios::trunc); output << manifest.dump(); }
    const auto oversized=snapshot(temp.root/"oversized.json",{verbatim(verbose)});
    auto oversized_registry=registry();
    const auto oversized_report=Json::parse(register_plugin_nodes(oversized_registry,options(oversized,temp.root)));
    check(oversized_report["available"].empty() && oversized_report["errors"].size() == 1 &&
        oversized_report["errors"][0]["code"] == "plugin_catalog_limit" &&
        oversized_registry.descriptors().size() == baseline,"Complete catalog admission limit did not reject whole package");
}
} // namespace

int main(int argc, char** argv) {
    try {
        check(argc == 2,"Usage: plugin_host_tests ABSOLUTE_PACKAGE_DIRECTORY");
        const auto package=fs::absolute(path_from_utf8(argv[1]));
        check(fs::is_directory(package),"Plugin package directory does not exist");
        run_checks(package);
        std::cout << "plugin host integration tests passed\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "plugin host integration test failed: " << error.what() << '\n';
        return 1;
    }
}
