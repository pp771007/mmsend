//! New releases on GitHub: which version is the latest, and putting it in place.
//! Only the window (at most daily) and `mmsend status` / `update` look; a send never
//! does, since scheduled jobs often run with no network to spare.

use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use reqwest::header::LOCATION;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::install::{self, Version};
use crate::store;

/// Its `/latest` redirects to the newest release's tag page. Read instead of the
/// GitHub API, which allows 60 anonymous requests an hour per IP: an office behind
/// one address would run out.
const RELEASES_URL: &str = "https://github.com/pp771007/mmsend/releases";
/// Debug builds only: a local stand-in for GitHub, to try updates without publishing.
#[cfg(debug_assertions)]
const DEV_RELEASES_VARIABLE: &str = "MMSEND_DEV_RELEASES_URL";
const EXE_ASSET: &str = "mmsend.exe";
const CHECKSUM_ASSET: &str = "mmsend.exe.sha256";
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a download may go without receiving anything. reqwest's blocking client
/// applies its timeout to each wait (the request, then every read), so this is not a
/// cap on the whole file: GitHub's file host has served the ~17 MB exe at ~50 KB/s,
/// over five minutes, and a cap of that size failed every such update.
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;
/// How long the window trusts its last look before asking GitHub again.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const STATE_FILE: &str = "update.json";

fn releases_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var(DEV_RELEASES_VARIABLE) {
        return url.trim_end_matches('/').to_owned();
    }
    RELEASES_URL.to_owned()
}

/// `redirect`: whether to follow redirects. The version check must not (the redirect
/// target *is* the answer); downloads must, since GitHub serves every release asset
/// through a redirect to its file host.
fn client(timeout: Duration, redirect: Policy) -> Result<Client, String> {
    Client::builder()
        .timeout(timeout)
        .redirect(redirect)
        .user_agent(concat!("mmsend/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn download_client() -> Result<Client, String> {
    client(DOWNLOAD_STALL_TIMEOUT, Policy::default())
}

/// The newest published version.
pub fn latest() -> Result<Version, String> {
    let response = client(CHECK_TIMEOUT, Policy::none())?
        .head(format!("{}/latest", releases_url()))
        .send()
        .map_err(|e| format!("無法連線至 GitHub：{e}"))?;
    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| format!("GitHub 回應異常（HTTP {}）", response.status().as_u16()))?;
    version_from_location(location).ok_or_else(|| format!("無法辨識最新版本：{location}"))
}

/// `.../releases/tag/v0.2.0` → 0.2.0
fn version_from_location(location: &str) -> Option<Version> {
    let (_, tag) = location.trim_end_matches('/').rsplit_once("/tag/")?;
    Version::parse(tag)
}

#[derive(Serialize, Deserialize)]
struct CheckState {
    /// Seconds since the epoch.
    checked_at: u64,
    latest: String,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// The window's daily look: the version found within the last day, or `None` when it is time to ask again.
pub fn recent_latest() -> Option<Version> {
    let state: CheckState = store::load_json(STATE_FILE).ok().flatten()?;
    let fresh = now_secs().saturating_sub(state.checked_at) < CHECK_INTERVAL.as_secs();
    fresh.then(|| Version::parse(&state.latest)).flatten()
}

pub fn remember(latest: Version) {
    // Only saves a request tomorrow; nothing breaks if it cannot be written.
    let _ = store::save_json(
        STATE_FILE,
        &CheckState {
            checked_at: now_secs(),
            latest: latest.to_string(),
        },
    );
}

/// Downloads `version`, checks it against its published SHA-256, and installs it.
/// `progress` gets the exe's download percentage each time it changes (only when
/// the server says how big the file is).
pub fn download_and_install(version: Version, progress: impl FnMut(u8)) -> Result<(), String> {
    let base = format!("{}/download/v{version}", releases_url());
    let client = download_client()?;
    let exe = fetch(&client, &format!("{base}/{EXE_ASSET}"), progress)?;
    let checksum = String::from_utf8_lossy(&fetch(&client, &format!("{base}/{CHECKSUM_ASSET}"), |_| {})?).into_owned();
    let expected = checksum.split_whitespace().next().unwrap_or_default();
    if !sha256_hex(&exe).eq_ignore_ascii_case(expected) {
        return Err("下載的檔案與發佈的檢查碼不符，未更新".to_owned());
    }
    let downloaded = download_path(version);
    std::fs::write(&downloaded, &exe).map_err(|e| format!("無法寫入 {}：{e}", downloaded.display()))?;
    let result = install::install_from(&downloaded);
    let _ = std::fs::remove_file(&downloaded);
    result
}

/// Read piece by piece rather than with `bytes()`, which puts the whole body under
/// one timeout; each `read` gets its own (see `DOWNLOAD_STALL_TIMEOUT`).
fn fetch(client: &Client, url: &str, mut progress: impl FnMut(u8)) -> Result<Vec<u8>, String> {
    let mut response = client.get(url).send().map_err(|e| format!("下載失敗：{e}"))?;
    if !response.status().is_success() {
        return Err(format!("下載失敗（HTTP {}）：{url}", response.status().as_u16()));
    }
    let total = response.content_length().filter(|&length| length > 0);
    let mut body = Vec::with_capacity(total.unwrap_or_default() as usize);
    let mut chunk = vec![0; DOWNLOAD_CHUNK_BYTES];
    let mut reported = None;
    loop {
        let read = response.read(&mut chunk).map_err(|e| {
            if stalled(&e) { STALLED_MESSAGE.to_owned() } else { format!("下載中斷：{e}") }
        })?;
        if read == 0 {
            return Ok(body);
        }
        body.extend_from_slice(&chunk[..read]);
        if let Some(total) = total {
            let percent = (body.len() as u64 * 100 / total).min(100) as u8;
            if reported != Some(percent) {
                reported = Some(percent);
                progress(percent);
            }
        }
    }
}

/// reqwest's own wording for this is "error decoding response body".
const STALLED_MESSAGE: &str = "下載中斷：太久沒有收到資料，請稍後再試";

fn stalled(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
        .is_some_and(reqwest::Error::is_timeout)
}

fn download_path(version: Version) -> PathBuf {
    std::env::temp_dir().join(format!("mmsend-{version}-{}.exe", std::process::id()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub answers every release download with a redirect to its file host.
    #[test]
    fn downloads_follow_the_redirect_to_the_file() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut stream = stream.unwrap();
                let mut request_line = String::new();
                BufReader::new(&stream).read_line(&mut request_line).unwrap();
                let reply = if request_line.contains("/download/") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/files/mmsend.exe\r\n\
                         Content-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nexe".to_owned()
                };
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        let url = format!("http://127.0.0.1:{port}/download/v9.9.9/mmsend.exe");
        assert_eq!(fetch(&download_client().unwrap(), &url, |_| {}).unwrap(), b"exe");
        server.join().unwrap();
    }

    /// A slow but steady download finishes however long it takes in all; only a
    /// stall ends it. (A whole-file limit failed every update on a slow line.)
    #[test]
    fn a_slow_download_that_keeps_arriving_is_not_cut_off() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        const BODY: &[u8] = b"exe!";
        const STALL_LIMIT: Duration = Duration::from_millis(500);
        const BYTE_INTERVAL: Duration = Duration::from_millis(300);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(&stream).read_line(&mut request_line).unwrap();
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", BODY.len());
            stream.write_all(head.as_bytes()).unwrap();
            // 4 bytes, 300 ms apart: 1.2 s in all, well past the 0.5 s limit.
            for byte in BODY {
                std::thread::sleep(BYTE_INTERVAL);
                stream.write_all(&[*byte]).unwrap();
                stream.flush().unwrap();
            }
        });
        let mut percents = Vec::new();
        let client = client(STALL_LIMIT, Policy::default()).unwrap();
        let body = fetch(&client, &format!("http://127.0.0.1:{port}/mmsend.exe"), |p| percents.push(p)).unwrap();
        assert_eq!(body, BODY);
        assert_eq!(percents.last(), Some(&100));
        server.join().unwrap();
    }

    #[test]
    fn a_download_that_stops_arriving_fails_and_says_so() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        const STALL_LIMIT: Duration = Duration::from_millis(300);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(&stream).read_line(&mut request_line).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nex")
                .unwrap();
            // Holds the connection open with half the body sent until the client gives up.
            let _ = done_rx.recv();
        });
        let client = client(STALL_LIMIT, Policy::default()).unwrap();
        let result = fetch(&client, &format!("http://127.0.0.1:{port}/mmsend.exe"), |_| {});
        assert_eq!(result, Err(STALLED_MESSAGE.to_owned()));
        done_tx.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn latest_version_is_read_from_the_tag_redirect() {
        let found = version_from_location("https://github.com/pp771007/mmsend/releases/tag/v0.2.0");
        assert_eq!(found, Version::parse("0.2.0"));
        assert!(found > Version::parse("0.1.9"));
        assert_eq!(version_from_location("https://github.com/pp771007/mmsend/releases"), None);
    }
}
