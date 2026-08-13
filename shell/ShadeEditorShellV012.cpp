#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <shobjidl_core.h>
#include <thumbcache.h>
#include <propsys.h>
#include <propkey.h>
#include <propvarutil.h>
#include <shlwapi.h>
#include <wincodec.h>
#include <wrl/client.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <new>
#include <string>
#include <vector>

#include "ShadeProjectData.h"

#pragma comment(linker, "/EXPORT:DllGetClassObject")
#pragma comment(linker, "/EXPORT:DllCanUnloadNow")

using Microsoft::WRL::ComPtr;
using shade_shell::ProjectData;

namespace {
std::atomic<long> g_objects{0};
std::atomic<long> g_locks{0};

const CLSID CLSID_ShadeEditorShell = {
    0x6f49f9d5, 0x0f3a, 0x4bf0, {0x8c, 0x74, 0x8a, 0x59, 0x95, 0x1a, 0x75, 0xd2}};
const GUID FMTID_ShadeEditor = {
    0xe1486a27, 0x9a7b, 0x4e56, {0xbb, 0xd1, 0x50, 0xd7, 0xf0, 0x1c, 0x17, 0x78}};

const PROPERTYKEY PKEY_Shade_FaceCount = {FMTID_ShadeEditor, 2};
const PROPERTYKEY PKEY_Shade_ActiveFace = {FMTID_ShadeEditor, 3};
const PROPERTYKEY PKEY_Shade_TotalSourceBytes = {FMTID_ShadeEditor, 4};
const PROPERTYKEY PKEY_Shade_SavedAt = {FMTID_ShadeEditor, 5};
const PROPERTYKEY PKEY_Shade_PhysicalWidthCm = {FMTID_ShadeEditor, 10};
const PROPERTYKEY PKEY_Shade_PhysicalHeightCm = {FMTID_ShadeEditor, 11};
const PROPERTYKEY PKEY_Shade_PixelWidth = {FMTID_ShadeEditor, 12};
const PROPERTYKEY PKEY_Shade_PixelHeight = {FMTID_ShadeEditor, 13};
const PROPERTYKEY PKEY_Shade_DpiX = {FMTID_ShadeEditor, 14};
const PROPERTYKEY PKEY_Shade_DpiY = {FMTID_ShadeEditor, 15};
const PROPERTYKEY PKEY_Shade_BitDepth = {FMTID_ShadeEditor, 16};
const PROPERTYKEY PKEY_Shade_ColorModel = {FMTID_ShadeEditor, 17};
const PROPERTYKEY PKEY_Shade_ChannelCount = {FMTID_ShadeEditor, 18};
const PROPERTYKEY PKEY_Shade_BaseChannelCount = {FMTID_ShadeEditor, 19};
const PROPERTYKEY PKEY_Shade_SourceFileName = {FMTID_ShadeEditor, 20};
const PROPERTYKEY PKEY_Shade_PhysicalDimensions = {FMTID_ShadeEditor, 21};
const PROPERTYKEY PKEY_Shade_PixelDimensions = {FMTID_ShadeEditor, 22};
const PROPERTYKEY PKEY_Shade_Dpi = {FMTID_ShadeEditor, 23};

const PROPERTYKEY kProperties[] = {
    PKEY_Title, PKEY_Shade_FaceCount, PKEY_Shade_ActiveFace,
    PKEY_Shade_TotalSourceBytes, PKEY_Shade_SavedAt,
    PKEY_Shade_PhysicalWidthCm, PKEY_Shade_PhysicalHeightCm,
    PKEY_Shade_PixelWidth, PKEY_Shade_PixelHeight,
    PKEY_Shade_DpiX, PKEY_Shade_DpiY, PKEY_Shade_BitDepth,
    PKEY_Shade_ColorModel, PKEY_Shade_ChannelCount,
    PKEY_Shade_BaseChannelCount, PKEY_Shade_SourceFileName,
    PKEY_Shade_PhysicalDimensions, PKEY_Shade_PixelDimensions, PKEY_Shade_Dpi,
};

bool key_equal(REFPROPERTYKEY a, REFPROPERTYKEY b) {
    return a.pid == b.pid && IsEqualGUID(a.fmtid, b.fmtid);
}

std::wstring utf8_to_wide(const std::string& text) {
    if (text.empty()) return {};
    int count = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, text.data(),
                                    static_cast<int>(text.size()), nullptr, 0);
    if (count <= 0) count = MultiByteToWideChar(CP_UTF8, 0, text.data(),
                                                static_cast<int>(text.size()), nullptr, 0);
    if (count <= 0) return {};
    std::wstring out(static_cast<size_t>(count), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(), count);
    return out;
}

HRESULT read_stream(IStream* stream, std::string& text) {
    if (!stream) return E_POINTER;
    LARGE_INTEGER zero{};
    HRESULT hr = stream->Seek(zero, STREAM_SEEK_SET, nullptr);
    if (FAILED(hr)) return hr;
    text.clear();
    char buffer[65536];
    constexpr size_t kMaxBytes = 64u * 1024u * 1024u;
    for (;;) {
        ULONG read = 0;
        hr = stream->Read(buffer, static_cast<ULONG>(sizeof(buffer)), &read);
        if (FAILED(hr)) return hr;
        if (!read) break;
        if (text.size() + read > kMaxBytes) return HRESULT_FROM_WIN32(ERROR_FILE_TOO_LARGE);
        text.append(buffer, buffer + read);
        if (hr == S_FALSE) break;
    }
    return S_OK;
}

HRESULT string_variant(const std::string& text, PROPVARIANT* out) {
    const auto wide = utf8_to_wide(text);
    return InitPropVariantFromString(wide.c_str(), out);
}
HRESULT wide_variant(const std::wstring& text, PROPVARIANT* out) {
    return InitPropVariantFromString(text.c_str(), out);
}

FILETIME unix_ms_to_filetime(std::int64_t unix_ms) {
    constexpr std::int64_t offset = 11644473600000LL;
    const ULONGLONG ticks = unix_ms > -offset
        ? static_cast<ULONGLONG>(unix_ms + offset) * 10000ULL : 0ULL;
    FILETIME value{};
    value.dwLowDateTime = static_cast<DWORD>(ticks);
    value.dwHighDateTime = static_cast<DWORD>(ticks >> 32);
    return value;
}

std::wstring number_text(double value, int precision = 2) {
    wchar_t buffer[64]{};
    swprintf_s(buffer, L"%.*f", precision, value);
    std::wstring text(buffer);
    while (text.size() > 1 && text.back() == L'0') text.pop_back();
    if (!text.empty() && text.back() == L'.') text.pop_back();
    return text;
}

std::wstring physical_dimensions(const ProjectData& data) {
    if (!data.has_active_face || data.active_face.dpi_x <= 0.0 || data.active_face.dpi_y <= 0.0) return {};
    const double w = static_cast<double>(data.active_face.width) / data.active_face.dpi_x * 2.54;
    const double h = static_cast<double>(data.active_face.height) / data.active_face.dpi_y * 2.54;
    return number_text(w) + L" x " + number_text(h) + L" cm";
}
std::wstring pixel_dimensions(const ProjectData& data) {
    if (!data.has_active_face) return {};
    return std::to_wstring(data.active_face.width) + L" x " + std::to_wstring(data.active_face.height) + L" px";
}
std::wstring dpi_dimensions(const ProjectData& data) {
    if (!data.has_active_face || data.active_face.dpi_x <= 0.0 || data.active_face.dpi_y <= 0.0) return {};
    return number_text(data.active_face.dpi_x, 1) + L" x " + number_text(data.active_face.dpi_y, 1) + L" DPI";
}

HRESULT thumbnail_from_png(const std::vector<std::uint8_t>& png, UINT requested,
                           HBITMAP* bitmap, WTS_ALPHATYPE* alpha_type) {
    if (!bitmap || !alpha_type) return E_POINTER;
    *bitmap = nullptr;
    *alpha_type = WTSAT_UNKNOWN;
    if (png.empty() || png.size() > MAXUINT) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);

    ComPtr<IStream> stream;
    stream.Attach(SHCreateMemStream(png.data(), static_cast<UINT>(png.size())));
    if (!stream) return E_OUTOFMEMORY;
    ComPtr<IWICImagingFactory> factory;
    HRESULT hr = CoCreateInstance(CLSID_WICImagingFactory, nullptr, CLSCTX_INPROC_SERVER,
                                  IID_PPV_ARGS(&factory));
    if (FAILED(hr)) return hr;
    ComPtr<IWICBitmapDecoder> decoder;
    hr = factory->CreateDecoderFromStream(stream.Get(), nullptr, WICDecodeMetadataCacheOnLoad, &decoder);
    if (FAILED(hr)) return hr;
    ComPtr<IWICBitmapFrameDecode> frame;
    hr = decoder->GetFrame(0, &frame);
    if (FAILED(hr)) return hr;
    UINT sw = 0, sh = 0;
    hr = frame->GetSize(&sw, &sh);
    if (FAILED(hr) || !sw || !sh) return FAILED(hr) ? hr : E_FAIL;

    const UINT limit = requested ? requested : 256;
    const double scale = (std::min)(1.0, (std::min)(static_cast<double>(limit) / sw,
                                                   static_cast<double>(limit) / sh));
    const UINT tw = (std::max)<UINT>(1, static_cast<UINT>(sw * scale + 0.5));
    const UINT th = (std::max)<UINT>(1, static_cast<UINT>(sh * scale + 0.5));
    IWICBitmapSource* source = frame.Get();
    ComPtr<IWICBitmapScaler> scaler;
    if (tw != sw || th != sh) {
        hr = factory->CreateBitmapScaler(&scaler);
        if (FAILED(hr)) return hr;
        hr = scaler->Initialize(frame.Get(), tw, th, WICBitmapInterpolationModeFant);
        if (FAILED(hr)) return hr;
        source = scaler.Get();
    }
    ComPtr<IWICFormatConverter> converter;
    hr = factory->CreateFormatConverter(&converter);
    if (FAILED(hr)) return hr;
    hr = converter->Initialize(source, GUID_WICPixelFormat32bppPBGRA,
                               WICBitmapDitherTypeNone, nullptr, 0.0, WICBitmapPaletteTypeCustom);
    if (FAILED(hr)) return hr;

    BITMAPINFO info{};
    info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    info.bmiHeader.biWidth = static_cast<LONG>(tw);
    info.bmiHeader.biHeight = -static_cast<LONG>(th);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    void* pixels = nullptr;
    HBITMAP dib = CreateDIBSection(nullptr, &info, DIB_RGB_COLORS, &pixels, nullptr, 0);
    if (!dib || !pixels) return HRESULT_FROM_WIN32(GetLastError());
    const UINT stride = tw * 4;
    hr = converter->CopyPixels(nullptr, stride, stride * th, static_cast<BYTE*>(pixels));
    if (FAILED(hr)) { DeleteObject(dib); return hr; }
    *bitmap = dib;
    *alpha_type = WTSAT_ARGB;
    return S_OK;
}

class ShadeShellHandler final : public IInitializeWithStream, public IThumbnailProvider, public IPropertyStore {
public:
    ShadeShellHandler() { ++g_objects; }
    ~ShadeShellHandler() { --g_objects; }

    IFACEMETHODIMP QueryInterface(REFIID iid, void** object) override {
        if (!object) return E_POINTER;
        *object = nullptr;
        if (IsEqualIID(iid, IID_IUnknown) || IsEqualIID(iid, IID_IInitializeWithStream))
            *object = static_cast<IInitializeWithStream*>(this);
        else if (IsEqualIID(iid, IID_IThumbnailProvider))
            *object = static_cast<IThumbnailProvider*>(this);
        else if (IsEqualIID(iid, IID_IPropertyStore))
            *object = static_cast<IPropertyStore*>(this);
        else return E_NOINTERFACE;
        AddRef();
        return S_OK;
    }
    IFACEMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&refs_)); }
    IFACEMETHODIMP_(ULONG) Release() override {
        const ULONG count = static_cast<ULONG>(InterlockedDecrement(&refs_));
        if (!count) delete this;
        return count;
    }
    IFACEMETHODIMP Initialize(IStream* stream, DWORD) override {
        if (!stream) return E_POINTER;
        if (stream_) return HRESULT_FROM_WIN32(ERROR_ALREADY_INITIALIZED);
        stream_ = stream;
        return S_OK;
    }
    IFACEMETHODIMP GetThumbnail(UINT cx, HBITMAP* bitmap, WTS_ALPHATYPE* alpha_type) override {
        const HRESULT hr = ensure_data();
        return FAILED(hr) ? hr : thumbnail_from_png(data_.thumbnail_png, cx, bitmap, alpha_type);
    }
    IFACEMETHODIMP GetCount(DWORD* count) override {
        if (!count) return E_POINTER;
        *count = static_cast<DWORD>(_countof(kProperties));
        return S_OK;
    }
    IFACEMETHODIMP GetAt(DWORD index, PROPERTYKEY* key) override {
        if (!key) return E_POINTER;
        if (index >= _countof(kProperties)) return E_INVALIDARG;
        *key = kProperties[index];
        return S_OK;
    }
    IFACEMETHODIMP GetValue(REFPROPERTYKEY key, PROPVARIANT* value) override {
        if (!value) return E_POINTER;
        PropVariantInit(value);
        const HRESULT hr = ensure_data();
        if (FAILED(hr)) return hr;
        if (key_equal(key, PKEY_Title)) return string_variant(data_.name, value);
        if (key_equal(key, PKEY_Shade_FaceCount)) { value->vt = VT_UI4; value->ulVal = data_.face_count; return S_OK; }
        if (key_equal(key, PKEY_Shade_ActiveFace)) {
            const std::string name = data_.has_active_face
                ? (!data_.active_face.label.empty() ? data_.active_face.label : data_.active_face.source_file_name)
                : std::string{};
            return string_variant(name, value);
        }
        if (key_equal(key, PKEY_Shade_TotalSourceBytes)) { value->vt = VT_UI8; value->uhVal.QuadPart = data_.total_source_bytes; return S_OK; }
        if (key_equal(key, PKEY_Shade_SavedAt)) {
            if (data_.saved_at_unix_ms <= 0) return S_OK;
            value->vt = VT_FILETIME; value->filetime = unix_ms_to_filetime(data_.saved_at_unix_ms); return S_OK;
        }
        if (!data_.has_active_face) return S_OK;
        if (key_equal(key, PKEY_Shade_PhysicalWidthCm) || key_equal(key, PKEY_Shade_PhysicalHeightCm)) {
            const bool x = key_equal(key, PKEY_Shade_PhysicalWidthCm);
            const double dpi = x ? data_.active_face.dpi_x : data_.active_face.dpi_y;
            const std::uint32_t pixels = x ? data_.active_face.width : data_.active_face.height;
            if (dpi <= 0.0) return S_OK;
            value->vt = VT_R8; value->dblVal = static_cast<double>(pixels) / dpi * 2.54; return S_OK;
        }
        if (key_equal(key, PKEY_Shade_PixelWidth)) { value->vt = VT_UI4; value->ulVal = data_.active_face.width; return S_OK; }
        if (key_equal(key, PKEY_Shade_PixelHeight)) { value->vt = VT_UI4; value->ulVal = data_.active_face.height; return S_OK; }
        if (key_equal(key, PKEY_Shade_DpiX)) { value->vt = VT_R8; value->dblVal = data_.active_face.dpi_x; return S_OK; }
        if (key_equal(key, PKEY_Shade_DpiY)) { value->vt = VT_R8; value->dblVal = data_.active_face.dpi_y; return S_OK; }
        if (key_equal(key, PKEY_Shade_BitDepth)) { value->vt = VT_UI4; value->ulVal = data_.active_face.bit_depth; return S_OK; }
        if (key_equal(key, PKEY_Shade_ColorModel)) return string_variant(data_.active_face.color_model, value);
        if (key_equal(key, PKEY_Shade_ChannelCount)) { value->vt = VT_UI4; value->ulVal = data_.active_face.channel_count; return S_OK; }
        if (key_equal(key, PKEY_Shade_BaseChannelCount)) { value->vt = VT_UI4; value->ulVal = data_.active_face.base_channel_count; return S_OK; }
        if (key_equal(key, PKEY_Shade_SourceFileName)) return string_variant(data_.active_face.source_file_name, value);
        if (key_equal(key, PKEY_Shade_PhysicalDimensions)) return wide_variant(physical_dimensions(data_), value);
        if (key_equal(key, PKEY_Shade_PixelDimensions)) return wide_variant(pixel_dimensions(data_), value);
        if (key_equal(key, PKEY_Shade_Dpi)) return wide_variant(dpi_dimensions(data_), value);
        return S_OK;
    }
    IFACEMETHODIMP SetValue(REFPROPERTYKEY, REFPROPVARIANT) override { return STG_E_ACCESSDENIED; }
    IFACEMETHODIMP Commit() override { return STG_E_ACCESSDENIED; }

private:
    HRESULT ensure_data() {
        if (parsed_) return parse_result_;
        parsed_ = true;
        if (!stream_) return parse_result_ = E_UNEXPECTED;
        std::string text;
        parse_result_ = read_stream(stream_.Get(), text);
        if (FAILED(parse_result_)) return parse_result_;
        std::string error;
        if (!shade_shell::ParseShadeProject(text, data_, &error))
            return parse_result_ = HRESULT_FROM_WIN32(ERROR_BAD_FORMAT);
        return parse_result_ = S_OK;
    }
    LONG refs_ = 1;
    ComPtr<IStream> stream_;
    bool parsed_ = false;
    HRESULT parse_result_ = E_PENDING;
    ProjectData data_;
};

class ShadeClassFactory final : public IClassFactory {
public:
    ShadeClassFactory() { ++g_objects; }
    ~ShadeClassFactory() { --g_objects; }
    IFACEMETHODIMP QueryInterface(REFIID iid, void** object) override {
        if (!object) return E_POINTER;
        *object = nullptr;
        if (!IsEqualIID(iid, IID_IUnknown) && !IsEqualIID(iid, IID_IClassFactory)) return E_NOINTERFACE;
        *object = static_cast<IClassFactory*>(this); AddRef(); return S_OK;
    }
    IFACEMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&refs_)); }
    IFACEMETHODIMP_(ULONG) Release() override {
        const ULONG count = static_cast<ULONG>(InterlockedDecrement(&refs_));
        if (!count) delete this;
        return count;
    }
    IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** object) override {
        if (outer) return CLASS_E_NOAGGREGATION;
        if (!object) return E_POINTER;
        *object = nullptr;
        auto* handler = new (std::nothrow) ShadeShellHandler();
        if (!handler) return E_OUTOFMEMORY;
        const HRESULT hr = handler->QueryInterface(iid, object);
        handler->Release();
        return hr;
    }
    IFACEMETHODIMP LockServer(BOOL lock) override { if (lock) ++g_locks; else --g_locks; return S_OK; }
private:
    LONG refs_ = 1;
};
}  // namespace

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) DisableThreadLibraryCalls(instance);
    return TRUE;
}

STDAPI DllCanUnloadNow(void) {
    return g_objects.load() == 0 && g_locks.load() == 0 ? S_OK : S_FALSE;
}

STDAPI DllGetClassObject(REFCLSID clsid, REFIID iid, LPVOID* object) {
    if (!object) return E_POINTER;
    *object = nullptr;
    if (!IsEqualCLSID(clsid, CLSID_ShadeEditorShell)) return CLASS_E_CLASSNOTAVAILABLE;
    auto* factory = new (std::nothrow) ShadeClassFactory();
    if (!factory) return E_OUTOFMEMORY;
    const HRESULT hr = factory->QueryInterface(iid, object);
    factory->Release();
    return hr;
}
