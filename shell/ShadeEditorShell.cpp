#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <thumbcache.h>
#include <propsys.h>
#include <propkey.h>
#include <propvarutil.h>
#include <shlwapi.h>
#include <wincodec.h>
#include <wrl/client.h>

#include <atomic>
#include <new>
#include <string>
#include <vector>

#include "ShadeProjectData.h"

using Microsoft::WRL::ComPtr;
using shade_shell::ProjectData;

namespace {
HMODULE g_module = nullptr;
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

bool key_equal(REFPROPERTYKEY a, REFPROPERTYKEY b) {
    return a.pid == b.pid && IsEqualGUID(a.fmtid, b.fmtid);
}

std::wstring utf8_to_wide(const std::string& text) {
    if (text.empty()) return {};
    int size = MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), nullptr, 0);
    if (size <= 0) return {};
    std::wstring out(static_cast<size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(), size);
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

HRESULT string_variant(const std::string& value, PROPVARIANT* out) {
    std::wstring wide = utf8_to_wide(value);
    return InitPropVariantFromString(wide.c_str(), out);
}

FILETIME unix_ms_to_filetime(std::int64_t unix_ms) {
    constexpr std::int64_t kEpochOffsetMs = 11644473600000ll;
    ULONGLONG ticks = unix_ms > -kEpochOffsetMs
        ? static_cast<ULONGLONG>(unix_ms + kEpochOffsetMs) * 10000ull : 0;
    FILETIME ft{};
    ft.dwLowDateTime = static_cast<DWORD>(ticks);
    ft.dwHighDateTime = static_cast<DWORD>(ticks >> 32);
    return ft;
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

    UINT limit = requested ? requested : 256;
    double scale = std::min(1.0, std::min(static_cast<double>(limit) / sw,
                                         static_cast<double>(limit) / sh));
    UINT tw = std::max<UINT>(1, static_cast<UINT>(sw * scale + 0.5));
    UINT th = std::max<UINT>(1, static_cast<UINT>(sh * scale + 0.5));
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
    UINT stride = tw * 4;
    hr = converter->CopyPixels(nullptr, stride, stride * th, static_cast<BYTE*>(pixels));
    if (FAILED(hr)) {
        DeleteObject(dib);
        return hr;
    }
    *bitmap = dib;
    *alpha_type = WTSAT_ARGB;
    return S_OK;
}
