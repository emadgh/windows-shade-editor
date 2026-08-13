#include "ShadeProjectData.h"

#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdlib>
#include <limits>
#include <map>
#include <utility>

namespace shade_shell {
namespace {

enum class JsonKind { Null, Bool, Number, String, Array, Object };

struct JsonValue {
    JsonKind kind = JsonKind::Null;
    bool boolean = false;
    double number = 0.0;
    std::string string;
    std::vector<JsonValue> array;
    std::map<std::string, JsonValue, std::less<>> object;

    const JsonValue* member(std::string_view name) const {
        if (kind != JsonKind::Object) return nullptr;
        auto it = object.find(name);
        return it == object.end() ? nullptr : &it->second;
    }
};

void append_utf8(std::string& out, std::uint32_t codepoint) {
    if (codepoint <= 0x7F) {
        out.push_back(static_cast<char>(codepoint));
    } else if (codepoint <= 0x7FF) {
        out.push_back(static_cast<char>(0xC0 | (codepoint >> 6)));
        out.push_back(static_cast<char>(0x80 | (codepoint & 0x3F)));
    } else if (codepoint <= 0xFFFF) {
        out.push_back(static_cast<char>(0xE0 | (codepoint >> 12)));
        out.push_back(static_cast<char>(0x80 | ((codepoint >> 6) & 0x3F)));
        out.push_back(static_cast<char>(0x80 | (codepoint & 0x3F)));
    } else if (codepoint <= 0x10FFFF) {
        out.push_back(static_cast<char>(0xF0 | (codepoint >> 18)));
        out.push_back(static_cast<char>(0x80 | ((codepoint >> 12) & 0x3F)));
        out.push_back(static_cast<char>(0x80 | ((codepoint >> 6) & 0x3F)));
        out.push_back(static_cast<char>(0x80 | (codepoint & 0x3F)));
    }
}

int hex_value(char ch) {
    if (ch >= '0' && ch <= '9') return ch - '0';
    if (ch >= 'a' && ch <= 'f') return ch - 'a' + 10;
    if (ch >= 'A' && ch <= 'F') return ch - 'A' + 10;
    return -1;
}

class Parser {
public:
    explicit Parser(std::string_view input)
        : begin_(input.data()), current_(input.data()), end_(input.data() + input.size()) {}

    bool parse(JsonValue& value, std::string& error) {
        skip_ws();
        if (!parse_value(value, error)) return false;
        skip_ws();
        if (current_ != end_) {
            error = "Unexpected trailing data in .shade JSON.";
            return false;
        }
        return true;
    }

private:
    const char* begin_;
    const char* current_;
    const char* end_;

    void skip_ws() {
        while (current_ < end_ && std::isspace(static_cast<unsigned char>(*current_))) ++current_;
    }

    bool fail(std::string& error, const char* message) const {
        const auto offset = static_cast<std::size_t>(current_ - begin_);
        error = std::string(message) + " at byte " + std::to_string(offset) + ".";
        return false;
    }

    bool consume(char ch) {
        if (current_ < end_ && *current_ == ch) {
            ++current_;
            return true;
        }
        return false;
    }

    bool literal(std::string_view text) {
        if (static_cast<std::size_t>(end_ - current_) < text.size()) return false;
        if (std::string_view(current_, text.size()) != text) return false;
        current_ += text.size();
        return true;
    }

    bool parse_value(JsonValue& value, std::string& error) {
        skip_ws();
        if (current_ >= end_) return fail(error, "Unexpected end of JSON");
        switch (*current_) {
            case '{': return parse_object(value, error);
            case '[': return parse_array(value, error);
            case '"': {
                value.kind = JsonKind::String;
                return parse_string(value.string, error);
            }
            case 't':
                if (!literal("true")) return fail(error, "Invalid literal");
                value.kind = JsonKind::Bool;
                value.boolean = true;
                return true;
            case 'f':
                if (!literal("false")) return fail(error, "Invalid literal");
                value.kind = JsonKind::Bool;
                value.boolean = false;
                return true;
            case 'n':
                if (!literal("null")) return fail(error, "Invalid literal");
                value.kind = JsonKind::Null;
                return true;
            default:
                return parse_number(value, error);
        }
    }

    bool parse_object(JsonValue& value, std::string& error) {
        if (!consume('{')) return fail(error, "Expected object");
        value.kind = JsonKind::Object;
        value.object.clear();
        skip_ws();
        if (consume('}')) return true;
        while (current_ < end_) {
            skip_ws();
            std::string key;
            if (!parse_string(key, error)) return false;
            skip_ws();
            if (!consume(':')) return fail(error, "Expected ':' after object key");
            JsonValue child;
            if (!parse_value(child, error)) return false;
            value.object.insert_or_assign(std::move(key), std::move(child));
            skip_ws();
            if (consume('}')) return true;
            if (!consume(',')) return fail(error, "Expected ',' between object members");
        }
        return fail(error, "Unterminated object");
    }

    bool parse_array(JsonValue& value, std::string& error) {
        if (!consume('[')) return fail(error, "Expected array");
        value.kind = JsonKind::Array;
        value.array.clear();
        skip_ws();
        if (consume(']')) return true;
        while (current_ < end_) {
            JsonValue child;
            if (!parse_value(child, error)) return false;
            value.array.push_back(std::move(child));
            skip_ws();
            if (consume(']')) return true;
            if (!consume(',')) return fail(error, "Expected ',' between array items");
        }
        return fail(error, "Unterminated array");
    }

    bool parse_string(std::string& out, std::string& error) {
        if (!consume('"')) return fail(error, "Expected JSON string");
        out.clear();
        while (current_ < end_) {
            unsigned char ch = static_cast<unsigned char>(*current_++);
            if (ch == '"') return true;
            if (ch < 0x20) return fail(error, "Control character in JSON string");
            if (ch != '\\') {
                out.push_back(static_cast<char>(ch));
                continue;
            }
            if (current_ >= end_) return fail(error, "Incomplete string escape");
            const char esc = *current_++;
            switch (esc) {
                case '"': out.push_back('"'); break;
                case '\\': out.push_back('\\'); break;
                case '/': out.push_back('/'); break;
                case 'b': out.push_back('\b'); break;
                case 'f': out.push_back('\f'); break;
                case 'n': out.push_back('\n'); break;
                case 'r': out.push_back('\r'); break;
                case 't': out.push_back('\t'); break;
                case 'u': {
                    std::uint32_t first = 0;
                    if (!parse_hex4(first, error)) return false;
                    std::uint32_t codepoint = first;
                    if (first >= 0xD800 && first <= 0xDBFF) {
                        if (end_ - current_ < 6 || current_[0] != '\\' || current_[1] != 'u')
                            return fail(error, "Missing low surrogate");
                        current_ += 2;
                        std::uint32_t second = 0;
                        if (!parse_hex4(second, error)) return false;
                        if (second < 0xDC00 || second > 0xDFFF)
                            return fail(error, "Invalid low surrogate");
                        codepoint = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                    } else if (first >= 0xDC00 && first <= 0xDFFF) {
                        return fail(error, "Unexpected low surrogate");
                    }
                    append_utf8(out, codepoint);
                    break;
                }
                default: return fail(error, "Unsupported string escape");
            }
        }
        return fail(error, "Unterminated string");
    }

    bool parse_hex4(std::uint32_t& value, std::string& error) {
        if (end_ - current_ < 4) return fail(error, "Incomplete Unicode escape");
        value = 0;
        for (int i = 0; i < 4; ++i) {
            int digit = hex_value(*current_++);
            if (digit < 0) return fail(error, "Invalid Unicode escape");
            value = (value << 4) | static_cast<std::uint32_t>(digit);
        }
        return true;
    }

    bool parse_number(JsonValue& value, std::string& error) {
        const char* start = current_;
        if (current_ < end_ && *current_ == '-') ++current_;
        if (current_ >= end_) return fail(error, "Incomplete JSON number");
        if (*current_ == '0') {
            ++current_;
        } else if (*current_ >= '1' && *current_ <= '9') {
            while (current_ < end_ && std::isdigit(static_cast<unsigned char>(*current_))) ++current_;
        } else {
            return fail(error, "Invalid JSON number");
        }
        if (current_ < end_ && *current_ == '.') {
            ++current_;
            if (current_ >= end_ || !std::isdigit(static_cast<unsigned char>(*current_)))
                return fail(error, "Invalid JSON fraction");
            while (current_ < end_ && std::isdigit(static_cast<unsigned char>(*current_))) ++current_;
        }
        if (current_ < end_ && (*current_ == 'e' || *current_ == 'E')) {
            ++current_;
            if (current_ < end_ && (*current_ == '+' || *current_ == '-')) ++current_;
            if (current_ >= end_ || !std::isdigit(static_cast<unsigned char>(*current_)))
                return fail(error, "Invalid JSON exponent");
            while (current_ < end_ && std::isdigit(static_cast<unsigned char>(*current_))) ++current_;
        }
        std::string token(start, current_);
        char* parsed_end = nullptr;
        const double number = std::strtod(token.c_str(), &parsed_end);
        if (!parsed_end || *parsed_end != '\0' || !std::isfinite(number))
            return fail(error, "Invalid JSON numeric value");
        value.kind = JsonKind::Number;
        value.number = number;
        return true;
    }
};

std::string string_or(const JsonValue* value, std::string fallback = {}) {
    return value && value->kind == JsonKind::String ? value->string : std::move(fallback);
}

std::uint64_t uint_or(const JsonValue* value, std::uint64_t fallback = 0) {
    if (!value || value->kind != JsonKind::Number || value->number < 0.0) return fallback;
    const double max_value = static_cast<double>(std::numeric_limits<std::uint64_t>::max());
    if (value->number > max_value) return fallback;
    return static_cast<std::uint64_t>(value->number);
}

std::int64_t int_or(const JsonValue* value, std::int64_t fallback = 0) {
    if (!value || value->kind != JsonKind::Number) return fallback;
    const double min_value = static_cast<double>(std::numeric_limits<std::int64_t>::min());
    const double max_value = static_cast<double>(std::numeric_limits<std::int64_t>::max());
    if (value->number < min_value || value->number > max_value) return fallback;
    return static_cast<std::int64_t>(value->number);
}

double double_or(const JsonValue* value, double fallback = 0.0) {
    return value && value->kind == JsonKind::Number && std::isfinite(value->number)
               ? value->number
               : fallback;
}

std::uint32_t u32_clamped(const JsonValue* value) {
    return static_cast<std::uint32_t>(
        std::min<std::uint64_t>(uint_or(value), std::numeric_limits<std::uint32_t>::max()));
}

bool populate_face(const JsonValue& value, FaceMetadata& face) {
    if (value.kind != JsonKind::Object) return false;
    face.label = string_or(value.member("label"));
    face.source_file_name = string_or(value.member("source_file_name"));
    face.width = u32_clamped(value.member("width"));
    face.height = u32_clamped(value.member("height"));
    face.bit_depth = u32_clamped(value.member("bit_depth"));
    face.color_model = string_or(value.member("color_model"));
    face.channel_count = u32_clamped(value.member("channel_count"));
    face.base_channel_count = u32_clamped(value.member("base_channel_count"));
    face.dpi_x = double_or(value.member("dpi_x"));
    face.dpi_y = double_or(value.member("dpi_y"));
    return true;
}

}  // namespace

bool DecodeBase64(std::string_view input, std::vector<std::uint8_t>& out) {
    signed char table[256];
    std::fill(std::begin(table), std::end(table), static_cast<signed char>(-1));
    for (int i = 0; i < 26; ++i) {
        table[static_cast<unsigned char>('A' + i)] = static_cast<signed char>(i);
        table[static_cast<unsigned char>('a' + i)] = static_cast<signed char>(26 + i);
    }
    for (int i = 0; i < 10; ++i)
        table[static_cast<unsigned char>('0' + i)] = static_cast<signed char>(52 + i);
    table[static_cast<unsigned char>('+')] = 62;
    table[static_cast<unsigned char>('/')] = 63;
    table[static_cast<unsigned char>('=')] = -2;

    out.clear();
    out.reserve(input.size() * 3 / 4);
    int quartet[4];
    int q = 0;
    bool saw_padding = false;
    for (char raw : input) {
        const unsigned char ch = static_cast<unsigned char>(raw);
        if (std::isspace(ch)) continue;
        const int value = table[ch];
        if (value == -1) return false;
        if (saw_padding && value != -2) return false;
        if (value == -2) saw_padding = true;
        quartet[q++] = value;
        if (q != 4) continue;
        if (quartet[0] < 0 || quartet[1] < 0) return false;
        out.push_back(static_cast<std::uint8_t>((quartet[0] << 2) | (quartet[1] >> 4)));
        if (quartet[2] == -2) {
            if (quartet[3] != -2) return false;
        } else {
            if (quartet[2] < 0) return false;
            out.push_back(static_cast<std::uint8_t>(((quartet[1] & 0x0F) << 4) | (quartet[2] >> 2)));
            if (quartet[3] != -2) {
                if (quartet[3] < 0) return false;
                out.push_back(static_cast<std::uint8_t>(((quartet[2] & 0x03) << 6) | quartet[3]));
            }
        }
        q = 0;
    }
    return q == 0;
}

bool ParseShadeProject(std::string_view json, ProjectData& out, std::string* error) {
    ProjectData parsed;
    JsonValue root;
    std::string local_error;
    Parser parser(json);
    if (!parser.parse(root, local_error) || root.kind != JsonKind::Object) {
        if (local_error.empty()) local_error = "Root .shade JSON value is not an object.";
        if (error) *error = local_error;
        return false;
    }

    parsed.schema_version = u32_clamped(root.member("schema_version"));
    if (parsed.schema_version != 9) {
        if (error) *error = "Shade shell integration supports .shade schema 9 only.";
        return false;
    }
    parsed.name = string_or(root.member("name"), "Shade Editor Project");

    const JsonValue* faces = root.member("faces");
    if (faces && faces->kind == JsonKind::Array)
        parsed.face_count = static_cast<std::uint32_t>(std::min<std::size_t>(
            faces->array.size(), std::numeric_limits<std::uint32_t>::max()));

    const JsonValue* metadata = root.member("file_metadata");
    if (metadata && metadata->kind == JsonKind::Object) {
        parsed.saved_at_unix_ms = int_or(metadata->member("saved_at_unix_ms"));
        parsed.face_count = u32_clamped(metadata->member("face_count"));
        parsed.active_face_index = u32_clamped(metadata->member("active_face_index"));
        parsed.total_source_bytes = uint_or(metadata->member("total_source_bytes"));
        const JsonValue* metadata_faces = metadata->member("faces");
        if (metadata_faces && metadata_faces->kind == JsonKind::Array &&
            parsed.active_face_index < metadata_faces->array.size()) {
            parsed.has_active_face =
                populate_face(metadata_faces->array[parsed.active_face_index], parsed.active_face);
        }
    }

    const JsonValue* thumbnail = root.member("thumbnail");
    if (thumbnail && thumbnail->kind == JsonKind::Object) {
        const std::string mime = string_or(thumbnail->member("mime_type"));
        const std::string encoded = string_or(thumbnail->member("data_base64"));
        parsed.thumbnail_width = u32_clamped(thumbnail->member("width"));
        parsed.thumbnail_height = u32_clamped(thumbnail->member("height"));
        if (mime == "image/png" && !encoded.empty()) {
            if (!DecodeBase64(encoded, parsed.thumbnail_png)) {
                if (error) *error = "Embedded .shade thumbnail has invalid base64 data.";
                return false;
            }
        }
    }

    out = std::move(parsed);
    if (error) error->clear();
    return true;
}

}  // namespace shade_shell
