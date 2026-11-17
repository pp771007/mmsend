//! `mmsend --user alice "message"` — the scripted side of the exe.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, CommandFactory, FromArgMatches, Parser};

use crate::APP_NAME;
use crate::api::{ApiError, NewPost};
use crate::console;
use crate::favorites;
use crate::session::{Error, Session};
use crate::store::{self, SendRecord};
use crate::subcommands;
use crate::target::{GROUP_OTHER_MEMBERS, Target};

/// Exit codes scripts can branch on. 2 matches what clap uses for bad arguments.
pub(crate) const EXIT_OTHER: u8 = 1;
pub(crate) const EXIT_USAGE: u8 = 2;
pub(crate) const EXIT_NOT_LOGGED_IN: u8 = 3;
pub(crate) const EXIT_LOGIN_EXPIRED: u8 = 4;
pub(crate) const EXIT_TARGET_NOT_FOUND: u8 = 5;
pub(crate) const EXIT_SEND_FAILED: u8 = 6;
pub(crate) const EXIT_NO_REPLIES: u8 = 7;

const EXIT_CODES: [(u8, &str); 8] = [
    (0, "成功"),
    (EXIT_OTHER, "其他錯誤"),
    (EXIT_USAGE, "參數錯誤或訊息為空"),
    (EXIT_NOT_LOGGED_IN, "尚未登入"),
    (EXIT_LOGIN_EXPIRED, "登入已失效（密碼已變更、權杖被撤銷等）"),
    (EXIT_TARGET_NOT_FOUND, "找不到收件對象，或無權限"),
    (EXIT_SEND_FAILED, "連線或伺服器錯誤"),
    (EXIT_NO_REPLIES, "還沒有回覆（mmsend replies）"),
];

/// Waits before the second and third attempt when the network or server hiccups
/// (a scheduled job at night has nobody to rerun it).
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(10)];

/// Mattermost's default limit on a post's length, in characters.
const MAX_POST_CHARS: usize = 16383;
/// Mattermost accepts at most this many files on one post.
const MAX_FILES_PER_POST: usize = 10;
/// A message too long for one post is sent whole as this attachment, with the
/// first lines shown in the post itself.
const FULL_TEXT_FILE_NAME: &str = "完整內容.txt";
const PREVIEW_LINES: usize = 20;
const PREVIEW_MAX_CHARS: usize = 2000;
/// How much of the message the send log keeps.
const SUMMARY_CHARS: usize = 60;
/// How much of the message a dry run prints before saying how many lines remain.
const DRY_RUN_LINES: usize = 10;

const STDIN_MARKER: &str = "-";
/// What cmd users type out of habit; it would otherwise be sent as a message.
const WINDOWS_HELP_FLAG: &str = "/?";

/// Hand-aligned (CJK characters are two columns wide).
const HELP_TEMPLATE: &str = "\
{about}

用法：
  mmsend --user <帳號> \"訊息\"           傳送私訊
  mmsend --channel <團隊/頻道> \"訊息\"   傳送至頻道
  mmsend --group <帳號,帳號> \"訊息\"     傳送至群組
  mmsend --to <別名> \"訊息\"             傳送給在設定視窗取的別名
  mmsend --user <帳號> --file 訊息.txt  從文字檔讀取訊息（適用於多行或含雙引號）
  mmsend --user <帳號> --attach <檔案>  附加檔案，可同時加上訊息
  <其他指令> | mmsend --user <帳號>     傳送其他指令的輸出
  mmsend install                        安裝：之後在新開的視窗可直接輸入 mmsend
  mmsend update                         更新至最新版本
  mmsend login                          開啟登入視窗，登入後自動關閉
  mmsend status                         顯示版本、安裝與登入狀態
  mmsend find <關鍵字>                  以姓名、帳號或頻道名稱尋找收件對象
  mmsend alias                          列出或設定別名（mmsend alias -h）
  mmsend delete <訊息ID>                刪除自己傳的訊息；--last 刪除最近一則
  mmsend replies <訊息ID>               列出對方的回覆，附件存到暫存資料夾
  mmsend                                開啟設定視窗

參數：
{options}{after-help}";

const EXAMPLES: &str = "\
範例：
  mmsend --user alice \"報表跑完了\"
  mmsend --channel dev/town-square \"部署完成\"
  mmsend --group alice,bob \"會議改到三點\"
  mmsend --user alice --attach 日報.xlsx \"今日報表\"
  ipconfig | mmsend --user alice --code";

/// Without it, multi-line messages get written as plain run-on lines for fear of
/// stray formatting.
const MARKDOWN_NOTE: &str = "\
訊息以 Mattermost 的 Markdown 顯示：多項內容用「- 」清單、段落之間空一行、重點用 **粗體**；
加 --code 則整段顯示為程式碼區塊，適合指令輸出。";

#[derive(Parser)]
#[command(
    name = "mmsend",
    version,
    about = "從命令列傳送 Mattermost 訊息。",
    help_template = HELP_TEMPLATE,
    disable_help_flag = true,
    disable_version_flag = true
)]
struct Args {
    #[command(flatten)]
    target: TargetArgs,

    /// 從文字檔讀取訊息
    #[arg(long, value_name = "檔案", conflicts_with = "message")]
    file: Option<PathBuf>,

    /// 附加檔案，可重複指定（最多 10 個）
    #[arg(long, value_name = "檔案")]
    attach: Vec<PathBuf>,

    /// 以程式碼區塊傳送，保留指令輸出的對齊
    #[arg(long)]
    code: bool,

    /// 試跑：檢查收件對象（顯示姓名）與內容，不傳送
    #[arg(long)]
    dry_run: bool,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,

    /// 顯示版本
    #[arg(short = 'V', long, action = ArgAction::Version)]
    version: Option<bool>,

    /// 訊息內容；省略時讀取其他指令的輸出
    #[arg(value_name = "訊息")]
    message: Option<String>,
}

/// Who to send to; also what `mmsend alias set` points an alias at.
#[derive(clap::Args)]
#[group(id = "target", required = true, multiple = false)]
pub(crate) struct TargetArgs {
    /// 傳送私訊給指定的 Mattermost 帳號
    #[arg(long, value_name = "帳號")]
    user: Option<String>,

    /// 傳送至頻道，使用網址中的名稱，例如 dev/town-square
    #[arg(long, value_name = "團隊/頻道")]
    channel: Option<String>,

    /// 傳送至群組，列出除您以外的成員帳號，例如 alice,bob
    #[arg(long, value_name = "帳號,帳號")]
    group: Option<String>,

    /// 傳送給在設定視窗釘選時取的別名
    #[arg(long, value_name = "別名")]
    to: Option<String>,

    /// Raw channel id; kept for scripts written before --group existed.
    #[arg(long, value_name = "ID", hide = true)]
    channel_id: Option<String>,
}

fn command() -> clap::Command {
    let exit_codes: String = EXIT_CODES
        .iter()
        .map(|(code, meaning)| format!("\n  {code}  {meaning}"))
        .collect();
    Args::command().after_help(format!(
        "{EXAMPLES}\n\n{MARKDOWN_NOTE}\n\n結束代碼（批次檔可用 if errorlevel 判斷）：{exit_codes}"
    ))
}

fn parse_args() -> Result<Args, clap::Error> {
    let matches = command().try_get_matches()?;
    Args::from_arg_matches(&matches)
}

/// clap's own messages are English; these cover what people actually get wrong.
pub(crate) fn usage_failure(error: &clap::Error) -> Failure {
    let arg = error
        .get(ContextKind::InvalidArg)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let problem = match error.kind() {
        // The recipient group shows as `<--user <帳號>|--channel ...>`, a missing
        // positional (`mmsend find` without a keyword) as `<關鍵字>...`.
        ErrorKind::MissingRequiredArgument if arg.contains("--user") => {
            "請指定收件對象：--user、--channel、--group 或 --to".to_owned()
        }
        ErrorKind::MissingRequiredArgument => format!("缺少{}", arg.trim_end_matches('.')),
        ErrorKind::ArgumentConflict => {
            "參數衝突：收件對象只能指定一個，訊息與 --file 也只能擇一".to_owned()
        }
        ErrorKind::UnknownArgument if arg.starts_with('-') => format!("無法辨識的參數 {arg}"),
        ErrorKind::UnknownArgument => {
            format!("多餘的參數「{arg}」：訊息含有空格時，請以雙引號括住")
        }
        ErrorKind::InvalidValue | ErrorKind::NoEquals => format!("{arg} 後面須指定內容"),
        _ => error.to_string(),
    };
    Failure::new(EXIT_USAGE, format!("{problem}\n請執行 mmsend -h 查看說明"))
}

pub(crate) struct Failure {
    pub(crate) code: u8,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Prints the outcome where scripts look for it: stdout on success, stderr otherwise.
pub(crate) fn report(result: Result<String, Failure>) -> ExitCode {
    match result {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("{}", failure.message);
            ExitCode::from(failure.code)
        }
    }
}

pub fn run() -> ExitCode {
    let first = std::env::args_os().nth(1);
    match first.as_ref().and_then(|arg| arg.to_str()) {
        Some(WINDOWS_HELP_FLAG) => {
            // Printing help only fails if stdout is gone; there is nobody to tell then.
            let _ = command().print_help();
            return ExitCode::SUCCESS;
        }
        Some(subcommands::LOGIN) => return report(subcommands::login()),
        Some(subcommands::STATUS) => return report(subcommands::status()),
        Some(subcommands::FIND) => return report(subcommands::find()),
        Some(subcommands::ALIAS) => return report(subcommands::alias()),
        Some(subcommands::INSTALL) => return report(subcommands::install()),
        Some(subcommands::UPDATE) => return report(subcommands::update()),
        Some(subcommands::DELETE) => return report(subcommands::delete()),
        Some(subcommands::REPLIES) => return report(subcommands::replies()),
        _ => {}
    }
    let mut record = SendRecord::starting_now();
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let failure = usage_failure(&e);
            eprintln!("{}", failure.message);
            // The arguments themselves are what went wrong, so they are the summary.
            let args: Vec<String> = std::env::args().skip(1).collect();
            record.summary = summarize_text(&args.join(" "));
            record.detail = failure.message;
            log(&record);
            return ExitCode::from(failure.code);
        }
    };
    if args.dry_run {
        // Nothing was sent, so nothing goes in the send log.
        return report(dry_run(&args));
    }
    let code = match send(args, &mut record) {
        Ok((recipient, post_id)) => {
            record.ok = true;
            // One line: `for /f` in a batch file keeps the last line it reads, so a
            // second line would silently replace this one.
            record.detail = format!("已傳送給 {recipient}（訊息 ID：{post_id}）");
            println!("{}", record.detail);
            record.post_id = Some(post_id);
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("{}", failure.message);
            record.detail = failure.message;
            ExitCode::from(failure.code)
        }
    };
    log(&record);
    code
}

/// Everything a send checks, without sending: who it goes to by name, and what.
fn dry_run(args: &Args) -> Result<String, Failure> {
    let target = parse_target(&args.target)?;
    let outgoing = compose(read_message(args)?, args.code, read_attachments(&args.attach)?)?;
    let mut session = Session::load().map_err(|e| session_failure(e, &target))?;
    let recipient = with_retries(|| target.look_up(&mut session)).map_err(|e| session_failure(e, &target))?;
    if let Some(channel_id) = recipient.existing_channel_id() {
        with_retries(|| session.call(|c| c.my_membership(channel_id))).map_err(|e| match e {
            Error::Api(ApiError::NotFound) => session_failure(Error::Api(ApiError::Forbidden), &target),
            e => session_failure(e, &target),
        })?;
    }
    let mut lines = vec![
        "試跑，未傳送".to_owned(),
        format!("收件對象：{}", recipient.label),
    ];
    if !outgoing.message.is_empty() {
        lines.push("內容：".to_owned());
        lines.extend(preview_lines(&outgoing.message));
    }
    if !outgoing.files.is_empty() {
        let names: Vec<&str> = outgoing.files.iter().map(|file| file.name.as_str()).collect();
        lines.push(format!("附件：{}", names.join("、")));
    }
    Ok(lines.join("\n"))
}

/// The post as it will go out, so whoever confirms reads the text itself rather than
/// someone's account of it; past DRY_RUN_LINES, how many more lines there are.
fn preview_lines(message: &str) -> Vec<String> {
    let all: Vec<&str> = message.lines().collect();
    let mut shown: Vec<String> = all.iter().take(DRY_RUN_LINES).map(|line| format!("  {line}")).collect();
    if all.len() > DRY_RUN_LINES {
        shown.push(format!("  …還有 {} 行", all.len() - DRY_RUN_LINES));
    }
    shown
}

fn log(record: &SendRecord) {
    // The log is a convenience: failing to write it must not fail (or clutter the
    // output of) the send itself.
    let _ = store::append_send_record(record);
}

pub(crate) fn summarize_text(text: &str) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty() && !line.starts_with("```"));
    line.unwrap_or_default().chars().take(SUMMARY_CHARS).collect()
}

fn summarize(outgoing: &Outgoing) -> String {
    let text = summarize_text(&outgoing.message);
    if outgoing.files.is_empty() {
        return text;
    }
    let names: Vec<&str> = outgoing.files.iter().map(|file| file.name.as_str()).collect();
    let files = format!("附件：{}", names.join("、"));
    if text.is_empty() { files } else { format!("{text}（{files}）") }
}

/// Returns who it went to, by name, and the new post's id.
fn send(args: Args, record: &mut SendRecord) -> Result<(String, String), Failure> {
    let target = parse_target(&args.target)?;
    record.target = target.to_string();
    let outgoing = compose(read_message(&args)?, args.code, read_attachments(&args.attach)?)?;
    record.summary = summarize(&outgoing);
    let mut session = Session::load().map_err(|e| session_failure(e, &target))?;
    let recipient = with_retries(|| target.look_up(&mut session)).map_err(|e| session_failure(e, &target))?;
    let channel_id =
        with_retries(|| recipient.channel_id(&mut session)).map_err(|e| session_failure(e, &target))?;
    let mut file_ids = Vec::with_capacity(outgoing.files.len());
    for file in &outgoing.files {
        let id = with_retries(|| session.call(|c| c.upload_file(&channel_id, &file.name, &file.bytes)))
            .map_err(|e| session_failure(e, &target))?;
        file_ids.push(id);
    }
    let pending_post_id = session.new_pending_post_id();
    let post = NewPost {
        channel_id: &channel_id,
        message: &outgoing.message,
        pending_post_id: &pending_post_id,
        file_ids: &file_ids,
    };
    let sent =
        with_retries(|| session.call(|c| c.create_post(&post))).map_err(|e| session_failure(e, &target))?;
    Ok((recipient.label, sent.id))
}

struct Attachment {
    name: String,
    bytes: Vec<u8>,
}

/// What actually goes into the post.
struct Outgoing {
    message: String,
    files: Vec<Attachment>,
}

fn read_attachments(paths: &[PathBuf]) -> Result<Vec<Attachment>, Failure> {
    paths
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)
                .map_err(|e| Failure::new(EXIT_USAGE, format!("無法讀取 {}：{e}", path.display())))?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Ok(Attachment { name, bytes })
        })
        .collect()
}

fn compose(text: Option<String>, code: bool, mut files: Vec<Attachment>) -> Result<Outgoing, Failure> {
    // `echo hi | mmsend` delivers "hi \r\n"; Mattermost rejects blank posts.
    let text = text.map(|text| text.trim_end().to_owned()).filter(|text| !text.is_empty());
    let message = match text {
        None if files.is_empty() => return Err(Failure::new(EXIT_USAGE, "訊息內容為空")),
        None => String::new(),
        Some(text) => {
            let formatted = if code { code_block(&text) } else { text.clone() };
            if formatted.chars().count() <= MAX_POST_CHARS {
                formatted
            } else {
                let head = preview(&text);
                let head = if code { code_block(&head) } else { head };
                files.push(Attachment {
                    name: FULL_TEXT_FILE_NAME.to_owned(),
                    bytes: text.into_bytes(),
                });
                format!("{head}\n（內容過長，完整內容請見附件「{FULL_TEXT_FILE_NAME}」）")
            }
        }
    };
    if files.len() > MAX_FILES_PER_POST {
        return Err(Failure::new(
            EXIT_USAGE,
            format!("附加檔案最多 {MAX_FILES_PER_POST} 個（訊息過長時，完整內容也會占用一個）"),
        ));
    }
    Ok(Outgoing { message, files })
}

/// A Markdown code block, so command output keeps its columns. The fence is
/// longer than any run of backticks inside, which would otherwise end it early.
fn code_block(text: &str) -> String {
    const MIN_FENCE: usize = 3;
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest_run + 1).max(MIN_FENCE));
    format!("{fence}\n{text}\n{fence}")
}

fn preview(text: &str) -> String {
    let head = text.lines().take(PREVIEW_LINES).collect::<Vec<_>>().join("\n");
    let head: String = head.chars().take(PREVIEW_MAX_CHARS).collect();
    format!("{head}\n…")
}

pub(crate) fn with_retries<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
    let mut delays = RETRY_DELAYS.iter();
    loop {
        match attempt() {
            Err(Error::Api(e)) if e.is_transient() => {
                let Some(delay) = delays.next() else {
                    return Err(Error::Api(e));
                };
                eprintln!("{e}，{} 秒後重試…", delay.as_secs());
                std::thread::sleep(*delay);
            }
            result => return result,
        }
    }
}

pub(crate) fn parse_target(args: &TargetArgs) -> Result<Target, Failure> {
    if let Some(alias) = &args.to {
        let favorites = favorites::load().map_err(|e| Failure::new(EXIT_OTHER, e))?;
        return favorites::find_alias(&favorites, alias.trim())
            .map(|favorite| favorite.target.clone())
            .ok_or_else(|| {
                Failure::new(
                    EXIT_TARGET_NOT_FOUND,
                    format!("找不到別名「{alias}」：請開啟「{APP_NAME}」確認別名"),
                )
            });
    }
    if let Some(user) = &args.user {
        return Ok(Target::User(user.trim_start_matches('@').to_owned()));
    }
    if let Some(spec) = &args.channel {
        return Target::parse_channel(spec)
            .ok_or_else(|| Failure::new(EXIT_USAGE, "--channel 的格式為「團隊/頻道」，例如 dev/town-square"));
    }
    if let Some(spec) = &args.group {
        let invalid = || Failure::new(EXIT_USAGE, "--group 須列出成員帳號，例如 alice,bob");
        let Some(Target::Group(mut members)) = Target::parse_group(spec) else {
            return Err(invalid());
        };
        // Mattermost adds the sender anyway; listing yourself would hide a group too small.
        if let Ok(Some(config)) = store::load_config() {
            members.retain(|member| !member.eq_ignore_ascii_case(&config.username));
        }
        let target = Target::Group(members);
        if let Target::Group(members) = &target
            && !GROUP_OTHER_MEMBERS.contains(&members.len())
        {
            let problem = match members.as_slice() {
                [] => return Err(invalid()),
                [only] => format!("群組至少要 2 位其他成員；只傳給一個人請用 --user {only}"),
                _ => format!(
                    "群組最多 {} 位其他成員（Mattermost 群組含您最多 {} 人），人數更多請改用頻道",
                    GROUP_OTHER_MEMBERS.end(),
                    GROUP_OTHER_MEMBERS.end() + 1
                ),
            };
            return Err(Failure::new(EXIT_USAGE, problem));
        }
        return Ok(target);
    }
    let id = args.channel_id.clone().expect("clap requires one target argument");
    Ok(Target::ChannelId(id))
}

/// `None`: no text at all, which is fine when files are attached.
fn read_message(args: &Args) -> Result<Option<String>, Failure> {
    if let Some(path) = &args.file {
        let bytes = std::fs::read(path)
            .map_err(|e| Failure::new(EXIT_USAGE, format!("無法讀取 {}：{e}", path.display())))?;
        return Ok(Some(console::decode_file_text(&bytes)));
    }
    match args.message.as_deref() {
        Some(text) if text != STDIN_MARKER => Ok(Some(text.to_owned())),
        _ if !args.attach.is_empty() && std::io::stdin().is_terminal() => Ok(None),
        _ => read_stdin().map(Some),
    }
}

fn read_stdin() -> Result<String, Failure> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(Failure::new(
            EXIT_USAGE,
            "缺少訊息內容：請在指令最後加上 \"訊息\"，或以 --file 指定文字檔",
        ));
    }
    let mut bytes = Vec::new();
    stdin
        .read_to_end(&mut bytes)
        .map_err(|e| Failure::new(EXIT_OTHER, format!("讀取管線失敗：{e}")))?;
    Ok(console::decode_piped_text(&bytes))
}

/// The failures every command that talks to the server shares.
pub(crate) fn login_failure(error: Error) -> Failure {
    match error {
        Error::NotLoggedIn => Failure::new(
            EXIT_NOT_LOGGED_IN,
            format!("尚未登入：請執行 mmsend login，或從開始功能表開啟「{APP_NAME}」登入"),
        ),
        // Reaching the server with a stored password it refuses: it was changed, or
        // the account has since turned on two-step verification.
        Error::Api(ApiError::Unauthorized | ApiError::MfaRequired | ApiError::MfaCodeWrong) => Failure::new(
            EXIT_LOGIN_EXPIRED,
            format!("登入已失效：請執行 mmsend login，或從開始功能表開啟「{APP_NAME}」重新登入"),
        ),
        Error::Api(e) => Failure::new(EXIT_SEND_FAILED, format!("連線失敗，{e}")),
        Error::Store(e) => Failure::new(EXIT_OTHER, e),
    }
}

fn session_failure(error: Error, target: &Target) -> Failure {
    match error {
        Error::NotLoggedIn
        | Error::Api(ApiError::Unauthorized | ApiError::MfaRequired | ApiError::MfaCodeWrong)
        | Error::Store(_) => login_failure(error),
        Error::Api(ApiError::NotFound) => Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("找不到 {target}：請以 mmsend find <姓名> 查詢，或開啟「{APP_NAME}」重新複製指令"),
        ),
        Error::Api(ApiError::Forbidden) => Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("無權限傳送至 {target}：請先加入該頻道"),
        ),
        Error::Api(ApiError::Server { status: 413, .. }) => {
            Failure::new(EXIT_SEND_FAILED, "附加檔案超過伺服器允許的大小")
        }
        Error::Api(ApiError::Server { status: 501, .. }) => {
            Failure::new(EXIT_SEND_FAILED, "伺服器未開放附加檔案")
        }
        Error::Api(e) => Failure::new(EXIT_SEND_FAILED, format!("傳送失敗，{e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_size_mattermost_refuses_is_caught_before_sending() {
        let group = |spec: &str| {
            parse_target(&TargetArgs {
                user: None,
                channel: None,
                group: Some(spec.to_owned()),
                to: None,
                channel_id: None,
            })
        };
        assert!(group("alice").is_err());
        assert!(group("a,b").is_ok());
        assert!(group("a,b,c,d,e,f,g").is_ok());
        assert!(group("a,b,c,d,e,f,g,h").is_err());
    }

    #[test]
    fn dry_run_shows_the_text_itself_up_to_a_limit() {
        let short = "第一行\n第二行\n第三行";
        assert_eq!(preview_lines(short), vec!["  第一行", "  第二行", "  第三行"]);
        let long: String = (1..=13).map(|n| format!("第 {n} 行\n")).collect();
        let shown = preview_lines(&long);
        assert_eq!(shown.len(), DRY_RUN_LINES + 1);
        assert_eq!(shown[DRY_RUN_LINES - 1], "  第 10 行");
        assert_eq!(shown[DRY_RUN_LINES], "  …還有 3 行");
    }

    #[test]
    fn missing_recipient_is_explained() {
        let error = command().try_get_matches_from(["mmsend", "訊息"]).expect_err("a recipient is required");
        assert!(usage_failure(&error).message.starts_with("請指定收件對象"));
    }

    #[test]
    fn code_block_fence_outlasts_backticks_in_the_output() {
        assert_eq!(code_block("a  b"), "```\na  b\n```");
        assert_eq!(code_block("run ```x```"), "````\nrun ```x```\n````");
    }

    #[test]
    fn too_long_message_is_attached_whole_with_a_preview() {
        let text = "第一行\n".repeat(MAX_POST_CHARS);
        let outgoing = compose(Some(text.clone()), true, Vec::new()).unwrap_or_else(|f| panic!("{}", f.message));
        assert_eq!(outgoing.files.len(), 1);
        assert_eq!(outgoing.files[0].bytes, text.trim_end().as_bytes());
        assert!(outgoing.message.starts_with("```\n第一行\n"));
        assert!(outgoing.message.chars().count() < MAX_POST_CHARS);
    }
}
