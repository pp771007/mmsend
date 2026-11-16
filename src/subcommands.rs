//! `mmsend <command>`: everything besides sending, so a script or an AI agent can
//! go from nothing to a sent message without opening the settings window.
//! Every send names a recipient flag first, so a bare first word can never have
//! been meant as a message.

use crate::cli::{EXIT_NOT_LOGGED_IN, EXIT_OTHER, Failure, login_failure, with_retries};
use crate::gui;
use crate::install::{self, InstallState, Version};
use crate::session::Session;
use crate::store;

pub const LOGIN: &str = "login";
pub const STATUS: &str = "status";

/// Waits while the person logs in; the caller only ever sees the outcome, never the password.
pub fn login() -> Result<String, Failure> {
    if !gui::run_login().map_err(|e| Failure::new(EXIT_OTHER, e))? {
        return Err(Failure::new(EXIT_NOT_LOGGED_IN, "登入視窗已關閉，尚未登入"));
    }
    let config = store::load_config()
        .map_err(|e| Failure::new(EXIT_OTHER, e))?
        .ok_or_else(|| Failure::new(EXIT_OTHER, "登入資料未能儲存"))?;
    Ok(format!("已登入 @{}（{}）", config.username, config.server_url.trim_end_matches('/')))
}

/// What an agent checks first. Asks the server, so "logged in" means sending works.
pub fn status() -> Result<String, Failure> {
    let installed = match install::state() {
        InstallState::Installed | InstallState::Newer(_) => "已安裝，可直接輸入 mmsend",
        InstallState::Outdated => "已安裝其他版本，執行 mmsend install 改裝這一版",
        InstallState::NotInstalled => "未安裝，執行 mmsend install 後可直接輸入 mmsend",
    };
    let head = format!("版本 {}\n{installed}", Version::current());
    let login = Session::load().and_then(|mut session| {
        let me = with_retries(|| session.call(|c| c.me()))?;
        Ok(format!("已登入 {}（{}）", me.label(), session.config.server_url.trim_end_matches('/')))
    });
    match login {
        Ok(line) => Ok(format!("{head}\n{line}")),
        Err(e) => {
            let failure = login_failure(e);
            Err(Failure::new(failure.code, format!("{head}\n{}", failure.message)))
        }
    }
}
