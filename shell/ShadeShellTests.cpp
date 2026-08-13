#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <thumbcache.h>
#include <propsys.h>
#include <shlwapi.h>

#include <cassert>
#include <iostream>
#include <string>

namespace {
const CLSID CLSID_ShadeEditorShell = {
    0x6f49f9d5, 0x0f3a, 0x4bf0, {0x8c, 0x74, 0x8a, 0x59, 0x95, 0x1a, 0x75, 0xd2}};
const GUID FMTID_ShadeEditor = {
    0xe1486a27, 0x9a7b, 0x4e56, {0xbb, 0xd1, 0x50, 0xd7, 0xf0, 0x1c, 0x17, 0x78}};
const PROPERTYKEY PKEY_Shade_FaceCount = {FMTID_ShadeEditor, 2};
const PROPERTYKEY PKEY_Shade_ChannelCount = {FMTID_ShadeEditor, 18};
const PROPERTYKEY PKEY_Shade_PhysicalDimensions = {FMTID_ShadeEditor, 21};

using DllGetClassObjectFn = HRESULT(__stdcall*)(REFCLSID, REFIID, void**);
using DllCanUnloadNowFn = HRESULT(__stdcall*)();
}

int wmain(int argc, wchar_t** argv) {
    assert(argc == 2);
    HRESULT hr = CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);
    assert(SUCCEEDED(hr));

    HMODULE module = LoadLibraryW(argv[1]);
    assert(module != nullptr);
    auto get_class = reinterpret_cast<DllGetClassObjectFn>(GetProcAddress(module, "DllGetClassObject"));
    auto can_unload = reinterpret_cast<DllCanUnloadNowFn>(GetProcAddress(module, "DllCanUnloadNow"));
    assert(get_class && can_unload);

    IClassFactory* factory = nullptr;
    hr = get_class(CLSID_ShadeEditorShell, IID_PPV_ARGS(&factory));
    assert(SUCCEEDED(hr) && factory);

    IInitializeWithStream* initializer = nullptr;
    hr = factory->CreateInstance(nullptr, IID_PPV_ARGS(&initializer));
    factory->Release();
    assert(SUCCEEDED(hr) && initializer);

    const std::string json = R"json({
      "schema_version":9,
      "name":"Shell test",
      "faces":[{"path":"face.tif","label":"Face 1"}],
      "adjustments":{},
      "thumbnail":{"mime_type":"image/png","width":1,"height":1,
        "data_base64":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9ZQmcAAAAASUVORK5CYII="},
      "file_metadata":{"saved_at_unix_ms":1770000000000,"face_count":1,
        "active_face_index":0,"total_source_bytes":1234,"faces":[{
          "label":"Face 1","source_file_name":"face.tif","width":5197,"height":10394,
          "bit_depth":8,"color_model":"CMYK","channel_count":6,"base_channel_count":4,
          "channel_names":["Cyan","Magenta","Yellow","Black","purpol","bgreen"],
          "dpi_x":220.0,"dpi_y":220.0,"dpi_from_source":true,"resolution_unit":2,
          "file_size_bytes":1234,"modified_at_unix_ms":null}]}
    })json";

    IStream* stream = SHCreateMemStream(reinterpret_cast<const BYTE*>(json.data()),
                                        static_cast<UINT>(json.size()));
    assert(stream);
    hr = initializer->Initialize(stream, STGM_READ);
    stream->Release();
    assert(SUCCEEDED(hr));

    IPropertyStore* properties = nullptr;
    hr = initializer->QueryInterface(IID_PPV_ARGS(&properties));
    assert(SUCCEEDED(hr) && properties);

    PROPVARIANT value;
    PropVariantInit(&value);
    hr = properties->GetValue(PKEY_Shade_FaceCount, &value);
    assert(SUCCEEDED(hr) && value.vt == VT_UI4 && value.ulVal == 1);
    PropVariantClear(&value);

    hr = properties->GetValue(PKEY_Shade_ChannelCount, &value);
    assert(SUCCEEDED(hr) && value.vt == VT_UI4 && value.ulVal == 6);
    PropVariantClear(&value);

    hr = properties->GetValue(PKEY_Shade_PhysicalDimensions, &value);
    assert(SUCCEEDED(hr) && value.vt == VT_LPWSTR && value.pwszVal != nullptr);
    std::wstring physical(value.pwszVal);
    assert(physical.find(L"cm") != std::wstring::npos);
    PropVariantClear(&value);
    properties->Release();

    IThumbnailProvider* thumbnail = nullptr;
    hr = initializer->QueryInterface(IID_PPV_ARGS(&thumbnail));
    assert(SUCCEEDED(hr) && thumbnail);
    HBITMAP bitmap = nullptr;
    WTS_ALPHATYPE alpha = WTSAT_UNKNOWN;
    hr = thumbnail->GetThumbnail(128, &bitmap, &alpha);
    assert(SUCCEEDED(hr) && bitmap != nullptr && alpha == WTSAT_ARGB);
    BITMAP info{};
    assert(GetObjectW(bitmap, sizeof(info), &info) == sizeof(info));
    assert(info.bmWidth == 1 && info.bmHeight == 1);
    DeleteObject(bitmap);
    thumbnail->Release();
    initializer->Release();

    assert(can_unload() == S_OK);
    FreeLibrary(module);
    CoUninitialize();
    std::wcout << L"ShadeEditorShell COM tests passed\n";
    return 0;
}
