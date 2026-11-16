//! Pinned people and channels. A pin can carry an alias, so a script says
//! `mmsend --to 值班` and following a new on-call person means changing the alias
//! in the window, not every script.

use serde::{Deserialize, Serialize};

use crate::store;
use crate::target::Target;

const FAVORITES_FILE: &str = "favorites.json";
const MAX_ALIAS_CHARS: usize = 24;
/// Besides letters and digits (any script). Everything else is either a space
/// (would need quoting) or means something to cmd (`&|<>^%!"`).
const ALIAS_PUNCTUATION: [char; 3] = ['_', '-', '.'];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Favorite {
    pub target: Target,
    /// Empty: no alias.
    #[serde(default)]
    pub alias: String,
    /// How the list showed it when it was pinned, for when the directory does not
    /// list it (a person found by searching, say).
    pub title: String,
    pub subtitle: String,
    pub user_id: Option<String>,
    #[serde(default)]
    pub private: bool,
}

pub fn load() -> Result<Vec<Favorite>, String> {
    Ok(store::load_json(FAVORITES_FILE)?.unwrap_or_default())
}

pub fn save(favorites: &[Favorite]) -> Result<(), String> {
    store::save_json(FAVORITES_FILE, &favorites)
}

/// Applies `change` to the pins as saved right now and returns the result. The
/// window keeps its own copy, and `mmsend alias set` may have changed the file
/// since; saving that copy whole would silently undo it.
pub fn update(change: impl FnOnce(&mut Vec<Favorite>)) -> Result<Vec<Favorite>, String> {
    let mut favorites = load()?;
    change(&mut favorites);
    save(&favorites)?;
    Ok(favorites)
}

/// The window's pin button. `unpin` is what the button showed; `list` is the file
/// as it is now, where the command line may have pinned or given an alias meanwhile.
pub fn toggle_pin(list: &mut Vec<Favorite>, pin: Favorite, unpin: bool) {
    match list.iter().position(|f| f.target == pin.target) {
        // A pin with an alias stays, as the window itself requires clearing it first.
        Some(index) if unpin && list[index].alias.is_empty() => {
            list.remove(index);
        }
        None if !unpin => list.push(pin),
        _ => {}
    }
}

/// The window's alias field: sets the alias of the pin on `target`, or says why it cannot.
pub fn set_alias(list: &mut [Favorite], target: &Target, alias: &str) -> Option<String> {
    if let Some(problem) = alias_problem(alias, list, target) {
        return Some(problem);
    }
    if let Some(pinned) = list.iter_mut().find(|f| &f.target == target) {
        pinned.alias = alias.to_owned();
    }
    None
}

/// Aliases are matched without regard to case.
pub fn find_alias<'a>(favorites: &'a [Favorite], alias: &str) -> Option<&'a Favorite> {
    let wanted = alias.to_lowercase();
    favorites
        .iter()
        .find(|f| !f.alias.is_empty() && f.alias.to_lowercase() == wanted)
}

/// Why `alias` cannot be used for the pin on `own`, if it cannot.
pub fn alias_problem(alias: &str, favorites: &[Favorite], own: &Target) -> Option<String> {
    if alias.is_empty() {
        return None;
    }
    if alias.chars().count() > MAX_ALIAS_CHARS {
        return Some(format!("別名最多 {MAX_ALIAS_CHARS} 個字"));
    }
    if !alias.chars().all(|c| c.is_alphanumeric() || ALIAS_PUNCTUATION.contains(&c)) {
        return Some("別名只能使用文字、數字與 _ - .".to_owned());
    }
    if alias.starts_with('-') {
        return Some("別名不能以 - 開頭".to_owned());
    }
    match find_alias(favorites, alias) {
        Some(other) if &other.target != own => Some(format!("「{alias}」已是 {} 的別名", other.title)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(target: Target, alias: &str) -> Favorite {
        Favorite {
            title: target.to_string(),
            target,
            alias: alias.to_owned(),
            subtitle: String::new(),
            user_id: None,
            private: false,
        }
    }

    #[test]
    fn alias_must_be_safe_on_a_cmd_line_and_unique() {
        let bob = Target::User("bob".into());
        let carol = Target::User("carol".into());
        let favorites = vec![pin(bob.clone(), "值班"), pin(carol.clone(), "")];
        assert_eq!(alias_problem("值班", &favorites, &bob), None);
        assert_eq!(alias_problem("on-call_2", &favorites, &carol), None);
        assert!(alias_problem("值班", &favorites, &carol).is_some());
        assert!(alias_problem("a&b", &favorites, &carol).is_some());
        assert!(alias_problem("two words", &favorites, &carol).is_some());
        assert!(alias_problem("-x", &favorites, &carol).is_some());
        assert_eq!(find_alias(&favorites, "值班").map(|f| &f.target), Some(&bob));
    }

    #[test]
    fn window_changes_keep_an_alias_set_from_the_command_line() {
        let bob = Target::User("bob".into());
        let carol = Target::User("carol".into());
        // The file after `mmsend alias set 值班 --user bob`, which the open window never saw.
        let mut file = vec![pin(bob.clone(), "值班")];

        toggle_pin(&mut file, pin(carol.clone(), ""), false);
        assert_eq!(set_alias(&mut file, &carol, "早班"), None);
        // The window still showed bob unpinned, so its button meant "pin"; and an
        // alias holder is never unpinned.
        toggle_pin(&mut file, pin(bob.clone(), ""), false);
        toggle_pin(&mut file, pin(bob.clone(), ""), true);

        assert_eq!(find_alias(&file, "值班").map(|f| &f.target), Some(&bob));
        assert_eq!(find_alias(&file, "早班").map(|f| &f.target), Some(&carol));
        assert_eq!(file.len(), 2);
    }
}
