#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shobjidl.h>
#include <thumbcache.h>
#include <wincodec.h>
#include <wincrypt.h>
#include <shellapi.h>

#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdint>
#include <new>
#include <string>
#include <vector>

#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "windowscodecs.lib")
#pragma comment(lib, "crypt32.lib")
#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "gdi32.lib")

namespace {

// {A8F6DCD8-DC68-4D1C-9D99-CA1449C22C78}
constexpr CLSID CLSID_ShadeThumbnailProvider = {
    0xa8f6dcd8,
    0xdc68,
    0x4d1c,
    {0x9d, 0x99, 0xca, 0x14, 0x49, 0xc2, 0x2c, 0x78},
};

constexpr wchar_t kProviderClsid[] = L"{A8F6DCD8-DC68-4D1C-9D99-CA1449C22C78}";
constexpr wchar_t kThumbnailHandlerCategory[] = L"{E357FCCD-A995-4576-B01F-234630154E96}";
constexpr size_t kMaxShadeBytes = 64u * 1024u * 1024u;
constexpr DWORD kMaxPngBytes = 16u * 1024u * 1024u;

HMODULE g_module = nullptr;
std::atomic<long> g_objects{0};
std::atomic<long> g_server_locks{0};

void safe_release(IUnknown* object) {
    if (object) object->Release();
}

HRESULT read_stream(IStream* source, std::string& output) {
    if (!source) return E_POINTER;
    IStream* stream = nullptr;
    HRESULT hr = source->Clone(&stream);
    if (FAILED(hr)) {
        stream = source;
        stream->AddRef();
    }
    LARGE_INTEGER zero{};
    hr = stream->Seek(zero, STREAM_SEEK_SET, nullptr);
    if (FAILED(hr)) {
        stream->Release();
        return hr;
    }
    output.clear();
    output.reserve(64 * 1024);
    char buffer[64 * 1024];
    while (true) {
        ULONG read = 0;
        hr = stream->Read(buffer, static_cast<ULONG>(sizeof(buffer)), &read);
        if (FAILED(hr)) {
            stream->Release();
            return hr;
        }
        if (read == 0) break;
        if (output.size() + static_cast<size_t>(read) > kMaxShadeBytes) {
            stream->Release();
            return HRESULT_FROM_WIN32(ERROR_FILE_TOO_LARGE);
        }
        output.append(buffer, buffer + read);
        if (hr == S_FALSE) break;
    }
    stream->Release();
    return S_OK;
}

bool extract_json_string(const std::string& json, const char* key, std::string& value) {
    std::string needle = "\"";
    needle += key;
    needle += "\"";
    size_t pos = json.find(needle);
    if (pos == std::string::npos) return false;
    pos = json.find(':', pos + needle.size());
    if (pos == std::string::npos) return false;
    ++pos;
    while (pos < json.size() && (json[pos] == ' ' || json[pos] == '\t' || json[pos] == '\r' || json[pos] == '\n')) ++pos;
    if (pos >= json.size() || json[pos] != '"') return false;
    ++pos;
    const size_t begin = pos;
    while (pos < json.size()) {
        if (json[pos] == '"') {
            value.assign(json.data() + begin, pos - begin);
            return true;
        }
        if (json[pos] == '\\') return false;
        ++pos;
    }
    return false;
}

HRESULT decode_base64(const std::string& text, std::vector<BYTE>& bytes) {
    DWORD size = 0;
    if (!CryptStringToBinaryA(text.c_str(), static_cast<DWORD>(text.size()), CRYPT_STRING_BASE64, nullptr, &size, nullptr, nullptr)) {
        return HRESULT_FROM_WIN32(GetLastError());
    }
    if (size == 0 || size > kMaxPngBytes) return HRESULT_FROM_WIN32(ERROR_FILE_TOO_LARGE);
    bytes.resize(size);
    if (!CryptStringToBinaryA(text.c_str(), static_cast<DWORD>(text.size()), CRYPT_STRING_BASE64, bytes.data(), &size, nullptr, nullptr)) {
        bytes.clear();
        return HRESULT_FROM_WIN32(GetLastError());
    }
    bytes.resize(size);
    return S_OK;
}

HRESULT png_to_hbitmap(std::vector<BYTE>& png, UINT requested, HBITMAP* bitmap) {
    if (!bitmap) return E_POINTER;
    *bitmap = nullptr;
    if (png.empty() || requested == 0) return E_INVALIDARG;

    IWICImagingFactory* factory = nullptr;
    IWICStream* stream = nullptr;
    IWICBitmapDecoder* decoder = nullptr;
    IWICBitmapFrameDecode* frame = nullptr;
    IWICBitmapScaler* scaler = nullptr;
    IWICFormatConverter* converter = nullptr;
    IWICBitmapSource* source = nullptr;
    HRESULT hr = CoCreateInstance(CLSID_WICImagingFactory, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&factory));
    if (FAILED(hr)) goto cleanup;
    hr = factory->CreateStream(&stream);
    if (FAILED(hr)) goto cleanup;
    hr = stream->InitializeFromMemory(png.data(), static_cast<DWORD>(png.size()));
    if (FAILED(hr)) goto cleanup;
    hr = factory->CreateDecoderFromStream(stream, nullptr, WICDecodeMetadataCacheOnLoad, &decoder);
    if (FAILED(hr)) goto cleanup;
    hr = decoder->GetFrame(0, &frame);
    if (FAILED(hr)) goto cleanup;

    UINT width = 0;
    UINT height = 0;
    hr = frame->GetSize(&width, &height);
    if (FAILED(hr) || width == 0 || height == 0) {
        if (SUCCEEDED(hr)) hr = E_FAIL;
        goto cleanup;
    }
    const double scale = std::min(1.0, std::min(static_cast<double>(requested) / width, static_cast<double>(requested) / height));
    const UINT target_width = std::max<UINT>(1, static_cast<UINT>(std::lround(width * scale)));
    const UINT target_height = std::max<UINT>(1, static_cast<UINT>(std::lround(height * scale)));

    if (target_width != width || target_height != height) {
        hr = factory->CreateBitmapScaler(&scaler);
        if (FAILED(hr)) goto cleanup;
        hr = scaler->Initialize(frame, target_width, target_height, WICBitmapInterpolationModeFant);
        if (FAILED(hr)) goto cleanup;
        source = scaler;
        source->AddRef();
    } else {
        source = frame;
        source->AddRef();
    }

    hr = factory->CreateFormatConverter(&converter);
    if (FAILED(hr)) goto cleanup;
    hr = converter->Initialize(source, GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, nullptr, 0.0, WICBitmapPaletteTypeCustom);
    if (FAILED(hr)) goto cleanup;

    if (target_width > UINT_MAX / 4 || target_height > UINT_MAX / (target_width * 4)) {
        hr = HRESULT_FROM_WIN32(ERROR_ARITHMETIC_OVERFLOW);
        goto cleanup;
    }
    BITMAPINFO info{};
    info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    info.bmiHeader.biWidth = static_cast<LONG>(target_width);
    info.bmiHeader.biHeight = -static_cast<LONG>(target_height);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    void* pixels = nullptr;
    HDC dc = GetDC(nullptr);
    HBITMAP dib = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &pixels, nullptr, 0);
    ReleaseDC(nullptr, dc);
    if (!dib || !pixels) {
        hr = HRESULT_FROM_WIN32(GetLastError());
        goto cleanup;
    }
    {
        const UINT stride = target_width * 4;
        const UINT buffer_size = stride * target_height;
        hr = converter->CopyPixels(nullptr, stride, buffer_size, static_cast<BYTE*>(pixels));
    }
    if (FAILED(hr)) {
        DeleteObject(dib);
        goto cleanup;
    }
    *bitmap = dib;

cleanup:
    safe_release(source);
    safe_release(converter);
    safe_release(scaler);
    safe_release(frame);
    safe_release(decoder);
    safe_release(stream);
    safe_release(factory);
    return hr;
}

HRESULT set_string_value(HKEY root, const std::wstring& subkey, const wchar_t* name, const std::wstring& value) {
    HKEY key = nullptr;
    LONG status = RegCreateKeyExW(root, subkey.c_str(), 0, nullptr, REG_OPTION_NON_VOLATILE, KEY_WRITE, nullptr, &key, nullptr);
    if (status != ERROR_SUCCESS) return HRESULT_FROM_WIN32(status);
    const BYTE* data = reinterpret_cast<const BYTE*>(value.c_str());
    const DWORD bytes = static_cast<DWORD>((value.size() + 1) * sizeof(wchar_t));
    status = RegSetValueExW(key, name, 0, REG_SZ, data, bytes);
    RegCloseKey(key);
    return HRESULT_FROM_WIN32(status);
}

std::wstring module_path() {
    std::vector<wchar_t> buffer(1024);
    while (buffer.size() < 32768) {
        DWORD length = GetModuleFileNameW(g_module, buffer.data(), static_cast<DWORD>(buffer.size()));
        if (length == 0) return {};
        if (length < buffer.size() - 1) return std::wstring(buffer.data(), length);
        buffer.resize(buffer.size() * 2);
    }
    return {};
}

class ShadeThumbnailProvider final : public IThumbnailProvider, public IInitializeWithStream {
public:
    ShadeThumbnailProvider() { ++g_objects; }
    ~ShadeThumbnailProvider() override { safe_release(stream_); --g_objects; }

    IFACEMETHODIMP QueryInterface(REFIID riid, void** object) override {
        if (!object) return E_POINTER;
        *object = nullptr;
        if (riid == IID_IUnknown || riid == __uuidof(IThumbnailProvider)) {
            *object = static_cast<IThumbnailProvider*>(this);
        } else if (riid == __uuidof(IInitializeWithStream)) {
            *object = static_cast<IInitializeWithStream*>(this);
        } else {
            return E_NOINTERFACE;
        }
        AddRef();
        return S_OK;
    }
    IFACEMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&ref_count_)); }
    IFACEMETHODIMP_(ULONG) Release() override {
        const ULONG count = static_cast<ULONG>(InterlockedDecrement(&ref_count_));
        if (count == 0) delete this;
        return count;
    }
    IFACEMETHODIMP Initialize(IStream* stream, DWORD mode) override {
        if (!stream) return E_POINTER;
        if ((mode & STGM_READWRITE) == STGM_READWRITE || (mode & STGM_WRITE) == STGM_WRITE) return STG_E_ACCESSDENIED;
        if (stream_) return HRESULT_FROM_WIN32(ERROR_ALREADY_INITIALIZED);
        stream_ = stream;
        stream_->AddRef();
        return S_OK;
    }
    IFACEMETHODIMP GetThumbnail(UINT cx, HBITMAP* bitmap, WTS_ALPHATYPE* alpha_type) override {
        if (!bitmap || !alpha_type) return E_POINTER;
        *bitmap = nullptr;
        *alpha_type = WTSAT_UNKNOWN;
        if (!stream_) return E_UNEXPECTED;
        std::string json;
        HRESULT hr = read_stream(stream_, json);
        if (FAILED(hr)) return hr;
        std::string mime;
        if (extract_json_string(json, "mime_type", mime) && mime != "image/png") return HRESULT_FROM_WIN32(ERROR_UNSUPPORTED_TYPE);
        std::string base64;
        if (!extract_json_string(json, "data_base64", base64) || base64.empty()) return HRESULT_FROM_WIN32(ERROR_NOT_FOUND);
        std::vector<BYTE> png;
        hr = decode_base64(base64, png);
        if (FAILED(hr)) return hr;
        hr = png_to_hbitmap(png, cx, bitmap);
        if (SUCCEEDED(hr)) *alpha_type = WTSAT_ARGB;
        return hr;
    }
private:
    LONG ref_count_ = 1;
    IStream* stream_ = nullptr;
};

class ShadeClassFactory final : public IClassFactory {
public:
    ShadeClassFactory() { ++g_objects; }
    ~ShadeClassFactory() override { --g_objects; }
    IFACEMETHODIMP QueryInterface(REFIID riid, void** object) override {
        if (!object) return E_POINTER;
        *object = nullptr;
        if (riid == IID_IUnknown || riid == IID_IClassFactory) {
            *object = static_cast<IClassFactory*>(this);
            AddRef();
            return S_OK;
        }
        return E_NOINTERFACE;
    }
    IFACEMETHODIMP_(ULONG) AddRef() override { return static_cast<ULONG>(InterlockedIncrement(&ref_count_)); }
    IFACEMETHODIMP_(ULONG) Release() override {
        const ULONG count = static_cast<ULONG>(InterlockedDecrement(&ref_count_));
        if (count == 0) delete this;
        return count;
    }
    IFACEMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** object) override {
        if (outer) return CLASS_E_NOAGGREGATION;
        if (!object) return E_POINTER;
        *object = nullptr;
        auto* provider = new (std::nothrow) ShadeThumbnailProvider();
        if (!provider) return E_OUTOFMEMORY;
        const HRESULT hr = provider->QueryInterface(riid, object);
        provider->Release();
        return hr;
    }
    IFACEMETHODIMP LockServer(BOOL lock) override {
        if (lock) ++g_server_locks;
        else --g_server_locks;
        return S_OK;
    }
private:
    LONG ref_count_ = 1;
};

} // namespace

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = instance;
        DisableThreadLibraryCalls(instance);
    }
    return TRUE;
}

extern "C" __declspec(dllexport) HRESULT __stdcall DllGetClassObject(REFCLSID clsid, REFIID riid, void** object) {
    if (clsid != CLSID_ShadeThumbnailProvider) return CLASS_E_CLASSNOTAVAILABLE;
    auto* factory = new (std::nothrow) ShadeClassFactory();
    if (!factory) return E_OUTOFMEMORY;
    const HRESULT hr = factory->QueryInterface(riid, object);
    factory->Release();
    return hr;
}

extern "C" __declspec(dllexport) HRESULT __stdcall DllCanUnloadNow() {
    return (g_objects.load() == 0 && g_server_locks.load() == 0) ? S_OK : S_FALSE;
}

extern "C" __declspec(dllexport) HRESULT __stdcall DllRegisterServer() {
    const std::wstring dll = module_path();
    if (dll.empty()) return HRESULT_FROM_WIN32(GetLastError());
    const std::wstring classes = L"Software\\Classes\\";
    const std::wstring clsid_key = classes + L"CLSID\\" + kProviderClsid + L"\\InprocServer32";
    HRESULT hr = set_string_value(HKEY_CURRENT_USER, clsid_key, nullptr, dll);
    if (FAILED(hr)) return hr;
    hr = set_string_value(HKEY_CURRENT_USER, clsid_key, L"ThreadingModel", L"Apartment");
    if (FAILED(hr)) return hr;
    const std::wstring thumbnail_key = classes + L".shade\\ShellEx\\" + kThumbnailHandlerCategory;
    hr = set_string_value(HKEY_CURRENT_USER, thumbnail_key, nullptr, kProviderClsid);
    if (FAILED(hr)) return hr;
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, nullptr, nullptr);
    return S_OK;
}

extern "C" __declspec(dllexport) HRESULT __stdcall DllUnregisterServer() {
    const std::wstring classes = L"Software\\Classes\\";
    const std::wstring clsid_key = classes + L"CLSID\\" + kProviderClsid;
    const std::wstring thumbnail_key = classes + L".shade\\ShellEx\\" + kThumbnailHandlerCategory;
    LONG first_error = ERROR_SUCCESS;
    LONG status = RegDeleteTreeW(HKEY_CURRENT_USER, thumbnail_key.c_str());
    if (status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND && first_error == ERROR_SUCCESS) first_error = status;
    status = RegDeleteTreeW(HKEY_CURRENT_USER, clsid_key.c_str());
    if (status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND && first_error == ERROR_SUCCESS) first_error = status;
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, nullptr, nullptr);
    return first_error == ERROR_SUCCESS ? S_OK : HRESULT_FROM_WIN32(first_error);
}
