#include "ShadeProjectData.h"

#include <cassert>
#include <cmath>
#include <iostream>
#include <string>

int main() {
    const std::string json = R"json({
      "schema_version": 9,
      "name": "Moonstone test",
      "faces": [
        {"path":"moonstone-face1.tif","label":"Face 1"},
        {"path":"moonstone-face2.tif","label":"Face 2"}
      ],
      "adjustments": {},
      "thumbnail": {
        "mime_type": "image/png",
        "width": 2,
        "height": 1,
        "data_base64": "iVBORw0KGgo="
      },
      "file_metadata": {
        "saved_at_unix_ms": 1770000000000,
        "face_count": 2,
        "active_face_index": 1,
        "total_source_bytes": 123456789,
        "faces": [
          {
            "label":"Face 1","source_file_name":"moonstone-face1.tif",
            "width":720,"height":1280,"bit_depth":8,"color_model":"CMYK",
            "channel_count":6,"base_channel_count":4,
            "channel_names":["Cyan","Magenta","Yellow","Black","purpol","bgreen"],
            "dpi_x":220.0,"dpi_y":220.0,"dpi_from_source":true,
            "resolution_unit":2,"file_size_bytes":100,"modified_at_unix_ms":null
          },
          {
            "label":"Face 2","source_file_name":"moonstone-face2.tif",
            "width":5197,"height":10394,"bit_depth":16,"color_model":"CMYK",
            "channel_count":6,"base_channel_count":4,
            "channel_names":["Cyan","Magenta","Yellow","Black","purpol","bgreen"],
            "dpi_x":220.0,"dpi_y":220.0,"dpi_from_source":false,
            "resolution_unit":2,"file_size_bytes":200,"modified_at_unix_ms":null
          }
        ]
      }
    })json";

    shade_shell::ProjectData data;
    std::string error;
    assert(shade_shell::ParseShadeProject(json, data, &error));
    assert(error.empty());
    assert(data.schema_version == 9);
    assert(data.name == "Moonstone test");
    assert(data.face_count == 2);
    assert(data.active_face_index == 1);
    assert(data.total_source_bytes == 123456789);
    assert(data.saved_at_unix_ms == 1770000000000LL);
    assert(data.has_active_face);
    assert(data.active_face.source_file_name == "moonstone-face2.tif");
    assert(data.active_face.width == 5197);
    assert(data.active_face.height == 10394);
    assert(data.active_face.bit_depth == 16);
    assert(data.active_face.color_model == "CMYK");
    assert(data.active_face.channel_count == 6);
    assert(data.active_face.base_channel_count == 4);
    assert(std::abs(data.active_face.dpi_x - 220.0) < 0.001);
    assert(data.thumbnail_width == 2);
    assert(data.thumbnail_height == 1);
    assert(data.thumbnail_png.size() == 8);
    assert(data.thumbnail_png[0] == 0x89 && data.thumbnail_png[1] == 0x50);

    shade_shell::ProjectData invalid;
    assert(!shade_shell::ParseShadeProject("{\"schema_version\":8}", invalid, &error));
    assert(!error.empty());

    std::cout << "ShadeProjectData tests passed\n";
    return 0;
}
