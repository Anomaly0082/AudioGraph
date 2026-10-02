#include "audioprocess/plugin_host.h"
#include "audioprocess/execution_error.h"
#include "audioprocess/graph_codec.h"
#include <audiograph/plugin.h>
#include <nlohmann/json.hpp>

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cwctype>
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <initializer_list>
#include <limits>
#include <memory>
#include <mutex>
#include <set>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#endif

namespace audioprocess {
namespace {
using Json = nlohmann::json;
namespace fs = std::filesystem;
constexpr std::uint64_t snapshot_limit = 1024 * 1024;
constexpr std::uint64_t manifest_limit = 64 * 1024;
constexpr std::uint64_t nodes_limit = 512 * 1024;
constexpr std::size_t catalog_limit = 32 * 1024;
constexpr std::uint64_t dll_limit = 256ull * 1024 * 1024;
constexpr std::uint64_t payload_limit = 256ull * 1024 * 1024;
constexpr auto run_limit = std::chrono::seconds(60);

[[noreturn]] void fail(std::string code, std::string message) {
    throw ExecutionError(std::move(code),std::move(message));
}
std::string required_string(const Json& object, const char* key, std::size_t maximum = 512) {
    if (!object.is_object() || !object.contains(key) || !object.at(key).is_string())
        fail("plugin_manifest_invalid",std::string("Missing string: ")+key);
    const auto value = object.at(key).get<std::string>();
    if (value.empty() || value.size() > maximum || value.find('\0') != std::string::npos)
        fail("plugin_manifest_invalid",std::string("Invalid string: ")+key);
    return value;
}
bool sha256_text(const std::string& text) {
    return text.size() == 64 && std::all_of(text.begin(),text.end(),[](unsigned char c) {
        return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
    });
}
bool schema_one(const Json& object) {
    if (!object.is_object() || !object.contains("schema_version")) return false;
    const auto& value=object["schema_version"];
    return (value.is_number_integer() || value.is_number_unsigned()) && value == 1;
}
std::string utf8_prefix(const std::string& text, std::size_t limit) {
    if (text.size() <= limit) return text;
    auto end=limit;
    while (end > 0 && (static_cast<unsigned char>(text[end]) & 0xC0u) == 0x80u) --end;
    return text.substr(0,end);
}
bool safe_id(const std::string& text) {
    return !text.empty() && text.size() <= 128 && text != "." && text != ".." &&
        std::all_of(text.begin(),text.end(),[](unsigned char c) {
            return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                   (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-';
        });
}
bool safe_plugin_id(const std::string& text) {
    if (text.empty() || text.size() > 128 || text.front() == '.' || text.back() == '.') return false;
    std::size_t start{};
    while (start < text.size()) {
        const auto end=text.find('.',start);
        const auto length=(end == std::string::npos ? text.size() : end)-start;
        if (!length) return false;
        const auto segment=text.substr(start,length);
        if (!std::all_of(segment.begin(),segment.end(),[](unsigned char c) {
            return (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_' || c == '-';
        })) return false;
        if (segment == "con" || segment == "prn" || segment == "aux" || segment == "nul" ||
            (segment.size() == 4 && ((segment.rfind("com",0) == 0) || (segment.rfind("lpt",0) == 0)) &&
             segment[3] >= '1' && segment[3] <= '9')) return false;
        if (end == std::string::npos) break;
        start=end+1;
    }
    return true;
}
bool leaf_name(const std::string& text) {
    return !text.empty() && text.size() <= 128 && text != "." && text != ".." &&
        text.find_first_of("/\\:") == std::string::npos &&
        text.back() != '.' && text.back() != ' ';
}
Json parse_json(const std::string& text) {
    std::vector<std::set<std::string>> keys;
    auto callback = [&keys](int depth, Json::parse_event_t event, Json& value) {
        if (depth > 48) fail("plugin_json_invalid","Plugin JSON exceeds nesting limit");
        if (event == Json::parse_event_t::object_start) keys.emplace_back();
        if (event == Json::parse_event_t::key && !keys.back().insert(value.get<std::string>()).second)
            fail("plugin_json_invalid","Plugin JSON contains a duplicate key");
        if (event == Json::parse_event_t::object_end) keys.pop_back();
        return true;
    };
    try { return Json::parse(text,callback,true); }
    catch (const ExecutionError&) { throw; }
    catch (...) { fail("plugin_json_invalid","Plugin JSON is invalid UTF-8 or syntax"); }
}
bool valid_utf8(const char* pointer, std::uint64_t length) noexcept {
    if (length && !pointer) return false;
    const auto* bytes = reinterpret_cast<const unsigned char*>(pointer);
    for (std::uint64_t i=0; i<length;) {
        const auto first = bytes[i++];
        if (first < 0x80) continue;
        std::uint64_t count{};
        unsigned char lower=0x80, upper=0xBF;
        if (first >= 0xC2 && first <= 0xDF) count=1;
        else if (first >= 0xE0 && first <= 0xEF) {
            count=2; if (first == 0xE0) lower=0xA0; if (first == 0xED) upper=0x9F;
        } else if (first >= 0xF0 && first <= 0xF4) {
            count=3; if (first == 0xF0) lower=0x90; if (first == 0xF4) upper=0x8F;
        } else return false;
        if (count > length-i || bytes[i] < lower || bytes[i] > upper) return false;
        ++i;
        for (std::uint64_t part=1;part<count;++part,++i)
            if (bytes[i] < 0x80 || bytes[i] > 0xBF) return false;
    }
    return true;
}
std::string utf8_path(const fs::path& path) {
    const auto bytes = path.u8string();
    return {reinterpret_cast<const char*>(bytes.data()),bytes.size()};
}

#ifdef _WIN32
struct Handle {
    HANDLE value{INVALID_HANDLE_VALUE};
    Handle() = default;
    explicit Handle(HANDLE raw):value(raw) {}
    Handle(Handle&& other) noexcept : value(std::exchange(other.value,INVALID_HANDLE_VALUE)) {}
    Handle& operator=(Handle&& other) noexcept {
        if (this != &other) { if (value != INVALID_HANDLE_VALUE) CloseHandle(value); value=std::exchange(other.value,INVALID_HANDLE_VALUE); }
        return *this;
    }
    ~Handle() { if (value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete; Handle& operator=(const Handle&) = delete;
};
bool local_absolute(const fs::path& path) {
    const auto wide = path.native();
    if (!path.is_absolute()) return false;
    const auto drive = [](const std::wstring& text, std::size_t at) {
        return text.size() >= at+3 &&
            ((text[at] >= L'A' && text[at] <= L'Z') || (text[at] >= L'a' && text[at] <= L'z')) &&
            text[at+1] == L':' && (text[at+2] == L'\\' || text[at+2] == L'/');
    };
    if (drive(wide,0)) return true;
    // Rust canonical paths on Windows use the local verbatim drive form.
    return wide.rfind(L"\\\\?\\",0) == 0 && drive(wide,4);
}
void check_path_components(const fs::path& path, bool directory) {
    if (!local_absolute(path)) fail("plugin_path_invalid","Plugin path must be a local absolute path");
    fs::path current = path.root_path();
    for (const auto& component : path.relative_path()) {
        current /= component;
        const auto attributes = GetFileAttributesW(current.c_str());
        if (attributes == INVALID_FILE_ATTRIBUTES || (attributes & FILE_ATTRIBUTE_REPARSE_POINT))
            fail("plugin_path_invalid","Missing or linked plugin path component");
    }
    const auto attributes = GetFileAttributesW(path.c_str());
    if (directory != ((attributes & FILE_ATTRIBUTE_DIRECTORY) != 0))
        fail("plugin_path_invalid","Plugin path has the wrong file kind");
}
Handle open_checked(const fs::path& path, std::uint64_t maximum, bool pin = false) {
    check_path_components(path,false);
    const auto share = FILE_SHARE_READ; // no writer or delete sharing while the handle lives
    Handle file(CreateFileW(path.c_str(),GENERIC_READ,share,nullptr,OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,nullptr));
    if (file.value == INVALID_HANDLE_VALUE) fail("plugin_file_unavailable","Cannot open plugin file without write/delete sharing");
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(file.value,&info) || (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) ||
        (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) || info.nNumberOfLinks != 1)
        fail("plugin_path_invalid","Plugin file is linked, a directory, or inaccessible");
    const auto length = (static_cast<std::uint64_t>(info.nFileSizeHigh)<<32) | info.nFileSizeLow;
    if (length > maximum) fail("plugin_file_too_large","Plugin file exceeds its size limit");
    (void)pin;
    return file;
}
std::string hash_handle(HANDLE file) {
    BCRYPT_ALG_HANDLE algorithm{};
    BCRYPT_HASH_HANDLE hash{};
    if (BCryptOpenAlgorithmProvider(&algorithm,BCRYPT_SHA256_ALGORITHM,nullptr,0) < 0)
        fail("plugin_hash_failed","Cannot initialize SHA-256");
    const auto close_algorithm = [&] { if (hash) BCryptDestroyHash(hash); BCryptCloseAlgorithmProvider(algorithm,0); };
    if (BCryptCreateHash(algorithm,&hash,nullptr,0,nullptr,0,0) < 0) {
        close_algorithm(); fail("plugin_hash_failed","Cannot create SHA-256 state");
    }
    LARGE_INTEGER zero{};
    if (!SetFilePointerEx(file,zero,nullptr,FILE_BEGIN)) { close_algorithm(); fail("plugin_hash_failed","Cannot seek plugin file"); }
    std::array<unsigned char,64*1024> buffer{};
    for (;;) {
        DWORD count{};
        if (!ReadFile(file,buffer.data(),static_cast<DWORD>(buffer.size()),&count,nullptr)) {
            close_algorithm(); fail("plugin_hash_failed","Cannot read plugin file");
        }
        if (!count) break;
        if (BCryptHashData(hash,buffer.data(),count,0) < 0) { close_algorithm(); fail("plugin_hash_failed","Cannot hash plugin file"); }
    }
    std::array<unsigned char,32> digest{};
    if (BCryptFinishHash(hash,digest.data(),static_cast<ULONG>(digest.size()),0) < 0) {
        close_algorithm(); fail("plugin_hash_failed","Cannot finish plugin SHA-256");
    }
    close_algorithm();
    constexpr char hex[] = "0123456789abcdef";
    std::string result; result.reserve(64);
    for (const auto byte : digest) { result.push_back(hex[byte>>4]); result.push_back(hex[byte&15]); }
    return result;
}
std::string read_handle(HANDLE file, std::uint64_t maximum) {
    LARGE_INTEGER size{};
    if (!GetFileSizeEx(file,&size) || size.QuadPart < 0 || static_cast<std::uint64_t>(size.QuadPart) > maximum)
        fail("plugin_file_too_large","Plugin static file exceeds its size limit");
    LARGE_INTEGER zero{};
    if (!SetFilePointerEx(file,zero,nullptr,FILE_BEGIN)) fail("plugin_file_unavailable","Cannot seek plugin file");
    std::string bytes(static_cast<std::size_t>(size.QuadPart),'\0');
    std::size_t offset{};
    while (offset < bytes.size()) {
        DWORD count{};
        const auto wanted = static_cast<DWORD>(std::min<std::size_t>(bytes.size()-offset,64*1024));
        if (!ReadFile(file,bytes.data()+offset,wanted,&count,nullptr) || !count)
            fail("plugin_file_unavailable","Cannot read plugin static file");
        offset += count;
    }
    return bytes;
}
struct StaticFile { std::string bytes; std::string sha; };
StaticFile static_file(const fs::path& path, std::uint64_t maximum) {
    auto handle = open_checked(path,maximum);
    return {read_handle(handle.value,maximum),hash_handle(handle.value)};
}
bool path_within(const fs::path& parent, const fs::path& child) {
    const auto upper = [](std::wstring text) {
        if (text.rfind(L"\\\\?\\",0) == 0) text.erase(0,4);
        std::replace(text.begin(),text.end(),L'/',L'\\');
        std::transform(text.begin(),text.end(),text.begin(),::towupper);
        return text;
    };
    const auto base = upper(parent.lexically_normal().native());
    const auto target = upper(child.lexically_normal().native());
    return target == base || (target.size() > base.size() && target.compare(0,base.size(),base) == 0 &&
        (base.back() == L'\\' || target[base.size()] == L'\\'));
}
#endif

struct Package {
    fs::path root;
    fs::path entry;
    fs::path nodes_path;
    std::string manifest_sha;
    std::string entry_sha;
    std::string nodes_sha;
    std::string nodes_bytes;
    std::string plugin_id;
    std::string plugin_version;
    std::vector<NodeDescriptor> descriptors;
    fs::path data_root;
    fs::path workspace;
};

DataType port_type(const std::string& name) {
    if (name == "Audio") return DataType::Audio;
    if (name == "Number") return DataType::Number;
    if (name == "Text") return DataType::Text;
    fail("plugin_unsupported_type","This version supports only Audio, Number and Text ports");
}
ParameterType parameter_type(const std::string& name) {
    if (name == "number") return ParameterType::Number;
    if (name == "text") return ParameterType::Text;
    if (name == "boolean") return ParameterType::Boolean;
    fail("plugin_unsupported_type","This version does not support FilePath or unknown plugin parameters");
}
void expect_fields(const Json& object, std::initializer_list<const char*> fields) {
    if (!object.is_object()) fail("plugin_manifest_invalid","Expected JSON object");
    for (auto it=object.begin();it!=object.end();++it)
        if (std::find_if(fields.begin(),fields.end(),[&](const char* key){return it.key()==key;}) == fields.end())
            fail("plugin_manifest_invalid","Unknown field in plugin package document");
}
std::vector<PortDescriptor> parse_ports(const Json& value) {
    if (!value.is_array() || value.size() > 32) fail("plugin_descriptor_invalid","Invalid plugin ports");
    std::vector<PortDescriptor> ports;
    std::set<std::string> ids;
    for (const auto& item : value) {
        expect_fields(item,{"id","type","required"});
        auto id=required_string(item,"id",64);
        if (!safe_id(id) || !ids.insert(id).second || !item.contains("required") || !item["required"].is_boolean())
            fail("plugin_descriptor_invalid","Duplicate or invalid plugin port");
        ports.push_back({std::move(id),port_type(required_string(item,"type",32)),item["required"].get<bool>()});
    }
    return ports;
}
std::vector<ParameterDescriptor> parse_parameters(const Json& value) {
    if (!value.is_array() || value.size() > 32) fail("plugin_descriptor_invalid","Invalid plugin parameters");
    std::vector<ParameterDescriptor> parameters;
    std::set<std::string> ids;
    for (const auto& item : value) {
        expect_fields(item,{"id","type","description","required","default","minimum","maximum","unit","enum","integer_only"});
        ParameterDescriptor descriptor{};
        descriptor.id = required_string(item,"id",64);
        if (!safe_id(descriptor.id) || !ids.insert(descriptor.id).second)
            fail("plugin_descriptor_invalid","Duplicate or invalid plugin parameter");
        descriptor.type = parameter_type(required_string(item,"type",32));
        descriptor.description = item.value("description",std::string{});
        if (descriptor.description.size() > 2048 || !item.contains("required") || !item["required"].is_boolean())
            fail("plugin_descriptor_invalid","Invalid plugin parameter description/required");
        descriptor.required = item["required"].get<bool>();
        if (item.contains("minimum")) {
            if (!item["minimum"].is_number()) fail("plugin_descriptor_invalid","Invalid parameter minimum");
            descriptor.minimum = item["minimum"].get<double>();
        }
        if (item.contains("maximum")) {
            if (!item["maximum"].is_number()) fail("plugin_descriptor_invalid","Invalid parameter maximum");
            descriptor.maximum = item["maximum"].get<double>();
        }
        descriptor.unit = item.value("unit",std::string{});
        if (descriptor.unit.size() > 64) fail("plugin_descriptor_invalid","Parameter unit too long");
        if (item.contains("integer_only")) {
            if (!item["integer_only"].is_boolean()) fail("plugin_descriptor_invalid","Invalid integer_only");
            descriptor.integer_only = item["integer_only"].get<bool>();
        }
        if (item.contains("enum")) {
            if (!item["enum"].is_array() || item["enum"].size() > 64) fail("plugin_descriptor_invalid","Invalid parameter enum");
            for (const auto& entry : item["enum"]) {
                if (!entry.is_string()) fail("plugin_descriptor_invalid","Invalid parameter enum entry");
                descriptor.enum_values.push_back(entry.get<std::string>());
            }
        }
        if (item.contains("default")) {
            const auto& default_value=item["default"];
            if (descriptor.type == ParameterType::Number && default_value.is_number()) descriptor.default_value=default_value.get<double>();
            else if (descriptor.type == ParameterType::Text && default_value.is_string()) descriptor.default_value=default_value.get<std::string>();
            else if (descriptor.type == ParameterType::Boolean && default_value.is_boolean()) descriptor.default_value=default_value.get<bool>();
            else fail("plugin_descriptor_invalid","Invalid plugin parameter default");
        }
        parameters.push_back(std::move(descriptor));
    }
    return parameters;
}

NodeDescriptor parse_descriptor(const Json& item, const Package& package) {
    expect_fields(item,{"typeId","displayName","description","execution_domain","stream_role","realtime_role",
        "inputs","outputs","parameters","plugin"});
    if (required_string(item,"execution_domain",32) != "synchronous" ||
        required_string(item,"stream_role",32) != "none" ||
        required_string(item,"realtime_role",32) != "none")
        fail("plugin_unsupported_domain","Only synchronous whole-value plugin nodes are supported");
    const auto& origin=item.at("plugin");
    expect_fields(origin,{"id","implementation_version","capabilities","experimental","production_ready"});
    if (required_string(origin,"id") != package.plugin_id ||
        required_string(origin,"implementation_version") != package.plugin_version ||
        !origin.contains("experimental") || origin["experimental"] != true ||
        !origin.contains("production_ready") || origin["production_ready"] != false ||
        !origin.contains("capabilities") || !origin["capabilities"].is_array() || origin["capabilities"].size() != 1 ||
        origin["capabilities"][0] != Json{{"id",AG_WHOLE_SYNC_ID},{"version",1}})
        fail("plugin_descriptor_invalid","Node plugin metadata differs from package manifest");
    NodeDescriptor descriptor{};
    descriptor.type_id=required_string(item,"typeId",128);
    if (!safe_id(descriptor.type_id) || descriptor.type_id.rfind(package.plugin_id+".",0) != 0)
        fail("plugin_descriptor_invalid","Node type is outside its plugin namespace");
    descriptor.display_name=required_string(item,"displayName",256);
    descriptor.description=required_string(item,"description",2048);
    descriptor.execution_domain=ExecutionDomain::Synchronous;
    descriptor.inputs=parse_ports(item.at("inputs"));
    descriptor.outputs=parse_ports(item.at("outputs"));
    descriptor.parameters=parse_parameters(item.at("parameters"));
    descriptor.plugin=PluginOrigin{package.plugin_id,package.plugin_version,package.manifest_sha,0,1};
    return descriptor;
}

#ifdef _WIN32
Package prepare_package(const Json& snapshot, const PluginHostOptions& options) {
    expect_fields(snapshot,{"root","manifest_sha256"});
    auto root_text=required_string(snapshot,"root",4096);
    if (!valid_utf8(root_text.data(),root_text.size())) fail("plugin_path_invalid","Package root is not UTF-8");
    Package package{};
    package.root=path_from_utf8(root_text);
    check_path_components(package.root,true);
    package.root=fs::canonical(package.root);
    package.manifest_sha=required_string(snapshot,"manifest_sha256",64);
    if (!sha256_text(package.manifest_sha)) fail("plugin_hash_invalid","Invalid manifest fingerprint");
    auto manifest_file=static_file(package.root / "manifest.json",manifest_limit);
    if (manifest_file.sha != package.manifest_sha) fail("plugin_hash_mismatch","Manifest fingerprint changed");
    const auto manifest=parse_json(manifest_file.bytes);
    expect_fields(manifest,{"schema_version","plugin_id","plugin_version","experimental","production_ready",
        "abi","platform","architecture","entry","nodes","capabilities","files"});
    if (!schema_one(manifest) || manifest.value("platform",std::string{}) != "Windows" ||
        manifest.value("architecture",std::string{}) != "x86_64" || !manifest.contains("abi") ||
        manifest["abi"] != Json{{"major",0},{"minor",1}} || !manifest.contains("capabilities") ||
        !manifest.contains("experimental") || manifest["experimental"] != true ||
        !manifest.contains("production_ready") || manifest["production_ready"] != false ||
        manifest["capabilities"] != Json::array({Json{{"id",AG_WHOLE_SYNC_ID},{"version",1}}}))
        fail("plugin_manifest_invalid","Unsupported manifest schema, platform or ABI");
    package.plugin_id=required_string(manifest,"plugin_id",128);
    package.plugin_version=required_string(manifest,"plugin_version",128);
    if (!safe_plugin_id(package.plugin_id) || !safe_id(package.plugin_version))
        fail("plugin_manifest_invalid","Invalid plugin identity");
    const auto entry=required_string(manifest,"entry",128);
    const auto nodes=required_string(manifest,"nodes",128);
    if (!leaf_name(entry) || !leaf_name(nodes) || entry == nodes ||
        fs::path(entry).extension() != ".dll" || nodes != "nodes.json")
        fail("plugin_manifest_invalid","Unsafe plugin entry or nodes filename");
    package.entry=package.root / path_from_utf8(entry);
    package.nodes_path=package.root / path_from_utf8(nodes);
    if (!manifest.contains("files") || !manifest["files"].is_array() || manifest["files"].size() != 2)
        fail("plugin_manifest_invalid","Manifest must hash entry and nodes files");
    std::unordered_map<std::string,std::string> hashes;
    for (const auto& item : manifest["files"]) {
        expect_fields(item,{"path","sha256"});
        const auto path=required_string(item,"path",128);
        const auto hash=required_string(item,"sha256",64);
        if (!leaf_name(path) || !sha256_text(hash) || !hashes.emplace(path,hash).second)
            fail("plugin_manifest_invalid","Duplicate or invalid manifest file");
    }
    if (!hashes.contains(entry) || !hashes.contains(nodes)) fail("plugin_manifest_invalid","Missing hashed package member");
    package.entry_sha=hashes.at(entry);
    package.nodes_sha=hashes.at(nodes);
    if (static_file(package.entry,dll_limit).sha != package.entry_sha)
        fail("plugin_hash_mismatch","Plugin DLL fingerprint changed");
    const auto nodes_file=static_file(package.nodes_path,nodes_limit);
    if (nodes_file.sha != package.nodes_sha) fail("plugin_hash_mismatch","Plugin node description fingerprint changed");
    package.nodes_bytes=nodes_file.bytes;
    const auto catalog=parse_json(package.nodes_bytes);
    expect_fields(catalog,{"schema_version","plugin_id","plugin_version","experimental","production_ready","nodes"});
    if (!schema_one(catalog) || required_string(catalog,"plugin_id") != package.plugin_id ||
        required_string(catalog,"plugin_version") != package.plugin_version || !catalog.contains("nodes") ||
        !catalog.contains("experimental") || catalog["experimental"] != true ||
        !catalog.contains("production_ready") || catalog["production_ready"] != false ||
        !catalog["nodes"].is_array() || catalog["nodes"].empty() || catalog["nodes"].size() > 64)
        fail("plugin_descriptor_invalid","Invalid static node catalog");
    std::set<std::string> ids;
    for (const auto& item : catalog["nodes"]) {
        auto descriptor=parse_descriptor(item,package);
        if (!ids.insert(descriptor.type_id).second) fail("plugin_duplicate_type","Duplicate node type inside package");
        package.descriptors.push_back(std::move(descriptor));
    }
    package.data_root=options.data_root / path_from_utf8(package.plugin_id);
    package.workspace=options.workspace;
    if (!local_absolute(options.data_root) || !local_absolute(options.workspace) ||
        path_within(options.workspace,package.root) || path_within(package.root,options.workspace) ||
        path_within(options.workspace,package.data_root) || path_within(package.root,package.data_root) ||
        path_within(package.data_root,options.workspace) || path_within(package.data_root,package.root))
        fail("plugin_data_root_invalid","Plugin data root overlaps workspace or installation");
    return package;
}

ag_string borrow(const std::string& text) noexcept { return {text.data(),text.size()}; }
std::string copy_bounded(ag_string text, std::uint64_t maximum) {
    if (text.size > maximum || !valid_utf8(text.data,text.size))
        fail("plugin_abi_invalid","Plugin returned oversized or invalid UTF-8 text");
    return text.size ? std::string(text.data,static_cast<std::size_t>(text.size)) : std::string{};
}
struct DescriptionCapture { std::string bytes; unsigned calls{}; };
ag_status AG_CALL receive_description(void* user, ag_string text) noexcept {
    try {
        auto& capture=*static_cast<DescriptionCapture*>(user);
        if (capture.calls++ || text.size > nodes_limit || !valid_utf8(text.data,text.size)) return AG_INVALID_ARGUMENT;
        capture.bytes.assign(text.data ? text.data : "",static_cast<std::size_t>(text.size));
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}

struct PackageRuntime {
    Package package;
    std::mutex mutex;
    bool loaded{};
    std::string load_error;
    HMODULE module{}; // deliberately retained until process exit
    ag_plugin_api api{};
    std::array<Handle,3> pinned;

    explicit PackageRuntime(Package value):package(std::move(value)) {}
    void verify_handles() {
        if (hash_handle(pinned[0].value) != package.manifest_sha ||
            hash_handle(pinned[1].value) != package.nodes_sha ||
            hash_handle(pinned[2].value) != package.entry_sha)
            fail("plugin_hash_mismatch","Pinned plugin package bytes changed");
    }
    void ensure_loaded() {
        std::scoped_lock lock(mutex);
        if (!load_error.empty()) fail("plugin_load_failed",load_error);
        if (loaded) { verify_handles(); return; }
        HMODULE fresh_module{};
        try {
            std::array<Handle,3> files{
                open_checked(package.root / "manifest.json",manifest_limit,true),
                open_checked(package.nodes_path,nodes_limit,true),
                open_checked(package.entry,dll_limit,true)};
            if (hash_handle(files[0].value) != package.manifest_sha ||
                hash_handle(files[1].value) != package.nodes_sha ||
                hash_handle(files[2].value) != package.entry_sha)
                fail("plugin_hash_mismatch","Plugin package changed after registration");
            fresh_module=LoadLibraryExW(package.entry.c_str(),nullptr,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
            if (!fresh_module) fail("plugin_load_failed","LoadLibraryExW could not load trusted plugin DLL");
            const auto address=GetProcAddress(fresh_module,"ag_plugin_get_api");
            if (!address) fail("plugin_abi_invalid","Plugin DLL lacks ag_plugin_get_api");
            const auto entry=reinterpret_cast<ag_get_api_fn>(address);
            ag_plugin_api selected{};
            if (entry(AG_ABI_MAJOR,AG_ABI_MINOR,sizeof(selected),&selected) != AG_OK ||
                selected.struct_size < sizeof(selected) || selected.abi_major != AG_ABI_MAJOR ||
                selected.abi_minor != AG_ABI_MINOR || selected.reserved != 0 ||
                !selected.describe_nodes || !selected.get_node_api ||
                copy_bounded(selected.plugin_id,128) != package.plugin_id ||
                copy_bounded(selected.plugin_version,128) != package.plugin_version)
                fail("plugin_abi_invalid","Plugin ABI or identity differs from verified manifest");
            DescriptionCapture capture{};
            if (selected.describe_nodes(&capture,receive_description) != AG_OK || capture.calls != 1 ||
                capture.bytes != package.nodes_bytes)
                fail("plugin_description_mismatch","Runtime description differs from hashed nodes.json");
            pinned=std::move(files);
            module=fresh_module;
            api=selected;
            loaded=true;
        } catch (const std::exception& error) {
            if (fresh_module) FreeLibrary(fresh_module);
            load_error=error.what();
            throw;
        }
    }
};
std::vector<std::shared_ptr<PackageRuntime>>& process_packages() {
    static auto* packages=new std::vector<std::shared_ptr<PackageRuntime>>;
    return *packages;
}

void prepare_data_root(const Package& package) {
    if (!local_absolute(package.data_root)) fail("plugin_data_root_invalid","Plugin data path must be local absolute");
    fs::path current=package.data_root.root_path();
    for (const auto& component : package.data_root.relative_path()) {
        current/=component;
        auto attributes=GetFileAttributesW(current.c_str());
        if (attributes == INVALID_FILE_ATTRIBUTES) {
            if (!CreateDirectoryW(current.c_str(),nullptr) && GetLastError() != ERROR_ALREADY_EXISTS)
                fail("plugin_data_root_invalid","Cannot create plugin data directory");
            attributes=GetFileAttributesW(current.c_str());
        }
        if (attributes == INVALID_FILE_ATTRIBUTES || !(attributes & FILE_ATTRIBUTE_DIRECTORY) ||
            (attributes & FILE_ATTRIBUTE_REPARSE_POINT))
            fail("plugin_data_root_invalid","Plugin data path contains a link or non-directory");
    }
    check_path_components(package.data_root,true);
    std::error_code error;
    const auto actual=fs::canonical(package.data_root,error);
    if (error || path_within(package.root,actual) || path_within(package.workspace,actual))
        fail("plugin_data_root_invalid","Plugin data root resolves inside workspace or package");
}

struct HostError { std::string code; std::string message; std::string port; std::string parameter; };
ag_status AG_CALL capture_error(void* user, const ag_error* error) noexcept {
    try {
        if (!user || !error) return AG_INVALID_ARGUMENT;
        std::uint64_t remaining=4096;
        for (const auto field : {error->code,error->message,error->port_id,error->parameter_id}) {
            if (field.size > remaining || !valid_utf8(field.data,field.size)) return AG_INVALID_ARGUMENT;
            remaining-=field.size;
        }
        auto& captured=*static_cast<HostError*>(user);
        captured.code=error->code.size ? std::string(error->code.data,error->code.size) : std::string{};
        captured.message=error->message.size ? std::string(error->message.data,error->message.size) : std::string{};
        captured.port=error->port_id.size ? std::string(error->port_id.data,error->port_id.size) : std::string{};
        captured.parameter=error->parameter_id.size ? std::string(error->parameter_id.data,error->parameter_id.size) : std::string{};
        return AG_OK;
    } catch (...) { return AG_INTERNAL_ERROR; }
}
std::string status_code(ag_status status) {
    switch (status) {
    case AG_CANCELLED: return "cancelled";
    case AG_DEADLINE_EXCEEDED: return "plugin_timeout";
    case AG_RESOURCE_LIMIT: return "plugin_resource_limit";
    case AG_INVALID_ARGUMENT: return "plugin_invalid_argument";
    case AG_UNSUPPORTED: return "plugin_unsupported";
    default: return "plugin_execution_failed";
    }
}
[[noreturn]] void status_failure(ag_status status, const HostError& error, const NodeDescriptor& descriptor) {
    const auto code=status_code(status);
    const auto prefix=safe_id(error.code) ? "Plugin " + error.code + ": " : std::string{};
    const auto message=prefix+(error.message.empty() ? "Plugin call did not succeed" : error.message);
    const auto port=std::find_if(descriptor.inputs.begin(),descriptor.inputs.end(),[&](const auto& p){return p.id == error.port;}) != descriptor.inputs.end() ||
        std::find_if(descriptor.outputs.begin(),descriptor.outputs.end(),[&](const auto& p){return p.id == error.port;}) != descriptor.outputs.end()
        ? error.port : std::string{};
    const auto parameter=std::find_if(descriptor.parameters.begin(),descriptor.parameters.end(),
        [&](const auto& p){return p.id == error.parameter;}) != descriptor.parameters.end()
        ? error.parameter : std::string{};
    throw ExecutionError(code,message,{},port,parameter);
}

struct OwnedParameter {
    std::string id;
    std::string text;
    ag_named_value wire{};
};
std::vector<OwnedParameter> prepare_parameters(const NodeDescriptor& descriptor, const ParameterMap& values) {
    std::vector<OwnedParameter> owned;
    owned.reserve(descriptor.parameters.size());
    for (const auto& item : descriptor.parameters) {
        const auto found=values.find(item.id);
        if (found == values.end()) {
            if (item.required) fail("plugin_missing_parameter","Normalized required plugin parameter is missing");
            continue;
        }
        auto& field=owned.emplace_back();
        field.id=item.id;
        if (const auto* number=std::get_if<double>(&found->second)) {
            if (!std::isfinite(*number)) fail("plugin_invalid_parameter","Non-finite plugin parameter");
            field.wire.value.type=AG_NUMBER;
            field.wire.value.data.number=*number;
        } else if (const auto* text=std::get_if<std::string>(&found->second)) {
            if (text->size() > 64*1024 || !valid_utf8(text->data(),text->size()))
                fail("plugin_invalid_parameter","Invalid plugin Text parameter");
            field.text=*text;
            field.wire.value.type=AG_TEXT;
        } else if (const auto* boolean=std::get_if<bool>(&found->second)) {
            field.wire.value.type=AG_BOOLEAN;
            field.wire.value.data.boolean=*boolean ? 1u : 0u;
        } else fail("plugin_unsupported_type","Plugin FilePath parameters are not supported");
    }
    for (auto& field : owned) {
        field.wire.id=borrow(field.id);
        if (field.wire.value.type == AG_TEXT) field.wire.value.data.string=borrow(field.text);
    }
    return owned;
}

struct RunState {
    ExecutionContext* graph{};
    std::chrono::steady_clock::time_point deadline;
};
std::uint32_t AG_CALL cancelled(void* user) noexcept {
    try { return static_cast<RunState*>(user)->graph->cancelled() ? 1u : 0u; }
    catch (...) { return 1u; }
}
std::uint64_t AG_CALL remaining(void* user) noexcept {
    try {
        const auto deadline=static_cast<RunState*>(user)->deadline;
        const auto now=std::chrono::steady_clock::now();
        if (now >= deadline) return 0;
        const auto milliseconds=std::chrono::duration_cast<std::chrono::milliseconds>(deadline-now).count();
        return static_cast<std::uint64_t>(std::max<std::int64_t>(1,milliseconds));
    } catch (...) { return 0; }
}
struct OwnedInput {
    std::string id;
    std::vector<float> samples;
    std::string text;
    ag_named_value wire{};
};
std::vector<OwnedInput> prepare_inputs(const NodeDescriptor& descriptor, const InputValues& inputs) {
    if (inputs.size() > descriptor.inputs.size() || inputs.size() > 32)
        fail("plugin_invalid_input","Unexpected plugin input count");
    std::vector<OwnedInput> owned;
    owned.reserve(inputs.size());
    std::uint64_t total{};
    for (const auto& port : descriptor.inputs) {
        const auto found=inputs.find(port.id);
        if (found == inputs.end()) {
            if (port.required) fail("plugin_missing_input","Required plugin input is absent");
            continue;
        }
        auto& item=owned.emplace_back();
        item.id=port.id;
        if (port.type == DataType::Audio) {
            const auto* audio=std::get_if<AudioClipPtr>(&found->second);
            if (!audio || !*audio || !(*audio)->format.valid() ||
                (*audio)->samples.size() % (*audio)->format.channel_count != 0)
                fail("plugin_invalid_input","Invalid Audio plugin input");
            const auto count=(*audio)->samples.size();
            if (count > payload_limit/sizeof(float) || total > payload_limit-count*sizeof(float))
                fail("plugin_resource_limit","Plugin input byte budget exceeded");
            total+=count*sizeof(float);
            item.samples=(*audio)->samples;
            if (!std::all_of(item.samples.begin(),item.samples.end(),[](float v){return std::isfinite(v);}))
                fail("plugin_invalid_input","Non-finite Audio plugin input");
            item.wire.value.type=AG_AUDIO;
            item.wire.value.data.audio={(*audio)->format.sample_rate,(*audio)->format.channel_count,
                (*audio)->frame_count(),count,item.samples.empty() ? nullptr : item.samples.data()};
        } else if (port.type == DataType::Number) {
            const auto* number=std::get_if<double>(&found->second);
            if (!number || !std::isfinite(*number)) fail("plugin_invalid_input","Invalid Number plugin input");
            if (total > payload_limit-8) fail("plugin_resource_limit","Plugin input byte budget exceeded");
            total+=8; item.wire.value.type=AG_NUMBER; item.wire.value.data.number=*number;
        } else if (port.type == DataType::Text) {
            const auto* text=std::get_if<std::string>(&found->second);
            if (!text || text->size() > payload_limit-total || !valid_utf8(text->data(),text->size()))
                fail("plugin_invalid_input","Invalid Text plugin input");
            total+=text->size(); item.text=*text; item.wire.value.type=AG_TEXT;
        } else fail("plugin_unsupported_type","Plugin FilePath input is unsupported");
    }
    for (auto& item : owned) {
        item.wire.id=borrow(item.id);
        if (item.wire.value.type == AG_TEXT) item.wire.value.data.string=borrow(item.text);
    }
    return owned;
}

struct Staging {
    const NodeDescriptor* descriptor{};
    OutputValues values;
    std::uint64_t bytes{};
    std::uint64_t text_bytes{};
    std::uint64_t max_bytes{payload_limit};
    ag_status rejected{AG_OK};
};
ag_status AG_CALL emit_value(void* user, ag_string id, const ag_value* value) noexcept {
    try {
        auto& staging=*static_cast<Staging*>(user);
        if (staging.rejected != AG_OK) return staging.rejected;
        const auto reject=[&](ag_status status) noexcept { staging.rejected=status; return status; };
        if (!value || value->reserved || id.size > 64 || !valid_utf8(id.data,id.size))
            return reject(AG_INVALID_ARGUMENT);
        const std::string name(id.data ? id.data : "",static_cast<std::size_t>(id.size));
        const auto found=std::find_if(staging.descriptor->outputs.begin(),staging.descriptor->outputs.end(),
            [&](const PortDescriptor& port){return port.id==name;});
        if (found == staging.descriptor->outputs.end() || staging.values.contains(name) ||
            staging.values.size() >= staging.descriptor->outputs.size()) return reject(AG_INVALID_ARGUMENT);
        if (found->type == DataType::Audio && value->type == AG_AUDIO) {
            const auto& audio=value->data.audio;
            if (!audio.sample_rate || !audio.channel_count || audio.channel_count > UINT16_MAX ||
                audio.frame_count > UINT64_MAX/audio.channel_count ||
                audio.sample_count != audio.frame_count*audio.channel_count ||
                audio.sample_count > (staging.max_bytes-staging.bytes)/sizeof(float) ||
                (audio.sample_count && !audio.samples)) return reject(AG_INVALID_ARGUMENT);
            auto clip=std::make_shared<AudioClip>();
            clip->format={audio.sample_rate,static_cast<std::uint16_t>(audio.channel_count)};
            if (audio.sample_count) clip->samples.assign(audio.samples,audio.samples+audio.sample_count);
            if (!std::all_of(clip->samples.begin(),clip->samples.end(),[](float v){return std::isfinite(v);}))
                return reject(AG_INVALID_ARGUMENT);
            staging.bytes+=audio.sample_count*sizeof(float);
            staging.values.emplace(name,AudioClipPtr{std::move(clip)});
        } else if (found->type == DataType::Number && value->type == AG_NUMBER) {
            if (!std::isfinite(value->data.number) || staging.bytes > staging.max_bytes-8)
                return reject(AG_INVALID_ARGUMENT);
            staging.bytes+=8; staging.values.emplace(name,value->data.number);
        } else if (found->type == DataType::Text && value->type == AG_TEXT) {
            const auto& text=value->data.string;
            if (!detail::plugin_text_output_fits(staging.text_bytes,text.size) ||
                text.size > staging.max_bytes-staging.bytes) return reject(AG_RESOURCE_LIMIT);
            if (!valid_utf8(text.data,text.size)) return reject(AG_INVALID_ARGUMENT);
            staging.bytes+=text.size;
            staging.text_bytes+=text.size;
            staging.values.emplace(name,std::string(text.data ? text.data : "",static_cast<std::size_t>(text.size)));
        } else return reject(AG_INVALID_ARGUMENT);
        return AG_OK;
    } catch (...) { if (user) static_cast<Staging*>(user)->rejected=AG_INTERNAL_ERROR; return AG_INTERNAL_ERROR; }
}

class PluginNode final : public ISyncNode {
public:
    PluginNode(std::shared_ptr<PackageRuntime> runtime, NodeDescriptor descriptor, const ParameterMap& parameters)
        : runtime_(std::move(runtime)), descriptor_(std::move(descriptor)) {
        runtime_->ensure_loaded(); // also re-hashes all pinned files for each new instance
        prepare_data_root(runtime_->package);
        const auto& plugin=runtime_->api;
        const std::string capability=AG_WHOLE_SYNC_ID;
        if (plugin.get_node_api(borrow(descriptor_.type_id),borrow(capability),AG_WHOLE_SYNC_VERSION,
            sizeof(api_),&api_) != AG_OK || api_.struct_size < sizeof(api_) ||
            api_.version != AG_WHOLE_SYNC_VERSION || !api_.create || !api_.run || !api_.destroy)
            fail("plugin_abi_invalid","Plugin node function table differs from verified contract");
        const auto owned=prepare_parameters(descriptor_,parameters);
        std::vector<ag_named_value> wires;
        wires.reserve(owned.size());
        for (const auto& field : owned) wires.push_back(field.wire);
        const auto resource=utf8_path(runtime_->package.root);
        const auto data=utf8_path(runtime_->package.data_root);
        ag_instance_environment environment{sizeof(ag_instance_environment),0,borrow(resource),borrow(data)};
        ag_create_info info{sizeof(ag_create_info),0,wires.data(),static_cast<std::uint32_t>(wires.size()),0,&environment};
        HostError error{};
        ag_error_sink sink{sizeof(ag_error_sink),0,&error,capture_error};
        const auto status=api_.create(&info,&sink,&instance_);
        if (status != AG_OK) {
            if (instance_) {
                // A non-OK create never transfers ownership. The pointer may already
                // be freed by the plugin, so invoking destroy here would be unsafe.
                instance_=nullptr;
                fail("plugin_abi_invalid","Failed plugin create returned a non-null instance");
            }
            status_failure(status,error,descriptor_);
        }
        if (!instance_) fail("plugin_abi_invalid","Successful plugin create returned a null instance");
    }
    ~PluginNode() override { if (instance_) api_.destroy(instance_); }
    [[nodiscard]] const NodeDescriptor& descriptor() const noexcept override { return descriptor_; }
    [[nodiscard]] OutputValues execute(const InputValues& inputs, ExecutionContext& context) override {
        const auto owned=prepare_inputs(descriptor_,inputs);
        std::vector<ag_named_value> wires;
        wires.reserve(owned.size());
        for (const auto& item : owned) wires.push_back(item.wire);
        RunState state{&context,std::chrono::steady_clock::now()+run_limit};
        const bool has_audio_output=std::any_of(descriptor_.outputs.begin(),descriptor_.outputs.end(),
            [](const PortDescriptor& port){return port.type == DataType::Audio;});
        const auto output_budget=detail::plugin_call_output_limit(has_audio_output);
        ag_call_context call{sizeof(ag_call_context),0,&state,cancelled,remaining,payload_limit,output_budget,
            static_cast<std::uint32_t>(descriptor_.inputs.size()),static_cast<std::uint32_t>(descriptor_.outputs.size())};
        Staging staging{&descriptor_};
        staging.max_bytes=output_budget;
        ag_output_sink outputs{sizeof(ag_output_sink),0,&staging,emit_value};
        HostError error{};
        ag_error_sink errors{sizeof(ag_error_sink),0,&error,capture_error};
        const auto status=api_.run(instance_,wires.data(),static_cast<std::uint32_t>(wires.size()),&call,&outputs,&errors);
        if (status != AG_OK) status_failure(status,error,descriptor_);
        if (staging.rejected != AG_OK) fail("plugin_output_invalid","Plugin ignored a rejected output callback");
        if (context.cancelled()) fail("cancelled","Plugin execution was cancelled before publication");
        if (remaining(&state) == 0) fail("plugin_timeout","Plugin execution exceeded 60 seconds");
        for (const auto& port : descriptor_.outputs)
            if (port.required && !staging.values.contains(port.id))
                fail("plugin_output_missing","Plugin did not emit a required output");
        return std::move(staging.values);
    }
private:
    std::shared_ptr<PackageRuntime> runtime_;
    NodeDescriptor descriptor_;
    ag_whole_sync_api api_{};
    ag_instance* instance_{};
};
#endif

} // namespace

std::string register_plugin_nodes(NodeRegistry& registry, const PluginHostOptions& options) {
    Json report={{"snapshot_id",options.snapshot_sha256},{"available",Json::array()},{"errors",Json::array()}};
    if (options.snapshot_path.empty()) {
        if (!options.snapshot_sha256.empty()) fail("plugin_snapshot_invalid","Snapshot hash was supplied without a snapshot path");
        return report.dump();
    }
#ifdef _WIN32
    if (!sha256_text(options.snapshot_sha256)) fail("plugin_snapshot_invalid","Missing snapshot SHA-256");
    const auto snapshot_file=static_file(options.snapshot_path,snapshot_limit);
    if (snapshot_file.sha != options.snapshot_sha256) fail("plugin_snapshot_mismatch","Plugin snapshot fingerprint changed");
    const auto snapshot=parse_json(snapshot_file.bytes);
    expect_fields(snapshot,{"schema_version","packages"});
    if (!schema_one(snapshot) || !snapshot.contains("packages") ||
        !snapshot["packages"].is_array() || snapshot["packages"].size() > 16)
        fail("plugin_snapshot_invalid","Invalid plugin snapshot schema or package count");
    struct Prepared { std::string label; Package package; };
    std::vector<Prepared> prepared;
    for (const auto& item : snapshot["packages"]) {
        std::string label="<invalid package>";
        try {
            if (item.is_object() && item.contains("root") && item["root"].is_string())
                label=utf8_prefix(item["root"].get<std::string>(),512);
            prepared.push_back({label,prepare_package(item,options)});
        } catch (const ExecutionError& error) {
            report["errors"].push_back({{"package",label},{"code",error.code},{"message",error.what()}});
        } catch (const std::exception&) {
            report["errors"].push_back({{"package",label},{"code","plugin_package_invalid"},{"message","Package validation failed"}});
        }
    }
    std::unordered_map<std::string,std::size_t> plugin_counts;
    std::unordered_map<std::string,std::size_t> type_counts;
    std::set<std::string> builtins;
    for (const auto& descriptor : registry.descriptors()) builtins.insert(descriptor.type_id);
    for (const auto& item : prepared) {
        ++plugin_counts[item.package.plugin_id];
        for (const auto& descriptor : item.package.descriptors) ++type_counts[descriptor.type_id];
    }
    for (auto& item : prepared) {
        const auto label=item.label;
        try {
            auto& package=item.package;
            if (plugin_counts[package.plugin_id] > 1)
                fail("plugin_duplicate_id","Conflicting plugin identities; none of the involved packages was selected");
            NodeRegistry candidate=registry;
            for (const auto& descriptor : package.descriptors) {
                if (builtins.contains(descriptor.type_id) || type_counts[descriptor.type_id] > 1)
                    fail("plugin_duplicate_type","Conflicting node type; none of the involved packages was selected");
            }
            auto runtime=std::make_shared<PackageRuntime>(std::move(package));
            for (const auto& descriptor : runtime->package.descriptors) {
                candidate.register_type(descriptor,[runtime,descriptor](const ParameterMap& parameters) {
                    return std::make_unique<PluginNode>(runtime,descriptor,parameters);
                });
            }
            if (node_catalog_json(candidate).size() > catalog_limit)
                fail("plugin_catalog_limit","Complete node catalog would exceed 32 KiB; package not registered");
            process_packages().push_back(runtime);
            registry=std::move(candidate);
            report["available"].push_back({{"plugin_id",runtime->package.plugin_id},
                {"plugin_version",runtime->package.plugin_version},{"package_sha256",runtime->package.manifest_sha}});
        } catch (const ExecutionError& error) {
            report["errors"].push_back({{"package",label},{"code",error.code},{"message",error.what()}});
        } catch (const std::exception&) {
            report["errors"].push_back({{"package",label},{"code","plugin_package_invalid"},{"message","Package validation failed"}});
        }
    }
#else
    fail("plugin_platform_unsupported","Plugin ABI 0.1 host is available only on Windows x64");
#endif
    return report.dump();
}

} // namespace audioprocess
