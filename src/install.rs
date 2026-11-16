//! "Install" copies the exe into a fixed folder on the user's PATH so copied
//! commands can say just `mmsend`: short, unaffected by where the download sits,
//! and valid in cmd, PowerShell, .bat files and Task Scheduler alike. (PowerShell
//! treats a quoted path as a string; it would need `& "C:\...\mmsend.exe"`.)

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};

pub const COMMAND_NAME: &str = "mmsend";
const EXE_NAME: &str = "mmsend.exe";
const ENVIRONMENT_KEY: &str = "Environment";
const PATH_VALUE: &str = "Path";
const BROADCAST_TIMEOUT_MS: u32 = 5000;

/// major.minor.patch, as Cargo.toml has it; winresource writes the same numbers
/// into the exe's version resource, which is how an installed copy is read back.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Version([u16; 3]);

impl Version {
    pub fn current() -> Self {
        let part = |text: &str| text.parse().expect("Cargo version parts are numeric");
        Self([
            part(env!("CARGO_PKG_VERSION_MAJOR")),
            part(env!("CARGO_PKG_VERSION_MINOR")),
            part(env!("CARGO_PKG_VERSION_PATCH")),
        ])
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.0;
        write!(f, "{major}.{minor}.{patch}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallState {
    NotInstalled,
    Installed,
    /// The installed copy is an older version, or another build of this version.
    Outdated,
    /// The installed copy is newer than this exe (an old download was opened):
    /// commands already work, and replacing it would be a downgrade.
    Newer(Version),
}

impl InstallState {
    pub fn commands_work(self) -> bool {
        matches!(self, Self::Installed | Self::Newer(_))
    }
}

fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("Programs").join(COMMAND_NAME))
}

/// Where `install` puts the exe: the full path works before a new shell picks up PATH.
pub fn installed_exe() -> Option<PathBuf> {
    install_dir().map(|dir| dir.join(EXE_NAME))
}

pub fn state() -> InstallState {
    let Some(dir) = install_dir() else {
        return InstallState::NotInstalled;
    };
    let installed = dir.join(EXE_NAME);
    let on_path = read_user_path().is_ok_and(|(path, _)| path_contains(&path, &dir));
    if !installed.exists() || !on_path {
        return InstallState::NotInstalled;
    }
    match std::env::current_exe() {
        Ok(current) if same_file_contents(&current, &installed) => InstallState::Installed,
        Ok(_) => match file_version(&installed) {
            Some(version) if version > Version::current() => InstallState::Newer(version),
            _ => InstallState::Outdated,
        },
        Err(_) => InstallState::Installed,
    }
}

fn file_version(path: &Path) -> Option<Version> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };

    let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let root = wide("\\");
    // SAFETY: `name` and `root` are NUL-terminated; `data` is sized by the size query
    // and outlives the pointer VerQueryValueW returns into it, whose length is checked
    // before it is read as a VS_FIXEDFILEINFO.
    unsafe {
        let size = GetFileVersionInfoSizeW(name.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut fixed: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut fixed, &mut len) == 0
            || (len as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let fixed = &*fixed.cast::<VS_FIXEDFILEINFO>();
        let high = |word: u32| (word >> 16) as u16;
        let low = |word: u32| (word & 0xffff) as u16;
        Some(Version([high(fixed.dwFileVersionMS), low(fixed.dwFileVersionMS), high(fixed.dwFileVersionLS)]))
    }
}

fn same_file_contents(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

pub fn install() -> Result<(), String> {
    let dir = install_dir().ok_or("找不到 LOCALAPPDATA 環境變數")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("無法建立 {}：{e}", dir.display()))?;
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = installed_exe().ok_or("找不到 LOCALAPPDATA 環境變數")?;
    if current != target {
        std::fs::copy(&current, &target).map_err(|e| {
            format!("無法複製到 {}（請先關閉其他 mmsend 視窗）：{e}", target.display())
        })?;
    }
    let (path, kind) = read_user_path()?;
    if !path_contains(&path, &dir) {
        write_user_path(&path_appending(&path, &dir.to_string_lossy()), kind)?;
        announce_environment_change();
    }
    // The download may be deleted after installing; this is how the window is reopened.
    create_shortcut(&target, &shortcut_path()?)
}

fn shortcut_path() -> Result<PathBuf, String> {
    let appdata = std::env::var_os("APPDATA").ok_or("找不到 APPDATA 環境變數")?;
    Ok(PathBuf::from(appdata)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(format!("{}.lnk", crate::APP_NAME)))
}

/// Undoes `install`: the PATH entry, the Start menu shortcut and the program folder.
/// Login data and settings are the caller's (`store`) to remove.
pub fn uninstall() -> Result<(), String> {
    let dir = install_dir().ok_or("找不到 LOCALAPPDATA 環境變數")?;
    let (path, kind) = read_user_path()?;
    if path_contains(&path, &dir) {
        write_user_path(&path_without(&path, &dir), kind)?;
        announce_environment_change();
    }
    let shortcut = shortcut_path()?;
    match std::fs::remove_file(&shortcut) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("無法刪除 {}：{e}", shortcut.display()));
        }
        _ => {}
    }
    if dir.exists() {
        move_running_copy_out(&dir)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("無法刪除 {}：{e}", dir.display()))?;
    }
    Ok(())
}

/// Opened from the Start menu, the window *is* the installed copy, and Windows
/// will not delete a running exe. It can still be renamed, though (within one
/// drive), so it moves to the temp folder and the program folder can go.
fn move_running_copy_out(dir: &Path) -> Result<(), String> {
    let Ok(current) = std::env::current_exe() else {
        return Ok(());
    };
    if !current.starts_with(dir) {
        return Ok(());
    }
    let parked = std::env::temp_dir().join(format!("{COMMAND_NAME}-uninstalled-{}.exe", std::process::id()));
    std::fs::rename(&current, &parked)
        .map_err(|e| format!("無法移除執行中的程式，請關閉視窗後手動刪除 {}：{e}", dir.display()))
}

pub(crate) fn create_shortcut(target: &Path, shortcut: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    let failed = |e: windows::core::Error| format!("無法建立開始功能表捷徑：{e}");
    // SAFETY: COM is initialised on this (worker) thread before use; every call
    // gets owned HSTRINGs that outlive it.
    unsafe {
        // S_FALSE (already initialised) is fine; a real failure surfaces below.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(failed)?;
        link.SetPath(&HSTRING::from(target.as_os_str())).map_err(failed)?;
        link.SetDescription(&HSTRING::from(crate::APP_NAME)).map_err(failed)?;
        let file: IPersistFile = link.cast().map_err(failed)?;
        file.Save(&HSTRING::from(shortcut.as_os_str()), true).map_err(failed)?;
    }
    Ok(())
}

fn normalize(entry: &str) -> String {
    entry.trim().trim_end_matches('\\').to_lowercase()
}

/// Whether one `;`-separated PATH entry names `dir` (any case, trailing `\`, or
/// written with %LOCALAPPDATA%).
fn names_dir(entry: &str, dir: &Path) -> bool {
    let expanded_local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    normalize(&entry.replace("%LOCALAPPDATA%", &expanded_local)) == normalize(&dir.to_string_lossy())
}

fn path_contains(path: &str, dir: &Path) -> bool {
    path.split(';').any(|entry| names_dir(entry, dir))
}

fn path_without(path: &str, dir: &Path) -> String {
    path.split(';')
        .filter(|entry| !names_dir(entry, dir))
        .collect::<Vec<_>>()
        .join(";")
}

fn path_appending(path: &str, dir: &str) -> String {
    let trimmed = path.trim_end_matches(';');
    if trimmed.is_empty() {
        dir.to_owned()
    } else {
        format!("{trimmed};{dir}")
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful RegOpenKeyExW.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn open_environment(access: u32) -> Result<RegKey, String> {
    let name = wide(ENVIRONMENT_KEY);
    let mut handle: HKEY = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, access, &mut handle) };
    if status != ERROR_SUCCESS {
        return Err(format!("無法開啟使用者環境變數（錯誤碼 {status}）"));
    }
    Ok(RegKey(handle))
}

/// The user's own PATH (not the system part) and its registry type, which must be
/// preserved: REG_EXPAND_SZ entries such as `%USERPROFILE%\bin` would stop expanding.
fn read_user_path() -> Result<(String, u32), String> {
    let key = open_environment(KEY_READ)?;
    let name = wide(PATH_VALUE);
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: size query with a null buffer, then a read into a buffer of that size.
    unsafe {
        let status = RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok((String::new(), REG_EXPAND_SZ));
        }
        if status != ERROR_SUCCESS {
            return Err(format!("無法讀取使用者 PATH（錯誤碼 {status}）"));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let status = RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr().cast(),
            &mut size,
        );
        if status != ERROR_SUCCESS {
            return Err(format!("無法讀取使用者 PATH（錯誤碼 {status}）"));
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Ok((String::from_utf16_lossy(&buffer[..len]), kind))
    }
}

fn write_user_path(value: &str, kind: u32) -> Result<(), String> {
    let key = open_environment(KEY_WRITE)?;
    let name = wide(PATH_VALUE);
    let data = wide(value);
    // SAFETY: `data` is NUL-terminated UTF-16 and its byte length is passed.
    let status = unsafe {
        RegSetValueExW(key.0, name.as_ptr(), 0, kind, data.as_ptr().cast(), (data.len() * 2) as u32)
    };
    if status != ERROR_SUCCESS {
        return Err(format!("無法更新使用者 PATH（錯誤碼 {status}）"));
    }
    Ok(())
}

/// Tells Explorer to reload the environment, so cmd windows opened from now on see
/// the new PATH. Windows that are already open keep the old one.
fn announce_environment_change() {
    let area = wide(ENVIRONMENT_KEY);
    let mut result = 0usize;
    // SAFETY: `area` outlives the call; the timeout bounds a hung top-level window.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            BROADCAST_TIMEOUT_MS,
            &mut result,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_appends_once_and_keeps_existing_entries() {
        let dir = Path::new(r"C:\Users\me\AppData\Local\Programs\mmsend");
        let before = r"%USERPROFILE%\bin;C:\tools\";
        let after = path_appending(before, &dir.to_string_lossy());
        assert_eq!(after, r"%USERPROFILE%\bin;C:\tools\;C:\Users\me\AppData\Local\Programs\mmsend");
        assert!(path_contains(&after, dir));
        assert!(path_contains(r"c:\users\me\appdata\local\programs\MMSEND\", dir));
        assert!(!path_contains(before, dir));
    }

    #[test]
    fn uninstall_removes_only_the_install_entry() {
        let dir = Path::new(r"C:\Users\me\AppData\Local\Programs\mmsend");
        let path = r"%USERPROFILE%\bin;c:\users\me\appdata\local\programs\MMSEND\;C:\tools";
        assert_eq!(path_without(path, dir), r"%USERPROFILE%\bin;C:\tools");
    }
}
