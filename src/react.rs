//! `mmsend react`: an emoji on someone's message (👍, 👌), for when that is the whole
//! answer and a new message would be noise. The reverse of what `replies` reports.
//!
//! The conversation is always named, as for a send; a message id alone could reach
//! any conversation the account can see (see `cli::post_in_conversation`).

use clap::error::{ContextKind, ErrorKind};
use clap::{ArgAction, ArgGroup, Parser};

use crate::api::{ApiError, Post};
use crate::cli::{
    EXIT_TARGET_NOT_FOUND, EXIT_USAGE, Failure, TargetArgs, describe_post, login_failure, parse_target,
    post_in_conversation, session_failure, usage_failure, with_retries,
};
use crate::replies::{SYSTEM_POST_PREFIX, in_order};
use crate::session::{Error, Session};
use crate::subcommands::HELP_TEMPLATE;

pub const REACT: &str = "react";

/// What Mattermost puts around an emoji name in text: `:ok_hand:`.
const EMOJI_DELIMITER: char = ':';
/// Characters of Mattermost emoji names, built-in and custom alike.
const EMOJI_NAME_PUNCTUATION: [char; 3] = ['_', '+', '-'];
/// Also how a missing emoji is told apart from a missing conversation in clap's errors.
const EMOJI_VALUE_NAME: &str = "表情";

#[derive(Parser)]
#[command(
    name = "mmsend react",
    about = "對訊息按表情回應（或用 --remove 移除），適合只需要回「收到」「讚」的時候。\n\
             沒指定 --id 時，按在對方最後傳的那則。",
    override_usage = "mmsend react --user <帳號> <表情> [--id <訊息ID>] [--dry-run] [--remove]\n  \
                      （或 --to、--channel、--group 指定對話）",
    help_template = HELP_TEMPLATE,
    no_binary_name = true,
    disable_help_flag = true,
    group(ArgGroup::new("conversation").required(true).args(["user", "to", "channel", "group"]))
)]
struct ReactArgs {
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

    /// 表情名稱，例如 +1、ok_hand、white_check_mark（:ok_hand: 也可以）
    #[arg(value_name = EMOJI_VALUE_NAME)]
    emoji: String,

    /// 要按的那則訊息（mmsend read 列出的 ID）；省略時是對方最後傳的那則
    #[arg(long, value_name = "訊息ID")]
    id: Option<String>,

    /// 試跑：顯示會按在哪則訊息上，不按
    #[arg(long)]
    dry_run: bool,

    /// 移除自己按過的這個表情
    #[arg(long)]
    remove: bool,

    /// 顯示說明
    #[arg(short = 'h', long, action = ArgAction::Help)]
    help: Option<bool>,
}

pub fn react() -> Result<String, Failure> {
    let args = match ReactArgs::try_parse_from(std::env::args_os().skip(2)) {
        Ok(args) => args,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => return Ok(e.render().to_string()),
        Err(e) if matches!(e.kind(), ErrorKind::MissingRequiredArgument | ErrorKind::ArgumentConflict)
            && !e.get(ContextKind::InvalidArg).is_some_and(|arg| arg.to_string().contains(EMOJI_VALUE_NAME)) =>
        {
            return Err(Failure::new(
                EXIT_USAGE,
                "請指定是哪個對話，擇一：--user / --to / --channel / --group\n請執行 mmsend react -h 查看說明",
            ));
        }
        Err(e) => return Err(usage_failure(&e)),
    };
    let emoji = emoji_name(&args.emoji)?;
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
    let post = match &args.id {
        Some(id) => post_in_conversation(&mut session, id, &channel_id, &recipient.label)?,
        None => their_last_post(&mut session, &channel_id, &me)
            .map_err(|e| session_failure(e, &target))?
            .ok_or_else(|| Failure::new(EXIT_TARGET_NOT_FOUND, format!("與 {} 的對話裡沒有對方傳的訊息", recipient.label)))?,
    };
    let which = describe_post(&mut session, &post);
    if args.dry_run {
        let action = if args.remove { "移除" } else { "按" };
        // The id, so the real run can name this message: by then a newer one may have come.
        return Ok(format!("試跑，未{action}\n將{action} :{emoji}:：{which}\n訊息 ID：{}", post.id));
    }
    if args.remove {
        with_retries(|| session.call(|c| c.remove_reaction(&me, &post.id, &emoji))).map_err(login_failure)?;
        Ok(format!("已移除 :{emoji}:：{which}"))
    } else {
        with_retries(|| session.call(|c| c.add_reaction(&me, &post.id, &emoji))).map_err(|e| match e {
            // What the server answers for an emoji name it does not have.
            Error::Api(ApiError::NotFound) => {
                Failure::new(EXIT_USAGE, format!("伺服器沒有表情「{emoji}」：請確認名稱，例如 +1、ok_hand"))
            }
            e => login_failure(e),
        })?;
        Ok(format!("已按 :{emoji}:：{which}"))
    }
}

/// `:ok_hand:` or `ok_hand` → `ok_hand`. An emoji character (👍) is refused with
/// examples: Mattermost takes names only.
fn emoji_name(text: &str) -> Result<String, Failure> {
    let name = text.trim().trim_matches(EMOJI_DELIMITER).to_lowercase();
    let valid = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || EMOJI_NAME_PUNCTUATION.contains(&c));
    if !valid {
        return Err(Failure::new(
            EXIT_USAGE,
            format!("「{}」不是表情名稱：請用英文名稱，例如 +1（👍）、ok_hand（👌）、white_check_mark（✅）", text.trim()),
        ));
    }
    Ok(name)
}

/// The newest message someone else wrote, from the latest page.
fn their_last_post(session: &mut Session, channel_id: &str, me: &str) -> Result<Option<Post>, Error> {
    let list = with_retries(|| session.call(|c| c.channel_posts(channel_id, None, 0)))?;
    Ok(in_order(list)
        .filter(|post| post.user_id != me && !post.kind.starts_with(SYSTEM_POST_PREFIX))
        .max_by_key(|post| post.create_at))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_names_are_taken_with_or_without_colons_and_emoji_characters_are_refused() {
        assert_eq!(emoji_name(":ok_hand:").ok(), Some("ok_hand".to_owned()));
        assert_eq!(emoji_name("+1").ok(), Some("+1".to_owned()));
        assert_eq!(emoji_name(" White_Check_Mark ").ok(), Some("white_check_mark".to_owned()));
        assert!(emoji_name("👍").is_err());
        assert!(emoji_name("::").is_err());
        assert!(emoji_name("../x").is_err());
    }
}
