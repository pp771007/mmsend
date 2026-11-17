//! Who a message goes to, and the command line that addresses them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::api::{self, ApiError};
use crate::session::{Error, Session};

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Target {
    /// Direct message, by Mattermost username.
    User(String),
    /// Public or private channel, by team and channel URL names
    /// (the slugs in `https://host/<team>/channels/<channel>`).
    Channel { team: String, channel: String },
    /// Group message, by the other members' usernames (sorted). Mattermost finds the
    /// group from its member list, so this survives without the opaque channel id.
    Group(Vec<String>),
    /// Any channel by its raw id; kept for scripts written against older versions.
    ChannelId(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TemplateKind {
    Inline,
    File,
    Pipe,
    Attach,
}

impl TemplateKind {
    /// The message is written into the command, inside quotes.
    pub fn quotes_message(self) -> bool {
        matches!(self, TemplateKind::Inline | TemplateKind::Attach)
    }
}

/// Mattermost group messages hold 3–8 people, the sender included. Checked before
/// sending because a dry run never opens the group, so the server cannot refuse it there.
pub const GROUP_OTHER_MEMBERS: std::ops::RangeInclusive<usize> = 2..=7;

const MESSAGE_PLACEHOLDER: &str = "訊息內容";
const FILE_PLACEHOLDER: &str = "訊息.txt";
const PIPE_PLACEHOLDER: &str = "您的指令";
const ATTACH_PLACEHOLDER: &str = "檔案路徑";

impl Target {
    /// Parses the `--channel team/channel` form.
    pub fn parse_channel(spec: &str) -> Option<Target> {
        let (team, channel) = spec.split_once('/')?;
        let (team, channel) = (team.trim(), channel.trim());
        if team.is_empty() || channel.is_empty() || channel.contains('/') {
            return None;
        }
        Some(Target::Channel {
            team: team.to_owned(),
            channel: channel.to_owned(),
        })
    }

    /// Parses the `--group alice,bob` form.
    pub fn parse_group(spec: &str) -> Option<Target> {
        let mut members: Vec<String> = spec
            .split(',')
            .map(|name| name.trim().trim_start_matches('@').to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        members.sort();
        members.dedup();
        (!members.is_empty()).then_some(Target::Group(members))
    }

    /// Mattermost usernames and URL names cannot contain spaces or shell
    /// metacharacters, so the arguments never need quoting.
    pub fn cli_args(&self) -> String {
        match self {
            Target::User(username) => format!("--user {username}"),
            Target::Channel { team, channel } => format!("--channel {team}/{channel}"),
            Target::Group(members) => format!("--group {}", members.join(",")),
            Target::ChannelId(id) => format!("--channel-id {id}"),
        }
    }

    /// `--to 值班` instead of the target's own arguments, for a pinned target with an alias.
    pub fn alias_args(alias: &str) -> String {
        format!("--to {alias}")
    }


    /// Who this is, read-only: a dry run must not open a direct or group channel.
    pub fn look_up(&self, session: &mut Session) -> Result<Recipient, Error> {
        match self {
            Target::User(username) => {
                let other = session.call(|c| c.user_by_username(username))?;
                Ok(Recipient {
                    label: other.label(),
                    name: other.full_name(),
                    private: false,
                    place: Place::Direct(other.id),
                })
            }
            Target::Channel { team, channel } => {
                let found = session.call(|c| c.channel_by_name(team, channel))?;
                Ok(Recipient {
                    label: format!("{self} {}", found.display_name),
                    name: found.display_name,
                    private: found.kind == api::CHANNEL_PRIVATE,
                    place: Place::Channel(found.id),
                })
            }
            Target::Group(members) => {
                let users = session.call(|c| c.users_by_usernames(members))?;
                if users.len() != members.len() {
                    return Err(Error::Api(ApiError::NotFound));
                }
                Ok(Recipient {
                    label: format!("群組 {}", users.iter().map(|u| u.label()).collect::<Vec<_>>().join("、")),
                    name: users.iter().map(|u| u.full_name()).collect::<Vec<_>>().join("、"),
                    private: false,
                    place: Place::Group(users.into_iter().map(|u| u.id).collect()),
                })
            }
            Target::ChannelId(id) => Ok(Recipient {
                label: self.to_string(),
                name: String::new(),
                private: false,
                place: Place::Channel(id.clone()),
            }),
        }
    }
}

/// A target found on the server.
pub struct Recipient {
    /// `@alice 王小明`, `dev/town-square 公眾大廳`: what a person checks before sending.
    pub label: String,
    /// The person's or channel's name, empty when it has none.
    pub name: String,
    pub private: bool,
    place: Place,
}

enum Place {
    Channel(String),
    /// The other person's user id.
    Direct(String),
    /// The other members' user ids.
    Group(Vec<String>),
}

impl Recipient {
    /// The channel to post in, opening the direct or group channel on first use.
    pub fn channel_id(&self, session: &mut Session) -> Result<String, Error> {
        let me = session.config.user_id.clone();
        match &self.place {
            Place::Channel(id) => Ok(id.clone()),
            Place::Direct(other) => Ok(session.call(|c| c.direct_channel(&me, other))?.id),
            Place::Group(others) => {
                let mut ids = others.clone();
                ids.push(me);
                Ok(session.call(|c| c.group_channel(&ids))?.id)
            }
        }
    }

    /// The person's user id, for a direct message (the window shows their picture).
    pub fn user_id(&self) -> Option<&str> {
        match &self.place {
            Place::Direct(id) => Some(id),
            Place::Channel(_) | Place::Group(_) => None,
        }
    }

    /// A channel can be found yet refuse the post (a public one not joined); direct
    /// and group channels are opened for the sender, so they cannot.
    pub fn existing_channel_id(&self) -> Option<&str> {
        match &self.place {
            Place::Channel(id) => Some(id),
            Place::Direct(_) | Place::Group(_) => None,
        }
    }
}

/// `program` is how the exe is invoked (`mmsend`, or a quoted full path), `args`
/// names the recipient; `message` fills the forms that quote it, empty means show a
/// placeholder to replace.
pub fn command_line(program: &str, args: &str, kind: TemplateKind, message: &str) -> String {
    let exe = program;
    let message = if message.is_empty() { MESSAGE_PLACEHOLDER } else { message };
    match kind {
        TemplateKind::Inline => format!("{exe} {args} {}", quote_argument(message)),
        TemplateKind::File => format!("{exe} {args} --file \"{FILE_PLACEHOLDER}\""),
        // Command output is usually columns (dir, ipconfig) that only line up
        // in a code block.
        TemplateKind::Pipe => format!("{PIPE_PLACEHOLDER} | {exe} {args} --code"),
        TemplateKind::Attach => {
            format!("{exe} {args} --attach \"{ATTACH_PLACEHOLDER}\" {}", quote_argument(message))
        }
    }
}

/// One quoted argument that both cmd and mmsend's own argument parsing (the MSVC
/// rules) read back unchanged, the escaping BatBadBut (CVE-2024-24576) settled on:
/// a `"` is written `""`, which mmsend reads as one quote and which keeps cmd inside
/// the quoted text so `&` `|` `>` stay text; backslashes right before a quote,
/// the closing one included, are doubled so they do not escape it.
/// `%` is written `%%`, which a batch file reads as one `%` instead of expanding a
/// variable; pasted at a cmd prompt it stays `%%`, but scripts are what the copied
/// command is for, and the window sends a one-off message itself.
/// The window's message field is single-line, so no newline.
fn quote_argument(text: &str) -> String {
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in text.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
                quoted.push_str("\"\"");
                backslashes = 0;
            }
            '%' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push_str("%%");
                backslashes = 0;
            }
            c => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::User(username) => write!(f, "@{username}"),
            Target::Channel { team, channel } => write!(f, "{team}/{channel}"),
            Target::Group(members) => write!(f, "群組 {}", members.join(",")),
            Target::ChannelId(id) => write!(f, "頻道 {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_command_fills_in_the_message_or_a_placeholder() {
        let target = Target::User("alice".into());
        assert_eq!(
            command_line("mmsend", &target.cli_args(), TemplateKind::Inline, ""),
            r#"mmsend --user alice "訊息內容""#
        );
        assert_eq!(
            command_line("mmsend", &target.cli_args(), TemplateKind::Inline, "報表跑完了"),
            r#"mmsend --user alice "報表跑完了""#
        );
    }

    #[test]
    fn group_spec_is_normalized_so_member_order_does_not_matter() {
        let target = Target::parse_group(" @bob, alice ,bob").unwrap();
        assert_eq!(target, Target::Group(vec!["alice".into(), "bob".into()]));
        assert_eq!(
            command_line("mmsend", &target.cli_args(), TemplateKind::File, ""),
            r#"mmsend --group alice,bob --file "訊息.txt""#
        );
        assert_eq!(Target::parse_group(" , "), None);
    }

    #[test]
    fn quotes_percents_and_trailing_backslashes_survive_a_batch_file() {
        let args = Target::User("alice".into()).cli_args();
        assert_eq!(
            command_line("mmsend", &args, TemplateKind::Inline, r#"he said "hi" & 100% at %TEMP% \"x\" C:\out\"#),
            r#"mmsend --user alice "he said ""hi"" & 100%% at %%TEMP%% \\""x\\"" C:\out\\""#
        );
    }

    #[test]
    fn channel_command_round_trips_through_the_parser() {
        let target = Target::parse_channel("dev-team/town-square").unwrap();
        let command = command_line(r#""C:\t\mmsend.exe""#, &target.cli_args(), TemplateKind::Pipe, "ignored");
        assert_eq!(command, r#"您的指令 | "C:\t\mmsend.exe" --channel dev-team/town-square --code"#);
    }

    #[test]
    fn malformed_channel_spec_is_rejected() {
        assert_eq!(Target::parse_channel("town-square"), None);
        assert_eq!(Target::parse_channel("team/"), None);
        assert_eq!(Target::parse_channel("a/b/c"), None);
    }
}
