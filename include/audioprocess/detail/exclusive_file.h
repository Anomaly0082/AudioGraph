#pragma once

#include <cstdio>
#include <cstdint>
#include <filesystem>
#include <stdexcept>
#include <string>

#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#include <share.h>
#include <sys/stat.h>
#else
#include <fcntl.h>
#include <unistd.h>
#endif

namespace audioprocess::detail {

inline std::string path_utf8(const std::filesystem::path& path) {
    const auto encoded = path.u8string();
    return {reinterpret_cast<const char*>(encoded.data()), encoded.size()};
}

// 独占创建文件，避免“检查存在后再截断”的竞态。失败时保留部分产物，绝不删除已有文件。
class ExclusiveFile {
public:
    explicit ExclusiveFile(const std::filesystem::path& path) {
        if (path.empty() || path.native().find(std::filesystem::path::value_type{}) !=
                                std::filesystem::path::string_type::npos) {
            throw std::invalid_argument("Output path must be nonempty and contain no NUL characters");
        }
        int descriptor = -1;
#ifdef _WIN32
        const auto error = _wsopen_s(&descriptor, path.c_str(),
            _O_WRONLY | _O_CREAT | _O_EXCL | _O_BINARY, _SH_DENYWR, _S_IREAD | _S_IWRITE);
        if (error == 0) {
            file_ = _fdopen(descriptor, "wb");
            if (!file_) { _close(descriptor); }
        }
#else
        descriptor = ::open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666);
        if (descriptor >= 0) {
            file_ = ::fdopen(descriptor, "wb");
            if (!file_) { ::close(descriptor); }
        }
#endif
        if (!file_) {
            throw std::runtime_error("Cannot create output file (existing files are not overwritten): " + path_utf8(path));
        }
    }
    ~ExclusiveFile() { if (file_) { std::fclose(file_); } }
    ExclusiveFile(const ExclusiveFile&) = delete;
    ExclusiveFile& operator=(const ExclusiveFile&) = delete;

    void write(const char* data, std::size_t size) {
        if (size != 0 && std::fwrite(data, 1, size, file_) != size) {
            throw std::runtime_error("Failed to write output file; partial output may remain");
        }
    }
    void seek(std::uint64_t offset) {
#ifdef _WIN32
        const auto result = _fseeki64(file_, static_cast<__int64>(offset), SEEK_SET);
#else
        const auto result = ::fseeko(file_, static_cast<off_t>(offset), SEEK_SET);
#endif
        if (result != 0) { throw std::runtime_error("Failed to seek output file"); }
    }
    void flush() {
        if (std::fflush(file_) != 0) {
            throw std::runtime_error("Failed to flush output file; partial output may remain");
        }
    }
private:
    std::FILE* file_{};
};

}  // namespace audioprocess::detail
