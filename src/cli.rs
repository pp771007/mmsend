//! `mmsend --user alice "message"` — the scripted side of the exe.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, ArgGroup, CommandFactory, FromArgMatches, Parser};

use crate::APP_NAME;
use crate::api::{ApiError, NewPost};
use crate::console;
use crate::favorites;
use crate::gui;
use crate::session::{Error, Session};
use crate::store::{self, SendRecord};
use crate::target::Target;

/// Exit codes scripts can branch on. 2 matches what clap uses for bad arguments.
const EXIT_OTHER: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_NOT_LOGGED_IN: u8 = 3;
const EXIT_LOGIN_EXPIRED: u8 = 4;
const EXIT_TARGET_NOT_FOUND: u8 = 5;
const EXIT_SEND_FAILED: u8 = 6;

const EXIT_CODES: [(u8, &str); 7] = [
    (0, "已傳送"),
    (EXIT_OTHER, "其他錯誤"),
    (EXIT_USAGE, "參數錯誤或訊息為空"),
    (EXIT_NOT_LOGGED_IN, "尚未登入"),
    (EXIT_LOGIN_EXPIRED, "登入已失效（密碼已變更）"),
    (EXIT_TARGET_NOT_FOUND, "找不到收件對象，或無權限"),
    (EXIT_SEND_FAILED, "連線或伺服器錯誤"),
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

const STDIN_MARKER: &str = "-";
/// What cmd users type out of habit; it would otherwise be sent as a message.
const WINDOWS_HELP_FLAG: &str = "/?";
/// `mmsend login`. Every send names a recipient first, so a bare first argument
/// can never have been meant as a message.
const LOGIN_COMMAND: &str = "login";

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
  mmsend login                          開啟登入視窗，登入後自動關閉
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

#[derive(Parser)]
#[command(
    name = "mmsend",
    version,
    about = "從命令列傳送 Mattermost 訊息。",
    help_template = HELP_TEMPLATE,
    disable_help_flag = true,
    disable_version_flag = true
)]
#[command(group(ArgGroup::new("target").required(true).args(["user", "channel", "group", "to", "channel_id"])))]
struct Args {
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

    /// 從文字檔讀取訊息
    #[arg(long, value_name = "檔案", conflicts_with = "message")]
    file: Option<PathBuf>,

    /// 附加檔案，可重複指定（最多 10 個）
    #[arg(long, value_name = "檔案")]
    attach: Vec<PathBuf>,

    /// 以程式碼區塊傳送，保留指令輸出的對齊
    #[arg(long)]
    code: bool,

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

fn command() -> clap::Command {
    let exit_codes: String = EXIT_CODES
        .iter()
        .map(|(code, meaning)| format!("\n  {code}  {meaning}"))
        .collect();
    Args::command().after_help(format!(
        "{EXAMPLES}\n\n結束代碼（批次檔可用 if errorlevel 判斷）：{exit_codes}"
    ))
}

fn parse_args() -> Result<Args, clap::Error> {
    let matches = command().try_get_matches()?;
    Args::from_arg_matches(&matches)
}

/// clap's own messages are English; these cover what people actually get wrong.
fn usage_failure(error: &clap::Error) -> Failure {
    let arg = error
        .get(ContextKind::InvalidArg)
        .map(|value| value.to_string())
        .unwrap_or_default();
    let problem = match error.kind() {
        ErrorKind::MissingRequiredArgument => "請指定收件對象：--user、--channel、--group 或 --to".to_owned(),
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

struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn run() -> ExitCode {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == WINDOWS_HELP_FLAG) {
        // Printing help only fails if stdout is gone; there is nobody to tell then.
        let _ = command().print_help();
        return ExitCode::SUCCESS;
    }
    if std::env::args_os().nth(1).is_some_and(|arg| arg == LOGIN_COMMAND) {
        return login();
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
    let code = match send(args, &mut record) {
        Ok(target) => {
            record.ok = true;
            record.detail = format!("已傳送給 {target}");
            println!("{}", record.detail);
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

/// Waits while the person logs in; the caller only ever sees the outcome, never the password.
fn login() -> ExitCode {
    let failure = match gui::run_login() {
        Ok(true) => match store::load_config() {
            Ok(Some(config)) => {
                println!("已登入 @{}（{}）", config.username, config.server_url.trim_end_matches('/'));
                return ExitCode::SUCCESS;
            }
            Ok(None) => Failure::new(EXIT_OTHER, "登入資料未能儲存"),
            Err(e) => Failure::new(EXIT_OTHER, e),
        },
        Ok(false) => Failure::new(EXIT_NOT_LOGGED_IN, "登入視窗已關閉，尚未登入"),
        Err(e) => Failure::new(EXIT_OTHER, e),
    };
    eprintln!("{}", failure.message);
    ExitCode::from(failure.code)
}

fn log(record: &SendRecord) {
    // The log is a convenience: failing to write it must not fail (or clutter the
    // output of) the send itself.
    let _ = store::append_send_record(record);
}

fn summarize_text(text: &str) -> String {
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

fn send(args: Args, record: &mut SendRecord) -> Result<Target, Failure> {
    let target = parse_target(&args)?;
    record.target = target.to_string();
    let outgoing = compose(read_message(&args)?, args.code, read_attachments(&args.attach)?)?;
    record.summary = summarize(&outgoing);
    let mut session = Session::load().map_err(|e| session_failure(e, &target))?;
    let channel_id = with_retries(|| target.resolve_channel_id(&mut session))
        .map_err(|e| session_failure(e, &target))?;
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
    with_retries(|| session.call(|c| c.create_post(&post))).map_err(|e| session_failure(e, &target))?;
    Ok(target)
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

fn with_retries<T>(mut attempt: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
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

fn parse_target(args: &Args) -> Result<Target, Failure> {
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
        return Target::parse_group(spec)
            .ok_or_else(|| Failure::new(EXIT_USAGE, "--group 須列出成員帳號，例如 alice,bob"));
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

fn session_failure(error: Error, target: &Target) -> Failure {
    match error {
        Error::NotLoggedIn => Failure::new(
            EXIT_NOT_LOGGED_IN,
            format!("尚未登入：請從開始功能表開啟「{APP_NAME}」並登入"),
        ),
        Error::Api(ApiError::Unauthorized) => Failure::new(
            EXIT_LOGIN_EXPIRED,
            format!("登入已失效：請從開始功能表開啟「{APP_NAME}」重新登入"),
        ),
        Error::Api(ApiError::NotFound) => Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("找不到 {target}：請開啟「{APP_NAME}」重新複製指令"),
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
        Error::Store(e) => Failure::new(EXIT_OTHER, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
