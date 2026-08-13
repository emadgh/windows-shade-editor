from pathlib import Path


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8").replace("\r\n", "\n")


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8", newline="\n")


cargo = read("Cargo.toml").replace('version = "0.11.0"', 'version = "0.12.0"', 1)
write("Cargo.toml", cargo)
lock = read("Cargo.lock").replace(
    'name = "windows-shade-editor"\nversion = "0.11.0"',
    'name = "windows-shade-editor"\nversion = "0.12.0"',
    1,
)
write("Cargo.lock", lock)

app = read("src/app_main.rs")
main_start = app.index("fn main() -> eframe::Result {")
main_end = app.index("\n}\n\n#[derive", main_start) + 2
new_main = '''fn main() -> eframe::Result {
    let startup_project = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("shade"))
        });
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("Shade Editor")
            .with_inner_size([1550.0, 920.0])
            .with_min_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Shade Editor",
        native_options,
        Box::new(move |cc| {
            let mut app = ShadeApp::new(cc);
            if let Some(path) = startup_project.clone() {
                app.open_project_path(path);
            }
            Ok(Box::new(app))
        }),
    )
}'''
app = app[:main_start] + new_main + app[main_end:]

open_start = app.index("    fn open_project_dialog(&mut self) {")
open_end = app.index("\n    fn save_project(&mut self, save_as: bool)", open_start)
old_open = app[open_start:open_end]
body_start = old_open.index("        let max_dimension = self.settings.max_preview_dimension;")
load_body = old_open[body_start:]
new_open = '''    fn open_project_dialog(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Shade project", &["shade"])
            .pick_file()
        else {
            return;
        };
        self.open_project_path(path);
    }

    fn open_project_path(&mut self, path: PathBuf) {
        if self.job.is_some() {
            return;
        }
        self.recovery_candidate = None;
'''
app = app[:open_start] + new_open + load_body + app[open_end:]
write("src/app_main.rs", app)

notes = read("RELEASE_NOTES.md")
if not notes.startswith("# Shade Editor v0.12.0"):
    notes = '''# Shade Editor v0.12.0

- Adds a native x64 Windows Shell extension for `.shade` files without loading the editor UI runtime inside Explorer.
- Explorer thumbnails use the PNG already embedded in schema-v9 `.shade` projects through `IThumbnailProvider` and WIC.
- A read-only `IPropertyStore` exposes Face count, active Face, physical/pixel dimensions, DPI, bit depth, color model, channel counts, source TIFF name, total source bytes, and save time.
- Ships a custom Windows Property System `.propdesc` schema for Explorer Details/columns/search metadata.
- Adds an elevated installer for COM/property-handler registration and per-user `.shade` file association.
- Shade Editor accepts a `.shade` path as its first command-line argument so Explorer double-click can open the selected project.
- Native parser and COM/WIC regression tests validate schema-v9 metadata and the embedded PNG thumbnail.
- `.shade` schema remains v9.

''' + notes
write("RELEASE_NOTES.md", notes)

road = read("docs/ROADMAP.md")
old = '''## Native Windows integration

- Windows Explorer `.shade` thumbnail provider using the embedded project PNG.
- Windows Property Handler exposing physical/pixel dimensions, DPI, bit depth, channel/Face counts, and save metadata.
'''
new = '''## Native Windows integration

- Implemented in v0.12: native `.shade` thumbnail provider and read-only Windows Property Handler using the embedded PNG and cached project metadata.
- Remaining validation: clean-workstation install, Explorer thumbnail cache, Details columns/search indexing, file association, upgrade, and removal while the Shell DLL may be loaded.
'''
if old in road:
    road = road.replace(old, new, 1)
write("docs/ROADMAP.md", road)
