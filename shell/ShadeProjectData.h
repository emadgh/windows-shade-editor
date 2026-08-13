#pragma once

#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace shade_shell {

struct FaceMetadata {
    std::string label;
    std::string source_file_name;
    std::uint32_t width = 0;
    std::uint32_t height = 0;
    std::uint32_t bit_depth = 0;
    std::string color_model;
    std::uint32_t channel_count = 0;
    std::uint32_t base_channel_count = 0;
    double dpi_x = 0.0;
    double dpi_y = 0.0;
};

struct ProjectData {
    std::string name;
    std::uint32_t schema_version = 0;
    std::uint32_t face_count = 0;
    std::uint32_t active_face_index = 0;
    std::uint64_t total_source_bytes = 0;
    std::int64_t saved_at_unix_ms = 0;
    bool has_active_face = false;
    FaceMetadata active_face;
    std::uint32_t thumbnail_width = 0;
    std::uint32_t thumbnail_height = 0;
    std::vector<std::uint8_t> thumbnail_png;
};

bool ParseShadeProject(std::string_view json, ProjectData& out, std::string* error = nullptr);
bool DecodeBase64(std::string_view input, std::vector<std::uint8_t>& out);

}  // namespace shade_shell
