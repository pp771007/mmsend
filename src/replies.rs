//! `mmsend replies`: what others answered to a message you sent, so whoever sent
//! a question (often an AI agent) can carry on with the answer.
//!
//! Only ever about the user's own messages: given any post id, it would read any
//! conversation the account can see, which is more than "what did they answer".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::error::ErrorKind;
use clap::{ArgAction, ArgGroup, Parser};

use crate::api::{self, ApiError, Channel, FileInfo, POSTS_PER_PAGE, Post, PostList, Reaction};
use crate::cli::{
    EXIT_NO_REPLIES, EXIT_TARGET_NOT_FOUND, EXIT_USAGE, Failure, TargetArgs, login_failure, parse_target,
    session_failure, summarize_text, usage_failure, with_retries,
};
use crate::install::COMMAND_NAME;
use crate::session::{Error, Session};
use crate::store;
use crate::subcommands::{HELP_TEMPLATE, last_sent};

pub const REPLIES: &str = "replies";

/// How far the scan goes, after the message or back to the user's last one; an old
/// message in a busy conversation would otherwise page through its whole history.
const MAX_SCANNED_POSTS: usize = 1000;
/// Mattermost ids are 26 letters and digits.
const POST_ID_LENGTH: usize = 26;
/// Join-and-leave notices and the like, which nobody wrote.
const SYSTEM_POST_PREFIX: &str = "system_";
/// How often --wait asks again: an answer is noticed within half a minute, and a
/// day of waiting is under 3000 light requests.
const WAIT_INTERVAL: Duration = Duration::from_secs(30);
const MAX_WAIT_MINUTES: u64 = 24 * 60;
const SECONDS_PER_MINUTE: u64 = 60;

#[derive(Parser)]
#[command(
    name = "mmsend replies",
    about = "列出別人對您傳的訊息的回覆與表情回應，附件下載到暫存資料夾。\n\
             私訊與群組列出之後的所有訊息；頻道只列出那則訊息的討論串。還沒有回覆時結束代碼為 7。",
    override_usage = "mmsend replies <訊息ID> [--wait <分鐘>]\n  \
                      mmsend replies --last\n  \
                      mmsend replies --user <帳號>      （或 --to、--channel、--group：您最後傳的那則）",
    help_template = HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true,
    group(ArgGroup::new("message").required(true).args(["post_id", "last", "user", "to", "channel", "group"]))
)]
struct RepliesArgs {
    /// 傳送成功時顯示的訊息 ID
    #[arg(value_name = "訊息ID")]
    post_id: Option<String>,

    /// 查這台電腦最近一次以 mmsend 傳送的訊息
    #[arg(long)]
    last: bool,

    /// 查您最後一則傳給這個帳號的私訊
    #[arg(long, value_name = "帳號")]
    user: Option<String>,

    /// 查您最後一則傳給這個別名的訊息
    #[arg(long, value_name = "別名")]
    to: Option<String>,

    /// 查您在這個頻道的最後一則訊息，例如 dev/town-square
    #[arg(long, value_name = "團隊/頻道")]
    channel: Option<String>,

    /// 查您在這個群組的最後一則訊息，例如 alice,bob
    #[arg(long, value_name = "帳號,帳號")]
    group: Option<String>,

    /// 還沒有回覆時最多等幾分鐘，一有回覆就結束（適合在背景執行）
    #[arg(long, value_name = "分鐘", value_parser = clap::value_parser!(u64).range(1..=MAX_WAIT_MINUTES))]
    wait: Option<u64>,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

/// What came back to one message.
struct Answers {
    mine: Post,
    /// Others' reactions on the message itself: a 👍 is often the whole answer.
    reactions: Vec<Reaction>,
    replies: Vec<Post>,
    /// More posts followed than were scanned.
    more: bool,
}

impl Answers {
    fn is_empty(&self) -> bool {
        self.reactions.is_empty() && self.replies.is_empty()
    }
}

pub fn replies() -> Result<String, Failure> {
    // The shared wording is about recipients and message text, which would mislead here.
    let args = match RepliesArgs::try_parse_from(std::env::args_os().skip(2)) {
        Ok(args) => args,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => return Ok(e.render().to_string()),
        Err(e) => {
            let problem = match e.kind() {
                ErrorKind::MissingRequiredArgument | ErrorKind::ArgumentConflict => {
                    "請指定要查哪一則訊息，擇一：訊息 ID、--last，或 --user / --to / --channel / --group".to_owned()
                }
                // --wait is the only argument with a value check.
                ErrorKind::ValueValidation => format!("--wait 請指定 1 到 {MAX_WAIT_MINUTES} 的分鐘數"),
                _ => return Err(usage_failure(&e)),
            };
            return Err(Failure::new(EXIT_USAGE, format!("{problem}\n請執行 mmsend replies -h 查看說明")));
        }
    };
    let mut session = Session::load().map_err(login_failure)?;
    let me = session.config.user_id.clone();
    let mine = own_message(&args, &mut session, &me)?;
    let channel = with_retries(|| session.call(|c| c.channel(&mine.channel_id))).map_err(login_failure)?;
    let deadline = args
        .wait
        .map(|minutes| Instant::now() + Duration::from_secs(minutes * SECONDS_PER_MINUTE));
    loop {
        let answers = gather(&mut session, &mine.id, &channel, &me)?;
        if !answers.is_empty() {
            return Ok(render(&mut session, &answers, &channel, &me));
        }
        match deadline {
            Some(deadline) if Instant::now() < deadline => {
                std::thread::sleep(WAIT_INTERVAL.min(deadline - Instant::now()));
            }
            _ => {
                let problem = match args.wait {
                    Some(minutes) => format!("等了 {minutes} 分鐘，還沒有回覆"),
                    None => "還沒有回覆".to_owned(),
                };
                return Err(Failure::new(EXIT_NO_REPLIES, problem));
            }
        }
    }
}

/// The message asked about: by id, the newest send from this PC, or the user's last
/// post to a recipient (which may have been sent from the phone).
fn own_message(args: &RepliesArgs, session: &mut Session, me: &str) -> Result<Post, Failure> {
    let target_args = TargetArgs {
        user: args.user.clone(),
        channel: args.channel.clone(),
        group: args.group.clone(),
        to: args.to.clone(),
        channel_id: None,
    };
    if [&target_args.user, &target_args.channel, &target_args.group, &target_args.to]
        .iter()
        .any(|arg| arg.is_some())
    {
        let target = parse_target(&target_args)?;
        let recipient = with_retries(|| target.look_up(session)).map_err(|e| session_failure(e, &target))?;
        let channel_id =
            with_retries(|| recipient.channel_id(session)).map_err(|e| session_failure(e, &target))?;
        return last_own_post(session, &channel_id, me)
            .map_err(|e| session_failure(e, &target))?
            .ok_or_else(|| {
                Failure::new(
                    EXIT_TARGET_NOT_FOUND,
                    format!("最近 {MAX_SCANNED_POSTS} 則訊息裡沒有您傳給 {} 的訊息", recipient.label),
                )
            });
    }
    let post_id = match &args.post_id {
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
    let post = fetch_post(session, &post_id)?;
    if post.user_id != me {
        return Err(Failure::new(
            EXIT_TARGET_NOT_FOUND,
            format!("訊息 {post_id} 不是您傳的：只能查自己傳的訊息的回覆"),
        ));
    }
    Ok(post)
}

fn fetch_post(session: &mut Session, post_id: &str) -> Result<Post, Failure> {
    with_retries(|| session.call(|c| c.post(post_id))).map_err(|e| match e {
        Error::Api(ApiError::NotFound) => {
            Failure::new(EXIT_TARGET_NOT_FOUND, format!("找不到訊息 {post_id}：可能已經刪除"))
        }
        e => login_failure(e),
    })
}

/// The user's newest post in the channel, looking back MAX_SCANNED_POSTS at most.
fn last_own_post(session: &mut Session, channel_id: &str, me: &str) -> Result<Option<Post>, Error> {
    for page in 0..(MAX_SCANNED_POSTS / POSTS_PER_PAGE) as u32 {
        let list = with_retries(|| session.call(|c| c.channel_posts(channel_id, None, page)))?;
        let full = list.order.len() >= POSTS_PER_PAGE;
        // Pages run newest to oldest, so the first page with one of the user's posts has the newest.
        let newest = in_order(list)
            .filter(|post| post.user_id == me && !post.kind.starts_with(SYSTEM_POST_PREFIX))
            .max_by_key(|post| post.create_at);
        if newest.is_some() || !full {
            return Ok(newest);
        }
    }
    Ok(None)
}

/// Asks again from scratch: --wait calls this every WAIT_INTERVAL.
fn gather(session: &mut Session, mine_id: &str, channel: &Channel, me: &str) -> Result<Answers, Failure> {
    // Fetched again for its reactions.
    let mine = fetch_post(session, mine_id)?;
    // In a channel, what follows a message is mostly other conversations; an answer
    // is what was written in its thread.
    let (lists, more) = if matches!(channel.kind.as_str(), api::CHANNEL_DIRECT | api::CHANNEL_GROUP) {
        posts_after(session, &mine)?
    } else {
        let root = if mine.root_id.is_empty() { &mine.id } else { &mine.root_id };
        (vec![with_retries(|| session.call(|c| c.thread(root))).map_err(login_failure)?], false)
    };
    let replies = replies_in(lists, me, mine.create_at);
    let reactions = mine.metadata.reactions.iter().filter(|r| r.user_id != me).cloned().collect();
    Ok(Answers { mine, reactions, replies, more })
}

/// The pages after the message, and whether there are more than MAX_SCANNED_POSTS.
fn posts_after(session: &mut Session, mine: &Post) -> Result<(Vec<PostList>, bool), Failure> {
    let mut lists = Vec::new();
    for page in 0..(MAX_SCANNED_POSTS / POSTS_PER_PAGE) as u32 {
        let list = with_retries(|| session.call(|c| c.channel_posts(&mine.channel_id, Some(&mine.id), page)))
            .map_err(login_failure)?;
        let full = list.order.len() >= POSTS_PER_PAGE;
        lists.push(list);
        if !full {
            return Ok((lists, false));
        }
    }
    Ok((lists, true))
}

/// A page's posts in its `order`. Its `posts` also carries the thread starters of
/// its replies, which can be older than the page, so only `order` counts.
fn in_order(mut list: PostList) -> impl Iterator<Item = Post> {
    let order = std::mem::take(&mut list.order);
    order.into_iter().filter_map(move |id| list.posts.remove(&id))
}

/// What others wrote after `since` (the message's time), oldest first.
fn replies_in(lists: Vec<PostList>, me: &str, since: i64) -> Vec<Post> {
    let mut replies: Vec<Post> = lists
        .into_iter()
        .flat_map(in_order)
        .filter(|post| post.user_id != me && post.create_at > since && !post.kind.starts_with(SYSTEM_POST_PREFIX))
        .collect();
    replies.sort_by_key(|post| post.create_at);
    replies
}

fn render(session: &mut Session, answers: &Answers, channel: &Channel, me: &str) -> String {
    // A direct channel is named `<user id>__<user id>`.
    let other = (channel.kind == api::CHANNEL_DIRECT)
        .then(|| channel.name.split("__").find(|id| *id != me).unwrap_or(me).to_owned());
    let ids: Vec<String> = answers
        .replies
        .iter()
        .map(|reply| reply.user_id.clone())
        .chain(answers.reactions.iter().map(|reaction| reaction.user_id.clone()))
        .chain(other.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    // Without names everything still reads; only who is shown by id.
    let names: HashMap<String, String> = with_retries(|| session.call(|c| c.users_by_ids(&ids)))
        .map(|users| users.into_iter().map(|user| (user.id.clone(), user.label())).collect())
        .unwrap_or_default();
    let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());

    let to = match (&other, channel.kind.as_str()) {
        (Some(other), _) => name(other),
        (None, api::CHANNEL_GROUP) => format!("群組 {}", channel.display_name),
        (None, _) => channel.display_name.clone(),
    };
    let mine = &answers.mine;
    let summary = summarize_text(&mine.message);
    let what = if summary.is_empty() {
        let files: Vec<&str> = mine.metadata.files.iter().map(|file| file.name.as_str()).collect();
        format!("附件 {}", files.join("、"))
    } else {
        format!("「{summary}」")
    };
    // The question comes first: whoever reads this later may not remember it.
    let mut blocks = vec![format!("您的訊息（{} 傳給 {to}）：{what}", store::local_time(mine.create_at))];

    if !answers.reactions.is_empty() {
        let mut by_person: Vec<(String, Vec<String>)> = Vec::new();
        for reaction in &answers.reactions {
            let emoji = format!(":{}:", reaction.emoji_name);
            match by_person.iter_mut().find(|(id, _)| *id == reaction.user_id) {
                Some((_, emojis)) => emojis.push(emoji),
                None => by_person.push((reaction.user_id.clone(), vec![emoji])),
            }
        }
        let lines: Vec<String> = by_person
            .iter()
            .map(|(id, emojis)| format!("  {} {}", name(id), emojis.join(" ")))
            .collect();
        blocks.push(format!("表情回應：\n{}", lines.join("\n")));
    }

    if !answers.replies.is_empty() {
        blocks.push(format!("{} 則回覆：", answers.replies.len()));
    }
    let folder = std::env::temp_dir().join(format!("{COMMAND_NAME}-replies")).join(&mine.id);
    let mut taken = HashSet::new();
    for reply in &answers.replies {
        // Indented, so a line someone wrote cannot pass for one of these headers.
        let mut lines = vec![format!("{} {}", store::local_time(reply.create_at), name(&reply.user_id))];
        lines.extend(reply.message.lines().map(|line| format!("  {line}")));
        for file in &reply.metadata.files {
            lines.push(match save_attachment(session, &folder, file, &mut taken) {
                Ok(path) => format!("  附件：{}", path.display()),
                Err(problem) => format!("  附件「{}」下載失敗：{problem}", file.name),
            });
        }
        blocks.push(lines.join("\n"));
    }
    if answers.more {
        blocks.push(format!("（只查了之後的 {MAX_SCANNED_POSTS} 則訊息，更新的請在 Mattermost 查看）"));
    }
    blocks.join("\n\n")
}

/// Saves an attachment under `folder`; two with the same name keep both.
fn save_attachment(
    session: &mut Session,
    folder: &Path,
    file: &FileInfo,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            order: vec!["late".into(), "mine".into(), "joined".into(), "early".into(), "before".into()],
            posts: [
                post("late", "howard", 30, ""),
                post("mine", "me", 25, ""),
                post("joined", "howard", 20, "system_join_channel"),
                post("early", "howard", 10, ""),
                // Earlier in the thread than the message asked about.
                post("before", "howard", 3, ""),
                // The thread starter of a reply, outside the page.
                post("older-root", "howard", 1, ""),
            ]
            .into_iter()
            .map(|post| (post.id.clone(), post))
            .collect(),
        };
        let ids: Vec<String> = replies_in(vec![list], "me", 5).into_iter().map(|post| post.id).collect();
        assert_eq!(ids, ["early", "late"]);
    }

    #[test]
    fn an_attachment_name_cannot_leave_the_folder() {
        assert_eq!(safe_file_name("IMG_0001.PNG", "f1"), "IMG_0001.PNG");
        assert_eq!(safe_file_name(r"..\..\evil.bat", "f1"), ".._.._evil.bat");
        assert_eq!(safe_file_name("C:/x.txt", "f1"), "C__x.txt");
        assert_eq!(safe_file_name("..", "f1"), "f1");
    }
}
