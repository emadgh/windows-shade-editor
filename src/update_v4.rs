use std::ffi::c_void;
use std::fs;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use windows_sys::Win32::Networking::WinHttp::*;

const REPOSITORY: &str = "emadgh/windows-shade-editor";
const API_HOST: &str = "api.github.com";
const ASSET_NAME: &str = "ShadeEditor.exe";
const MAX_DOWNLOAD_SIZE: usize = 200 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub release_url: String,
    pub download_url: String,
}

#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading {
        info: UpdateInfo,
        downloaded: u64,
        total: Option<u64>,
    },
    Ready(UpdateInfo, PathBuf),
    Failed(String),
}

#[derive(Clone)]
pub struct UpdateManager {
    state: Arc<Mutex<UpdateStatus>>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self { state: Arc::new(Mutex::new(UpdateStatus::Idle)) }
    }
}

impl UpdateManager {
    pub fn status(&self) -> UpdateStatus {
        self.state.lock().unwrap().clone()
    }

    pub fn start_check(&self, auto_download: bool) -> bool {
        {
            let mut state = self.state.lock().unwrap();
            if matches!(*state, UpdateStatus::Checking | UpdateStatus::Downloading { .. }) {
                return false;
            }
            *state = UpdateStatus::Checking;
        }

        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            let next = match check_latest_release() {
                Ok(Some(info)) if auto_download => {
                    *state.lock().unwrap() = UpdateStatus::Downloading {
                        info: info.clone(),
                        downloaded: 0,
                        total: None,
                    };
                    let progress_state = Arc::clone(&state);
                    let progress_info = info.clone();
                    match download_update(&info, move |downloaded, total| {
                        *progress_state.lock().unwrap() = UpdateStatus::Downloading {
                            info: progress_info.clone(),
                            downloaded,
                            total,
                        };
                    }) {
                        Ok(path) => UpdateStatus::Ready(info, path),
                        Err(message) => UpdateStatus::Failed(message),
                    }
                }
                Ok(Some(info)) => UpdateStatus::Available(info),
                Ok(None) => UpdateStatus::UpToDate,
                Err(message) => UpdateStatus::Failed(message),
            };
            *state.lock().unwrap() = next;
        });
        true
    }

    pub fn start_download(&self) -> bool {
        let info = match self.status() {
            UpdateStatus::Available(info) => info,
            _ => return false,
        };
        *self.state.lock().unwrap() = UpdateStatus::Downloading {
            info: info.clone(),
            downloaded: 0,
            total: None,
        };
        let state = Arc::clone(&self.state);
        std::thread::spawn(move || {
            let progress_state = Arc::clone(&state);
            let progress_info = info.clone();
            let next = match download_update(&info, move |downloaded, total| {
                *progress_state.lock().unwrap() = UpdateStatus::Downloading {
                    info: progress_info.clone(),
                    downloaded,
                    total,
                };
            }) {
                Ok(path) => UpdateStatus::Ready(info, path),
                Err(message) => UpdateStatus::Failed(message),
            };
            *state.lock().unwrap() = next;
        });
        true
    }

    pub fn apply_ready(&self) -> Result<bool, String> {
        let (_info, source) = match self.status() {
            UpdateStatus::Ready(info, source) => (info, source),
            _ => return Ok(false),
        };
        let current_exe = std::env::current_exe().map_err(|err| format!("Cannot locate current executable: {err}"))?;
        let script = std::env::temp_dir().join(format!("ShadeEditor-updater-{}.ps1", std::process::id()));
        fs::write(&script, updater_script()).map_err(|err| format!("Cannot create updater script: {err}"))?;
        launch_updater(&script, &source, &current_exe)?;
        Ok(true)
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

struct InternetHandle(*mut c_void);

impl Drop for InternetHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0); }
        }
    }
}

enum HttpPayload {
    Data(Vec<u8>),
    NotFound,
}

fn check_latest_release() -> Result<Option<UpdateInfo>, String> {
    let path = format!("/repos/{REPOSITORY}/releases/latest");
    let body = match http_get(API_HOST, &path, None)? {
        HttpPayload::Data(body) => body,
        // GitHub returns 404 when a repository has no published Release yet.
        // That is not an application error; it simply means there is nothing to install.
        HttpPayload::NotFound => return Ok(None),
    };
    let release: GithubRelease = serde_json::from_slice(&body)
        .map_err(|err| format!("Invalid GitHub release response: {err}"))?;
    if !is_newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let asset = release.assets.into_iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(ASSET_NAME))
        .ok_or_else(|| format!("Release {} has no {ASSET_NAME} asset.", release.tag_name))?;
    Ok(Some(UpdateInfo {
        version: release.tag_name.trim_start_matches(['v', 'V']).to_owned(),
        release_url: release.html_url,
        download_url: asset.browser_download_url,
    }))
}

fn download_update<F>(info: &UpdateInfo, progress: F) -> Result<PathBuf, String>
where
    F: Fn(u64, Option<u64>),
{
    let (host, path) = split_https_url(&info.download_url)
        .ok_or_else(|| "Update URL is not a valid HTTPS URL.".to_owned())?;
    let payload = http_get(host, path, Some(&progress))?;
    let bytes = match payload {
        HttpPayload::Data(bytes) => bytes,
        HttpPayload::NotFound => return Err("Update asset returned HTTP 404.".to_owned()),
    };
    if bytes.len() < 100_000 || !bytes.starts_with(b"MZ") {
        return Err("Downloaded update is not a valid Windows executable.".to_owned());
    }
    let safe_version = info.version.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-')
        .collect::<String>();
    let path = std::env::temp_dir().join(format!(
        "ShadeEditor-update-{}-{safe_version}.exe",
        std::process::id()
    ));
    fs::write(&path, bytes).map_err(|err| format!("Cannot store downloaded update: {err}"))?;
    Ok(path)
}

fn launch_updater(script: &Path, source: &Path, destination: &Path) -> Result<(), String> {
    Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(script)
        .arg("-TargetPid").arg(std::process::id().to_string())
        .arg("-Source").arg(source)
        .arg("-Destination").arg(destination)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("Cannot launch updater: {err}"))
}

fn updater_script() -> &'static str {
    r#"param([int]$TargetPid, [string]$Source, [string]$Destination)
Wait-Process -Id $TargetPid -ErrorAction SilentlyContinue
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    try {
        Copy-Item -LiteralPath $Source -Destination $Destination -Force -ErrorAction Stop
        Start-Process -FilePath $Destination
        Remove-Item -LiteralPath $Source -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
        exit 0
    } catch {
        Start-Sleep -Milliseconds 500
    }
}
"#
}

fn split_https_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

fn version_tuple(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.trim_start_matches(['v', 'V']).split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch_text = parts.next().unwrap_or("0");
    let patch = patch_text.split(|ch: char| !ch.is_ascii_digit()).next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(remote: &str, current: &str) -> bool {
    matches!((version_tuple(remote), version_tuple(current)), (Some(remote), Some(current)) if remote > current)
}

fn http_get(
    host: &str,
    path: &str,
    progress: Option<&dyn Fn(u64, Option<u64>)>,
) -> Result<HttpPayload, String> {
    unsafe {
        let agent = wide("Shade Editor Update Checker");
        let session = InternetHandle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() { return Err("Cannot initialize WinHTTP.".to_owned()); }

        let host_wide = wide(host);
        let connection = InternetHandle(WinHttpConnect(
            session.0,
            host_wide.as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() { return Err("Cannot connect to update server.".to_owned()); }

        let verb = wide("GET");
        let path_wide = wide(path);
        let request = InternetHandle(WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path_wide.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE | WINHTTP_FLAG_REFRESH,
        ));
        if request.0.is_null() { return Err("Cannot create update request.".to_owned()); }
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err("Update request failed.".to_owned());
        }

        let mut status_code = 0u32;
        let mut status_size = size_of::<u32>() as u32;
        let mut index = 0u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status_code as *mut u32 as *mut c_void,
            &mut status_size,
            &mut index,
        ) == 0 {
            return Err("Cannot read update HTTP status.".to_owned());
        }
        if status_code == 404 {
            return Ok(HttpPayload::NotFound);
        }
        if !(200..300).contains(&status_code) {
            return Err(format!("Update server returned HTTP {status_code}."));
        }

        let mut content_length = 0u32;
        let mut content_length_size = size_of::<u32>() as u32;
        let mut content_index = 0u32;
        let total = if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut content_length as *mut u32 as *mut c_void,
            &mut content_length_size,
            &mut content_index,
        ) != 0 && content_length > 0 {
            Some(u64::from(content_length))
        } else {
            None
        };

        let mut body = Vec::new();
        if let Some(callback) = progress { callback(0, total); }
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err("Cannot read update response.".to_owned());
            }
            if available == 0 { break; }
            if body.len().saturating_add(available as usize) > MAX_DOWNLOAD_SIZE {
                return Err("Update download is unexpectedly large.".to_owned());
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr() as *mut c_void,
                available,
                &mut read,
            ) == 0 {
                return Err("Cannot read update data.".to_owned());
            }
            body.truncate(start + read as usize);
            if let Some(callback) = progress { callback(body.len() as u64, total); }
        }
        Ok(HttpPayload::Data(body))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_release_versions() {
        assert!(is_newer("v0.4.0", "0.3.1"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.3.1", "0.3.1"));
        assert!(!is_newer("v0.3.0", "0.3.1"));
    }
}
