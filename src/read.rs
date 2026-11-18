//! `mmsend read`: the latest messages with one person, group or channel, both sides,
//! so whoever acts next (often an AI agent) can see what was said before deciding.
//!
//! Only by recipient, the same ones a message can be sent to; never by post id, which
//! would reach any conversation the account can see (see `replies`).

use std::collections::{HashMap, HashSet};

use clap::error::ErrorKind;
use clap::{ArgAction, ArgGroup, Parser};

use crate::api::{POSTS_PER_PAGE, Post};
use crate::cli::{
    EXIT_USAGE, Failure, TargetArgs, login_failure, parse_target, session_failure, summarize_text, usage_failure,
    with_retries,
};
use crate::install::COMMAND_NAME;
use crate::replies::{SYSTEM_POST_PREFIX, in_order, save_attachment};
use crate::session::Session;
use crate::store;
use crate::subcommands::OPTIONS_ONLY_HELP_TEMPLATE;

pub const READ: &str = "read";

const DEFAULT_LIMIT: u16 = 20;
/// One page from the server: more than enough to catch up before answering.
const MAX_LIMIT: usize = POSTS_PER_PAGE;
/// How the user's own messages are shown, here and in a --reply dry run.
pub(crate) const ME_LABEL: &str = "我";

#[derive(Parser)]
#[command(
    name = "mmsend read",
    about = "列出與某人、群組或頻道最近的訊息（雙方的都列，由舊到新），附件下載到暫存資料夾。",
    override_usage = "mmsend read --user <帳號> [--limit <則數>]\n  \
                      mmsend read --to <別名> | --group <帳號,帳號> | --channel <團隊/頻道>",
    help_template = OPTIONS_ONLY_HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true,
    group(ArgGroup::new("recipient").required(true).args(["user", "to", "channel", "group"]))
)]
struct ReadArgs {
    /// 與這個帳號的私訊
    #[arg(long, value_name = "帳號")]
    user: Option<String>,

    /// 與這個別名的對話
    #[arg(long, value_name = "別名")]
    to: Option<String>,

    /// 這個頻道，例如 dev/town-square
    #[arg(long, value_name = "團隊/頻道")]
    channel: Option<String>,

    /// 這個群組，例如 alice,bob
    #[arg(long, value_name = "帳號,帳號")]
    group: Option<String>,

    /// 列出幾則（預設 20，最多 200）
    #[arg(long, value_name = "則數", default_value_t = DEFAULT_LIMIT, hide_default_value = true,
          value_parser = clap::value_parser!(u16).range(1..=MAX_LIMIT as i64))]
    limit: u16,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

pub fn read() -> Result<String, Failure> {
    let args = match ReadArgs::try_parse_from(std::env::args_os().skip(2)) {
        Ok(args) => args,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => return Ok(e.render().to_string()),
        Err(e) => {
            let problem = match e.kind() {
                ErrorKind::MissingRequiredArgument | ErrorKind::ArgumentConflict => {
                    "請指定要讀哪個對話，擇一：--user / --to / --channel / --group".to_owned()
                }
                // --limit is the only argument with a value check.
                ErrorKind::ValueValidation | ErrorKind::InvalidValue => format!("--limit 請指定 1 到 {MAX_LIMIT} 的則數"),
                _ => return Err(usage_failure(&e)),
            };
            return Err(Failure::new(EXIT_USAGE, format!("{problem}\n請執行 mmsend read -h 查看說明")));
        }
    };
    let target = parse_target(&TargetArgs {
        user: args.user,
        channel: args.channel,
        group: args.group,
        to: args.to,
        channel_id: None,
    })?;
    let mut session = Session::load().map_err(login_failure)?;
    let me = session.config.user_id.clone();
    let recipient = with_retries(|| target.look_up(&mut session)).map_err(|e| session_failure(e, &target))?;
    let channel_id = with_retries(|| recipient.channel_id(&mut session)).map_err(|e| session_failure(e, &target))?;
    let list = with_retries(|| session.call(|c| c.channel_posts(&channel_id, None, 0)))
        .map_err(|e| session_failure(e, &target))?;
    // The page also carries the thread starters of its replies, older ones included:
    // kept aside to say what a reply answers.
    let starters: HashMap<String, (String, String)> = list
        .posts
        .values()
        .map(|post| (post.id.clone(), (post.user_id.clone(), summarize_text(&post.message))))
        .collect();
    let posts = latest(in_order(list).collect(), usize::from(args.limit));
    if posts.is_empty() {
        return Ok(format!("與 {} 還沒有任何訊息", recipient.label));
    }
    Ok(render(&mut session, &posts, &starters, &recipient.label, &channel_id, &me))
}

/// The newest `limit` posts people wrote, oldest first.
fn latest(mut posts: Vec<Post>, limit: usize) -> Vec<Post> {
    posts.retain(|post| !post.kind.starts_with(SYSTEM_POST_PREFIX));
    posts.sort_by_key(|post| post.create_at);
    let skip = posts.len().saturating_sub(limit);
    posts.split_off(skip)
}

fn render(
    session: &mut Session,
    posts: &[Post],
    starters: &HashMap<String, (String, String)>,
    with: &str,
    channel_id: &str,
    me: &str,
) -> String {
    let ids: Vec<String> = posts
        .iter()
        .flat_map(|post| {
            let reactors = post.metadata.reactions.iter().map(|r| r.user_id.clone());
            let starter = starters.get(&post.root_id).map(|(user_id, _)| user_id.clone());
            std::iter::once(post.user_id.clone()).chain(reactors).chain(starter)
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    // Without names everything still reads; only who is shown by id.
    let names: HashMap<String, String> = with_retries(|| session.call(|c| c.users_by_ids(&ids)))
        .map(|users| users.into_iter().map(|user| (user.id.clone(), user.label())).collect())
        .unwrap_or_default();
    let name = |id: &str| {
        if id == me { ME_LABEL.to_owned() } else { names.get(id).cloned().unwrap_or_else(|| id.to_owned()) }
    };

    // One folder per conversation, emptied each time, so reading again does not pile up copies.
    let folder = std::env::temp_dir().join(format!("{COMMAND_NAME}-read")).join(channel_id);
    let _ = std::fs::remove_dir_all(&folder);
    let mut taken = HashSet::new();

    let mut blocks = vec![format!("與 {with} 最近 {} 則訊息（由舊到新）：", posts.len())];
    for post in posts {
        // Indented, so a line someone wrote cannot pass for one of these headers.
        // The id is what `mmsend --reply` takes to answer this message.
        let mut notes = vec![format!("訊息 ID：{}", post.id)];
        if !post.root_id.is_empty() {
            notes.push(match starters.get(&post.root_id) {
                Some((starter, summary)) if !summary.is_empty() => format!("回覆 {} 的「{summary}」", name(starter)),
                Some((starter, _)) => format!("回覆 {} 的訊息", name(starter)),
                None => "討論串回覆".to_owned(),
            });
        }
        let header = format!("{} {}（{}）", store::local_time(post.create_at), name(&post.user_id), notes.join("；"));
        let mut lines = vec![header];
        lines.extend(post.message.lines().map(|line| format!("  {line}")));
        for file in &post.metadata.files {
            lines.push(match save_attachment(session, &folder, file, &mut taken) {
                Ok(path) => format!("  附件：{}", path.display()),
                Err(problem) => format!("  附件「{}」下載失敗：{problem}", file.name),
            });
        }
        let reactions: Vec<String> = post
            .metadata
            .reactions
            .iter()
            .map(|reaction| format!("{} :{}:", name(&reaction.user_id), reaction.emoji_name))
            .collect();
        if !reactions.is_empty() {
            lines.push(format!("  表情回應：{}", reactions.join("、")));
        }
        blocks.push(lines.join("\n"));
    }
    blocks.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_messages_people_wrote_come_oldest_first() {
        let post = |id: &str, create_at: i64, kind: &str| Post {
            id: id.into(),
            create_at,
            kind: kind.into(),
            ..Default::default()
        };
        // The server sends newest first.
        let posts = vec![
            post("d", 40, ""),
            post("joined", 30, "system_join_channel"),
            post("c", 20, ""),
            post("b", 10, ""),
            post("a", 5, ""),
        ];
        let ids: Vec<String> = latest(posts, 3).into_iter().map(|post| post.id).collect();
        assert_eq!(ids, ["b", "c", "d"]);
    }
}
