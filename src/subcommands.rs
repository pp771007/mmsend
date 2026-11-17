//! `mmsend <command>`: everything besides sending, so a script or an AI agent can
//! go from nothing to a sent message without opening the settings window.
//! Every send names a recipient flag first, so a bare first word can never have
//! been meant as a message.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{ArgAction, Parser, Subcommand};

use crate::api::{self, ApiError, POSTS_PER_PAGE, Post, PostList, USER_SEARCH_LIMIT};
use crate::cli::{
    EXIT_NO_REPLIES, EXIT_NOT_LOGGED_IN, EXIT_OTHER, EXIT_SEND_FAILED, EXIT_TARGET_NOT_FOUND, EXIT_USAGE,
    Failure, TargetArgs, login_failure, parse_target, summarize_text, usage_failure, with_retries,
};
use crate::favorites::{self, Favorite};
use crate::gui;
use crate::install::{self, COMMAND_NAME, InstallState, Version};
use crate::session::{Error, Session};
use crate::store;
use crate::target::Target;
use crate::update;

pub const LOGIN: &str = "login";
pub const STATUS: &str = "status";
pub const FIND: &str = "find";
pub const ALIAS: &str = "alias";
pub const INSTALL: &str = "install";
pub const UPDATE: &str = "update";

/// Installs the newest release, if this is not it.
pub fn update() -> Result<String, Failure> {
    if let Some(help) = no_arguments(UPDATE, "更新至最新版本（下載後先核對檢查碼），已是最新版本時不做任何事。")? {
        return Ok(help);
    }
    let latest = update::latest().map_err(|e| Failure::new(EXIT_SEND_FAILED, format!("無法檢查更新：{e}")))?;
    update::remember(latest);
    if latest <= Version::current() {
        return Ok(format!("已是最新版本 v{}", Version::current()));
    }
    update::download_and_install(latest).map_err(|e| Failure::new(EXIT_OTHER, e))?;
    let exe = install::installed_exe().map(|path| path.display().to_string()).unwrap_or_default();
    Ok(format!("已更新至 v{latest}：{exe}"))
}
pub const DELETE: &str = "delete";

#[derive(Parser)]
#[command(
    name = "mmsend delete",
    about = "刪除自己傳的訊息。對方若已收到通知（手機、桌面、Email），通知無法收回。",
    override_usage = "mmsend delete <訊息ID>\n  mmsend delete --last [--dry-run]",
    help_template = HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true
)]
struct DeleteArgs {
    /// 傳送成功時顯示的訊息 ID
    #[arg(value_name = "訊息ID", required_unless_present = "last", conflicts_with = "last")]
    post_id: Option<String>,

    /// 刪除這台電腦最近一次以 mmsend 傳送的訊息
    #[arg(long)]
    last: bool,

    /// 試跑：顯示將刪除的訊息，不刪除
    #[arg(long)]
    dry_run: bool,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

pub fn delete() -> Result<String, Failure> {
    let args = match parse::<DeleteArgs>()? {
        Ok(args) => args,
        Err(help) => return Ok(help),
    };
    // `sent`: when and to whom, known for --last; a scheduled job may have sent
    // the newest message, which is what the dry run lets someone notice.
    let (post_id, sent) = match args.post_id {
        Some(id) => (id.trim().to_owned(), None),
        // Always the newest send: run twice, the second finds it already deleted
        // instead of reaching back to an older message.
        None => last_sent().map(|(id, sent)| (id, Some(sent)))?,
    };
    let mut session = Session::load().map_err(login_failure)?;
    let gone = || Failure::new(EXIT_TARGET_NOT_FOUND, format!("找不到訊息 {post_id}：可能已經刪除"));
    let post = with_retries(|| session.call(|c| c.post(&post_id))).map_err(|e| match e {
        Error::Api(ApiError::NotFound) => gone(),
        e => login_failure(e),
    })?;
    refuse_unless_own(&post, &session.config.user_id)?;
    let summary = summarize_text(&post.message);
    let what = if summary.is_empty() { format!(" {post_id}") } else { format!("「{summary}」") };
    let when = sent.map(|sent| format!("（{sent}）")).unwrap_or_default();
    if args.dry_run {
        return Ok(format!("試跑，未刪除\n將刪除訊息{what}{when}"));
    }
    match with_retries(|| session.call(|c| c.delete_post(&post_id))) {
        // A retry after a lost response finds it gone: that is the goal reached.
        Ok(()) | Err(Error::Api(ApiError::NotFound)) => {}
        Err(Error::Api(ApiError::Forbidden)) => {
            return Err(Failure::new(
                EXIT_TARGET_NOT_FOUND,
                "伺服器不允許刪除這則訊息（管理員可能關閉了刪除自己訊息的權限）",
            ));
        }
        Err(e) => return Err(login_failure(e)),
    }
    Ok(format!("已刪除訊息{what}{when}"))
}

/// The newest message this PC sent, and "when, to whom".
fn last_sent() -> Result<(String, String), Failure> {
    store::load_send_records()
        .map_err(|e| Failure::new(EXIT_OTHER, e))?
        .into_iter()
        .find_map(|record| {
            let sent = format!("{} 傳給 {}", record.time, record.target);
            record.post_id.filter(|_| record.ok).map(|id| (id, sent))
        })
        .ok_or_else(|| Failure::new(EXIT_TARGET_NOT_FOUND, "傳送紀錄中沒有傳送成功的訊息"))
}

pub const REPLIES: &str = "replies";

/// Scanning further back than this many posts after the message stops; an old
/// message in a busy conversation would otherwise page through its whole history.
const MAX_SCANNED_POSTS: usize = 1000;
/// Mattermost ids are 26 letters and digits.
const POST_ID_LENGTH: usize = 26;
/// Join-and-leave notices and the like, which nobody wrote.
const SYSTEM_POST_PREFIX: &str = "system_";

#[derive(Parser)]
#[command(
    name = "mmsend replies",
    about = "列出別人在您傳的訊息之後回的訊息，附件下載到暫存資料夾。\n私訊與群組列出之後的所有訊息；頻道只列出那則訊息的討論串。還沒有回覆時結束代碼為 7。",
    override_usage = "mmsend replies <訊息ID>\n  mmsend replies --last",
    help_template = HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true
)]
struct RepliesArgs {
    /// 傳送成功時顯示的訊息 ID
    #[arg(value_name = "訊息ID", required_unless_present = "last", conflicts_with = "last")]
    post_id: Option<String>,

    /// 查這台電腦最近一次以 mmsend 傳送的訊息
    #[arg(long)]
    last: bool,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

/// Only for the user's own messages: given any post id, this would read any
/// conversation the account can see, which is more than "what did they answer".
pub fn replies() -> Result<String, Failure> {
    let args = match parse::<RepliesArgs>()? {
        Ok(args) => args,
        Err(help) => return Ok(help),
    };
    let post_id = match args.post_id {
        Some(id) => id.trim().to_owned(),
        None => last_sent()?.0,
    };
    // The server answers a malformed id with a 400, which would read as a connection problem.
    if post_id.len() != POST_ID_LENGTH || !post_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Failure::new(
            EXIT_USAGE,
            format!("「{post_id}」不是訊息 ID：請用傳送成功時顯示的 {POST_ID_LENGTH} 碼英數字"),
        ));
    }
    let mut session = Session::load().map_err(login_failure)?;
    let me = session.config.user_id.clone();
    let post = with_retries(|| session.call(|c| c.post(&post_id))).map_err(|e| match e {
        Error::Api(ApiError::NotFound) => {
            Failure::new(EXIT_TARGET_NOT_FOUND, format!("找不到訊息 {post_id}：可能已經刪除"))
        }
        e => login_failure(e),
    })?;
    if post.user_id != me {
        return Err(Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("訊息 {post_id} 不是您傳的：只能查自己傳的訊息的回覆"),
        ));
    }
    let channel = with_retries(|| session.call(|c| c.channel(&post.channel_id))).map_err(login_failure)?;
    // In a channel, what follows a message is mostly other conversations; a reply
    // to it is what was written in its thread.
    let (lists, more) = if matches!(channel.kind.as_str(), api::CHANNEL_DIRECT | api::CHANNEL_GROUP) {
        posts_after(&mut session, &post)?
    } else {
        (vec![with_retries(|| session.call(|c| c.thread(&post.id))).map_err(login_failure)?], false)
    };
    let replies = replies_in(lists, &me);
    if replies.is_empty() {
        return Err(Failure::new(EXIT_NO_REPLIES, "還沒有回覆"));
    }

    let ids: Vec<String> = replies.iter().map(|reply| reply.user_id.clone()).collect::<HashSet<_>>().into_iter().collect();
    // Without names the replies still read; the handle is only missing.
    let names: HashMap<String, String> = with_retries(|| session.call(|c| c.users_by_ids(&ids)))
        .map(|users| users.into_iter().map(|user| (user.id.clone(), user.label())).collect())
        .unwrap_or_default();
    let folder = std::env::temp_dir().join(format!("{COMMAND_NAME}-replies")).join(&post.id);
    let mut taken = HashSet::new();
    let mut blocks = vec![format!("{} 則回覆：", replies.len())];
    for reply in &replies {
        let who = names.get(&reply.user_id).cloned().unwrap_or_else(|| reply.user_id.clone());
        // Indented, so a line someone wrote cannot pass for one of these headers.
        let mut lines = vec![format!("{} {who}", store::local_time(reply.create_at))];
        lines.extend(reply.message.lines().map(|line| format!("  {line}")));
        for file in &reply.metadata.files {
            let saved = save_attachment(&mut session, &folder, file, &mut taken);
            lines.push(match saved {
                Ok(path) => format!("  附件：{}", path.display()),
                Err(problem) => format!("  附件「{}」下載失敗：{problem}", file.name),
            });
        }
        blocks.push(lines.join("\n"));
    }
    if more {
        blocks.push(format!("（只查了之後的 {MAX_SCANNED_POSTS} 則訊息，更新的請在 Mattermost 查看）"));
    }
    Ok(blocks.join("\n\n"))
}

/// The pages after the message, and whether there are more than MAX_SCANNED_POSTS.
fn posts_after(session: &mut Session, post: &Post) -> Result<(Vec<PostList>, bool), Failure> {
    let mut lists = Vec::new();
    for page in 0..(MAX_SCANNED_POSTS / POSTS_PER_PAGE) as u32 {
        let list = with_retries(|| session.call(|c| c.posts_after(&post.channel_id, &post.id, page)))
            .map_err(login_failure)?;
        let full = list.order.len() >= POSTS_PER_PAGE;
        lists.push(list);
        if !full {
            return Ok((lists, false));
        }
    }
    Ok((lists, true))
}

/// What others wrote, oldest first. A page's `posts` also carries the thread starters
/// of its replies, which can be older than the message, so only `order` counts.
fn replies_in(lists: Vec<PostList>, me: &str) -> Vec<Post> {
    let mut replies: Vec<Post> = lists
        .into_iter()
        .flat_map(|mut list| {
            let order = std::mem::take(&mut list.order);
            order.into_iter().filter_map(move |id| list.posts.remove(&id))
        })
        .filter(|post| post.user_id != me && !post.kind.starts_with(SYSTEM_POST_PREFIX))
        .collect();
    replies.sort_by_key(|post| post.create_at);
    replies
}

/// Saves an attachment under `folder`; two with the same name keep both.
fn save_attachment(
    session: &mut Session,
    folder: &std::path::Path,
    file: &api::FileInfo,
    taken: &mut HashSet<String>,
) -> Result<PathBuf, String> {
    let mut name = safe_file_name(&file.name, &file.id);
    if !taken.insert(name.to_lowercase()) {
        name = format!("{}-{name}", file.id);
    }
    let bytes = with_retries(|| session.call(|c| c.download_file(&file.id))).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let path = folder.join(name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Someone else chose the name: it must stay a plain name inside the folder, so
/// `..\..\x` or `C:\x` cannot write elsewhere.
fn safe_file_name(name: &str, fallback: &str) -> String {
    const NOT_IN_NAMES: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || NOT_IN_NAMES.contains(&c) { '_' } else { c })
        .collect();
    // Windows drops trailing dots and spaces, so ".." would name the parent folder.
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']);
    if cleaned.is_empty() { fallback.to_owned() } else { cleaned.to_owned() }
}

/// The server lets an admin delete anyone's post; this command only ever takes back
/// the user's own.
fn refuse_unless_own(post: &Post, my_user_id: &str) -> Result<(), Failure> {
    if post.user_id == my_user_id {
        return Ok(());
    }
    Err(Failure::new(EXIT_TARGET_NOT_FOUND, format!("訊息 {} 不是您傳的，不能刪除", post.id)))
}

/// What the Install button does: copy to a fixed folder on PATH, plus a Start menu entry.
pub fn install() -> Result<String, Failure> {
    let about = "安裝這個檔案：之後在新開的視窗可直接輸入 mmsend，並顯示安裝後的完整路徑。";
    if let Some(help) = no_arguments(INSTALL, about)? {
        return Ok(help);
    }
    let exe = install::installed_exe()
        .ok_or_else(|| Failure::new(EXIT_OTHER, "找不到 LOCALAPPDATA 環境變數"))?
        .display()
        .to_string();
    match install::state() {
        InstallState::Installed => return Ok(format!("已安裝：{exe}")),
        InstallState::Newer(version) => return Ok(format!("已安裝較新的版本 {version}：{exe}")),
        InstallState::NotInstalled | InstallState::Different(_) => {}
    }
    install::install().map_err(|e| Failure::new(EXIT_OTHER, e))?;
    Ok(format!(
        "已安裝：{exe}\n新開啟的 cmd / PowerShell 才能直接輸入 mmsend；已開啟的視窗請使用上面的完整路徑"
    ))
}

/// clap's default headings are English.
const HELP_TEMPLATE: &str = "\
{about}

用法：
  {usage}

參數：
{positionals}

選項：
{options}";

const NO_ARGUMENTS_HELP_TEMPLATE: &str = "\
{about}

用法：
  {usage}

選項：
{options}";

/// For the commands that take no arguments: `Some(help)` for -h, and any other word
/// refused, so that a look at `mmsend install -h` never installs.
fn no_arguments(name: &'static str, about: &'static str) -> Result<Option<String>, Failure> {
    no_arguments_in(name, about, std::env::args_os().skip(2))
}

fn no_arguments_in(
    name: &'static str,
    about: &'static str,
    words: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<Option<String>, Failure> {
    let command = clap::Command::new(name)
        .about(about)
        .override_usage(format!("mmsend {name}"))
        .help_template(NO_ARGUMENTS_HELP_TEMPLATE)
        .no_binary_name(true)
        .disable_help_flag(true)
        .arg(clap::Arg::new("help").short('h').long("help").action(ArgAction::Help).help("顯示說明"));
    match command.try_get_matches_from(words) {
        Ok(_) => Ok(None),
        Err(e) if e.kind() == ErrorKind::DisplayHelp => Ok(Some(e.render().to_string())),
        Err(_) => Err(Failure::new(EXIT_USAGE, format!("mmsend {name} 不需要參數"))),
    }
}

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
    let about = "開啟登入視窗，登入後自動關閉並顯示登入的帳號；沒登入就關閉視窗時，結束代碼為 3。";
    if let Some(help) = no_arguments(LOGIN, about)? {
        return Ok(help);
    }
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
    let about = "顯示版本、是否已安裝、有沒有新版，以及登入狀態（會連線確認）。";
    if let Some(help) = no_arguments(STATUS, about)? {
        return Ok(help);
    }
    let current = Version::current();
    let installed = match install::state() {
        InstallState::Installed | InstallState::Newer(_) if install::on_current_path() => {
            "已安裝，可直接輸入 mmsend".to_owned()
        }
        InstallState::Installed | InstallState::Newer(_) => {
            let exe = install::installed_exe().map(|path| path.display().to_string()).unwrap_or_default();
            format!("已安裝；這個視窗還不認得 mmsend 指令，請用完整路徑：{exe}")
        }
        InstallState::Different(Some(version)) if version != current => {
            format!("電腦上安裝的是 v{version}，執行 mmsend install 安裝這一版（v{current}）")
        }
        InstallState::Different(_) => "電腦上安裝的是另一份程式，執行 mmsend install 安裝這一版".to_owned(),
        InstallState::NotInstalled => "未安裝，執行 mmsend install 後可直接輸入 mmsend".to_owned(),
    };
    let mut head = format!("版本 {}\n{installed}", Version::current());
    // Not knowing is no failure of status: a line is left out, the exit code is not affected.
    if let Ok(latest) = update::latest() {
        update::remember(latest);
        head.push('\n');
        head.push_str(&if latest > Version::current() {
            format!("有新版 v{latest}：執行 mmsend {UPDATE} 更新")
        } else {
            "已是最新版本".to_owned()
        });
    }
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
        .map(alias_line)
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

const ALIAS_HELP_TEMPLATE: &str = "\
{about}

用法：
  mmsend alias                          列出別名
  mmsend alias set <別名> --user <帳號>  設定別名（也可用 --channel、--group）
  mmsend alias remove <別名>            移除別名

換人時重新 set 同一個別名即可，所有使用 --to 的批次檔都不用修改。";

#[derive(Parser)]
#[command(
    name = "mmsend alias",
    about = "列出或設定別名；傳送時以 mmsend --to <別名> 指定對象。",
    help_template = ALIAS_HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct AliasArgs {
    #[command(subcommand)]
    command: Option<AliasCommand>,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

#[derive(Subcommand)]
enum AliasCommand {
    /// 把別名設定給一個對象；別名原本屬於別人時改為這個對象
    Set {
        #[arg(value_name = "別名")]
        alias: String,
        #[command(flatten)]
        target: TargetArgs,
    },
    /// 移除別名（釘選保留）
    Remove {
        #[arg(value_name = "別名")]
        alias: String,
    },
}

pub fn alias() -> Result<String, Failure> {
    let args = match parse::<AliasArgs>()? {
        Ok(args) => args,
        Err(help) => return Ok(help),
    };
    let mut favorites = favorites::load().map_err(|e| Failure::new(EXIT_OTHER, e))?;
    match args.command {
        None => {
            let lines: Vec<String> = favorites.iter().filter(|f| !f.alias.is_empty()).map(alias_line).collect();
            if lines.is_empty() {
                return Ok("尚未設定別名：以 mmsend alias set <別名> --user <帳號> 設定".to_owned());
            }
            Ok(lines.join("\n"))
        }
        Some(AliasCommand::Set { alias, target }) => {
            let alias = alias.trim().to_owned();
            let target = parse_target(&target)?;
            // The point of an alias is following a new person without editing scripts,
            // so taking it over is the normal case, not a conflict.
            let previous = favorites::release_alias(&mut favorites, &alias, &target);
            if let Some(problem) = favorites::alias_problem(&alias, &favorites, &target) {
                return Err(Failure::new(EXIT_USAGE, problem));
            }
            if alias.is_empty() {
                return Err(Failure::new(EXIT_USAGE, "請指定別名"));
            }
            let mut session = Session::load().map_err(login_failure)?;
            let recipient = with_retries(|| target.look_up(&mut session)).map_err(|e| match e {
                Error::Api(ApiError::NotFound) => {
                    Failure::new(EXIT_TARGET_NOT_FOUND, format!("找不到 {target}：請以 mmsend find <姓名> 查詢"))
                }
                e => login_failure(e),
            })?;
            // A pin holds one alias, so scripts still using its old one would stop
            // working: say so.
            let mut replaced = None;
            match favorites.iter_mut().find(|f| f.target == target) {
                Some(pinned) => {
                    let old = std::mem::replace(&mut pinned.alias, alias.clone());
                    if !old.is_empty() && old != alias {
                        replaced = Some(old);
                    }
                }
                None => favorites.push(Favorite {
                    alias: alias.clone(),
                    title: if recipient.name.is_empty() { target.to_string() } else { recipient.name.clone() },
                    subtitle: if recipient.name.is_empty() { String::new() } else { target.to_string() },
                    user_id: recipient.user_id().map(str::to_owned),
                    private: recipient.private,
                    target,
                }),
            }
            favorites::save(&favorites).map_err(|e| Failure::new(EXIT_OTHER, e))?;
            let moved = previous.map(|from| format!("（原本是 {from}）")).unwrap_or_default();
            let mut lines = vec![format!("別名「{alias}」→ {}{moved}", recipient.label)];
            if let Some(old) = replaced {
                lines.push(format!("一個對象只能有一個別名，原本的別名「{old}」已失效"));
            }
            lines.push(format!("傳送時使用：mmsend {} \"訊息\"", Target::alias_args(&alias)));
            Ok(lines.join("\n"))
        }
        Some(AliasCommand::Remove { alias }) => {
            let Some(favorite) = favorites
                .iter_mut()
                .find(|f| !f.alias.is_empty() && f.alias.to_lowercase() == alias.trim().to_lowercase())
            else {
                return Err(Failure::new(EXIT_TARGET_NOT_FOUND, format!("找不到別名「{alias}」")));
            };
            favorite.alias.clear();
            let target = favorite.target.to_string();
            favorites::save(&favorites).map_err(|e| Failure::new(EXIT_OTHER, e))?;
            Ok(format!("已移除別名「{alias}」（{target} 仍保留釘選）"))
        }
    }
}

/// `--to 值班  @alice 王小明`: the arguments, then who they reach.
fn alias_line(favorite: &Favorite) -> String {
    let who = if favorite.title.starts_with('@') || favorite.title == favorite.target.to_string() {
        favorite.target.to_string()
    } else {
        format!("{} {}", favorite.target, favorite.title)
    };
    format!("  {}  {who}", Target::alias_args(&favorite.alias))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_your_own_posts_can_be_deleted() {
        let post = |user_id: &str| Post {
            id: "p1".into(),
            user_id: user_id.into(),
            ..Default::default()
        };
        assert!(refuse_unless_own(&post("me"), "me").is_ok());
        assert!(refuse_unless_own(&post("someone-else"), "me").is_err());
    }

    #[test]
    fn replies_are_what_others_wrote_after_the_message_oldest_first() {
        let post = |id: &str, user_id: &str, create_at: i64, kind: &str| Post {
            id: id.into(),
            user_id: user_id.into(),
            create_at,
            kind: kind.into(),
            ..Default::default()
        };
        let list = PostList {
            order: vec!["late".into(), "mine".into(), "joined".into(), "early".into()],
            posts: [
                post("late", "howard", 30, ""),
                post("mine", "me", 25, ""),
                post("joined", "howard", 20, "system_join_channel"),
                post("early", "howard", 10, ""),
                // The thread starter of a reply, sent before the message asked about.
                post("older-root", "howard", 1, ""),
            ]
            .into_iter()
            .map(|post| (post.id.clone(), post))
            .collect(),
        };
        let ids: Vec<String> = replies_in(vec![list], "me").into_iter().map(|post| post.id).collect();
        assert_eq!(ids, ["early", "late"]);
    }

    #[test]
    fn an_attachment_name_cannot_leave_the_folder() {
        assert_eq!(safe_file_name("IMG_0001.PNG", "f1"), "IMG_0001.PNG");
        assert_eq!(safe_file_name(r"..\..\evil.bat", "f1"), ".._.._evil.bat");
        assert_eq!(safe_file_name("C:/x.txt", "f1"), "C__x.txt");
        assert_eq!(safe_file_name("..", "f1"), "f1");
    }

    #[test]
    fn a_command_without_arguments_answers_help_instead_of_running() {
        let words = |list: &[&str]| list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let run = |list: &[&str]| no_arguments_in(INSTALL, "安裝", words(list));
        assert!(matches!(run(&["-h"]), Ok(Some(help)) if help.contains("mmsend install")));
        assert!(matches!(run(&[]), Ok(None)));
        assert!(matches!(run(&["now"]), Err(failure) if failure.code == EXIT_USAGE));
    }
}
