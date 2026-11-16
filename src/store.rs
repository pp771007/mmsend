//! Persistent state. Non-secret settings live in `%APPDATA%\mmsend\config.json`;
//! the password and session token live in Windows Credential Manager, which encrypts
//! them with DPAPI under the current Windows account.

use std::io::Write;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};

use crate::install::COMMAND_NAME;

const CONFIG_FILE: &str = "config.json";
const SEND_LOG_FILE: &str = "sends.jsonl";
/// Weeks of a few scheduled jobs. The file is cut back to this many lines once it
/// has grown to twice as many, so most runs only append.
const SEND_LOG_KEEP: usize = 300;
/// Debug builds only: one more profile (`mmsend-dev-<name>`) beside the debug
/// build's own, for a clean start such as never having logged in.
#[cfg(debug_assertions)]
const DEV_PROFILE_VARIABLE: &str = "MMSEND_DEV_PROFILE";

/// The settings folder and the entry under Control Panel > Credential Manager >
/// Windows Credentials: `mmsend`, or `mmsend-dev` for a debug build.
fn profile_name() -> String {
    #[cfg(debug_assertions)]
    if let Ok(profile) = std::env::var(DEV_PROFILE_VARIABLE) {
        return format!("{COMMAND_NAME}-{profile}");
    }
    COMMAND_NAME.to_owned()
}

/// `%APPDATA%\mmsend`: settings and anything else kept between runs.
pub fn app_dir() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA").ok_or("找不到 APPDATA 環境變數")?;
    Ok(PathBuf::from(appdata).join(profile_name()))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub server_url: String,
    /// What the user typed to log in (username or email); reused for silent re-login.
    pub login_id: String,
    pub username: String,
    pub user_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Secrets {
    /// Kept so an expired session token can be renewed without the user noticing.
    pub password: String,
    pub token: String,
}

/// One command-line run, so "why didn't last night's job send?" has an answer.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SendRecord {
    /// Local time, formatted when written: showing it needs no time-zone maths.
    pub time: String,
    /// Who it was for, as the command line printed it (empty if the arguments
    /// could not be parsed).
    pub target: String,
    pub ok: bool,
    /// What the command printed: "已傳送給 @bob", or the error.
    pub detail: String,
    /// The start of the message and the attachment names, to tell runs apart.
    pub summary: String,
    /// The sent post, for `mmsend delete --last`; absent in records older than that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_id: Option<String>,
}

impl SendRecord {
    pub fn starting_now() -> Self {
        // SAFETY: GetLocalTime only fills the struct it is given.
        let now = unsafe {
            let mut now = std::mem::zeroed();
            GetLocalTime(&mut now);
            now
        };
        SendRecord {
            time: format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
            ),
            target: String::new(),
            ok: false,
            detail: String::new(),
            summary: String::new(),
            post_id: None,
        }
    }
}

pub fn append_send_record(record: &SendRecord) -> Result<(), String> {
    let dir = app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("無法建立 {}：{e}", dir.display()))?;
    let path = dir.join(SEND_LOG_FILE);
    let mut line = serde_json::to_string(record).map_err(|e| e.to_string())?;
    line.push('\n');
    // One write per line: appends from jobs running at the same time do not interleave.
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .map_err(|e| format!("無法寫入 {}：{e}", path.display()))?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > SEND_LOG_KEEP * 2 {
        let kept = lines[lines.len() - SEND_LOG_KEEP..].join("\n") + "\n";
        std::fs::write(&path, kept).map_err(|e| format!("無法寫入 {}：{e}", path.display()))?;
    }
    Ok(())
}

/// Newest first.
pub fn load_send_records() -> Result<Vec<SendRecord>, String> {
    let path = app_dir()?.join(SEND_LOG_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("無法讀取 {}：{e}", path.display())),
    };
    // A line cut short by a crash should not hide every other record.
    Ok(text.lines().rev().filter_map(|line| serde_json::from_str(line).ok()).collect())
}

/// Settings, send log and everything else in `app_dir`, for uninstalling.
pub fn remove_app_dir() -> Result<(), String> {
    let dir = app_dir()?;
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("無法刪除 {}：{e}", dir.display())),
        _ => Ok(()),
    }
}

/// A JSON file in `app_dir`; `None` if it does not exist yet.
pub fn load_json<T: DeserializeOwned>(file: &str) -> Result<Option<T>, String> {
    let path = app_dir()?.join(file);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("設定檔 {} 損毀：{e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("無法讀取設定檔 {}：{e}", path.display())),
    }
}

pub fn save_json<T: Serialize>(file: &str, value: &T) -> Result<(), String> {
    let dir = app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("無法建立 {}：{e}", dir.display()))?;
    let path = dir.join(file);
    let json = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("無法寫入 {}：{e}", path.display()))
}

pub fn load_config() -> Result<Option<Config>, String> {
    load_json(CONFIG_FILE)
}

pub fn save_config(config: &Config) -> Result<(), String> {
    save_json(CONFIG_FILE, config)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn load_secrets() -> Result<Option<Secrets>, String> {
    let target = wide(&profile_name());
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is NUL-terminated and outlives the call; on success the
    // returned buffer is read once and released with CredFree.
    let bytes = unsafe {
        if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
            let error = GetLastError();
            return if error == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(format!("無法讀取 Windows 認證管理員（錯誤碼 {error}）"))
            };
        }
        let c = &*credential;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
        CredFree(credential as *const _);
        bytes
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("認證管理員中的資料已損毀：{e}"))
}

pub fn save_secrets(secrets: &Secrets) -> Result<(), String> {
    let mut blob = serde_json::to_vec(secrets).map_err(|e| e.to_string())?;
    let mut target = wide(&profile_name());
    let mut user_name = wide(COMMAND_NAME);
    // SAFETY: an all-zero CREDENTIALW is a valid "no optional fields" value; every
    // pointer we set points into buffers that live until CredWriteW returns.
    let ok = unsafe {
        let mut credential: CREDENTIALW = std::mem::zeroed();
        credential.Type = CRED_TYPE_GENERIC;
        credential.TargetName = target.as_mut_ptr();
        credential.UserName = user_name.as_mut_ptr();
        credential.CredentialBlobSize = blob.len() as u32;
        credential.CredentialBlob = blob.as_mut_ptr();
        credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
        CredWriteW(&credential, 0)
    };
    if ok == 0 {
        let error = unsafe { GetLastError() };
        return Err(format!("無法寫入 Windows 認證管理員（錯誤碼 {error}）"));
    }
    Ok(())
}

pub fn clear_secrets() -> Result<(), String> {
    let target = wide(&profile_name());
    // SAFETY: `target` is NUL-terminated and outlives the call.
    unsafe {
        if CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
            let error = GetLastError();
            if error != ERROR_NOT_FOUND {
                return Err(format!("無法清除 Windows 認證管理員（錯誤碼 {error}）"));
            }
        }
    }
    Ok(())
}
