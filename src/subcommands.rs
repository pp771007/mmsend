//! `mmsend <command>`: everything besides sending, so a script or an AI agent can
//! go from nothing to a sent message without opening the settings window.
//! Every send names a recipient flag first, so a bare first word can never have
//! been meant as a message.

use std::collections::HashSet;

use clap::error::ErrorKind;
use clap::{ArgAction, Parser};

use crate::api::{self, USER_SEARCH_LIMIT};
use crate::cli::{
    EXIT_NOT_LOGGED_IN, EXIT_OTHER, EXIT_TARGET_NOT_FOUND, Failure, login_failure, usage_failure,
    with_retries,
};
use crate::favorites;
use crate::gui;
use crate::install::{self, InstallState, Version};
use crate::session::{Error, Session};
use crate::store;
use crate::target::Target;

pub const LOGIN: &str = "login";
pub const STATUS: &str = "status";
pub const FIND: &str = "find";

/// clap's default headings are English.
const HELP_TEMPLATE: &str = "\
{about}

用法：
  {usage}

參數：
{positionals}

選項：
{options}";

/// Parses the words after `mmsend <command>`; `Ok(Err(help))` when help was asked for.
/// The commands' parsers set `no_binary_name`, since `mmsend find` is two words.
fn parse<T: Parser>() -> Result<Result<T, String>, Failure> {
    match T::try_parse_from(std::env::args_os().skip(2)) {
        Ok(args) => Ok(Ok(args)),
        Err(e) if e.kind() == ErrorKind::DisplayHelp => Ok(Err(e.render().to_string())),
        Err(e) => Err(usage_failure(&e)),
    }
}

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

#[derive(Parser)]
#[command(
    name = "mmsend find",
    about = "以帳號、姓名或頻道名稱尋找收件對象，列出可直接使用的參數。",
    help_template = HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true
)]
struct FindArgs {
    /// 關鍵字，例如姓名、帳號或頻道名稱
    #[arg(value_name = "關鍵字", required = true, num_args = 1..)]
    keyword: Vec<String>,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

/// One line per match, starting with the exact arguments a send takes.
pub fn find() -> Result<String, Failure> {
    let args = match parse::<FindArgs>()? {
        Ok(args) => args,
        Err(help) => return Ok(help),
    };
    let keyword = args.keyword.join(" ").trim().to_owned();
    let needle = keyword.to_lowercase();
    let mut lines = Vec::new();

    let favorites = favorites::load().map_err(|e| Failure::new(EXIT_OTHER, e))?;
    let aliases: Vec<String> = favorites
        .iter()
        .filter(|f| !f.alias.is_empty())
        .filter(|f| [&f.alias, &f.title, &f.subtitle].iter().any(|s| s.to_lowercase().contains(&needle)))
        .map(|f| format!("  {}  {}", Target::alias_args(&f.alias), f.target))
        .collect();
    push_section(&mut lines, "別名", aliases);

    let mut session = Session::load().map_err(login_failure)?;
    let people = with_retries(|| session.call(|c| c.search_users(&keyword))).map_err(login_failure)?;
    let mut people_lines: Vec<String> = people
        .iter()
        .filter(|u| u.delete_at == 0)
        .map(|u| match u.full_name() {
            name if name.is_empty() => format!("  {}", Target::User(u.username.clone()).cli_args()),
            name => format!("  {}  {name}", Target::User(u.username.clone()).cli_args()),
        })
        .collect();
    if people.len() >= USER_SEARCH_LIMIT as usize {
        people_lines.push(format!("  （只列出前 {USER_SEARCH_LIMIT} 位，請改用更完整的關鍵字）"));
    }
    push_section(&mut lines, "人員", people_lines);

    let channels = matching_channels(&mut session, &needle).map_err(login_failure)?;
    push_section(&mut lines, "頻道", channels);

    if lines.is_empty() {
        return Err(Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("找不到符合「{keyword}」的人員、頻道或別名"),
        ));
    }
    Ok(lines.join("\n"))
}

fn push_section(lines: &mut Vec<String>, title: &str, entries: Vec<String>) {
    if !entries.is_empty() {
        lines.push(format!("{title}："));
        lines.extend(entries);
    }
}

/// Public and private channels the user belongs to (the only ones a post can go
/// to) whose name or URL name contains `needle`.
fn matching_channels(session: &mut Session, needle: &str) -> Result<Vec<String>, Error> {
    let teams = with_retries(|| session.call(|c| c.my_teams()))?;
    let show_team = teams.len() > 1;
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for team in &teams {
        for channel in with_retries(|| session.call(|c| c.my_channels(&team.id)))? {
            let postable = matches!(channel.kind.as_str(), api::CHANNEL_OPEN | api::CHANNEL_PRIVATE);
            let matches = channel.display_name.to_lowercase().contains(needle)
                || channel.name.to_lowercase().contains(needle);
            if !postable || !matches || channel.delete_at != 0 || !seen.insert(channel.id.clone()) {
                continue;
            }
            let target = Target::Channel {
                team: team.name.clone(),
                channel: channel.name,
            };
            let team_name = if show_team { format!("（{}）", team.display_name) } else { String::new() };
            lines.push(format!("  {}  {}{team_name}", target.cli_args(), channel.display_name));
        }
    }
    Ok(lines)
}
