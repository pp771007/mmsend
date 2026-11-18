mod api;
mod cli;
mod console;
mod favorites;
mod gui;
mod icon;
mod icons;
mod install;
mod react;
mod read;
mod replies;
mod session;
mod store;
mod subcommands;
mod target;
mod theme;
mod update;

use std::process::ExitCode;

/// Product name for people (window title, Start menu, messages); `mmsend` is only
/// the command. A debug build says so, so it is never mistaken for the installed one.
pub const APP_NAME: &str = if cfg!(debug_assertions) { "Mattermost 指令傳訊（開發版）" } else { "Mattermost 指令傳訊" };

fn main() -> ExitCode {
    if std::env::args_os().len() > 1 {
        cli::run()
    } else {
        console::release_own_console();
        gui::run()
    }
}
