use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct AppLog {
    path: PathBuf,
}

impl Default for AppLog {
    fn default() -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self { path: base.join("ShadeEditor").join("shade-editor.log") }
    }
}

impl AppLog {
    pub fn path(&self) -> &PathBuf { &self.path }

    pub fn write(&self, level: &str, message: &str) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let sanitized = message.replace(['\r', '\n'], " ");
            let _ = writeln!(file, "[{timestamp}] {level}: {sanitized}");
        }
    }

    pub fn info(&self, message: &str) { self.write("INFO", message); }
    pub fn error(&self, message: &str) { self.write("ERROR", message); }

    pub fn read(&self) -> String {
        fs::read_to_string(&self.path).unwrap_or_else(|_| "No log entries yet.".to_owned())
    }

    pub fn clear(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("Cannot create log directory: {err}"))?;
        }
        fs::write(&self.path, "").map_err(|err| format!("Cannot clear log: {err}"))
    }
}
