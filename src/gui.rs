//! The settings window: log in, pick a person or channel, copy (or try) the command.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CornerRadius, CursorIcon, FontData, FontFamily,
    FontId, FontInsert, FontPriority, Frame, Galley, InsertFontFamily, Key, Label, Layout, Margin,
    Painter, Pos2, Rect, Response, RichText, Sense, Shadow, Stroke, StrokeKind, TextEdit,
    TextureHandle, TextureId, TextureOptions, Vec2, vec2,
};
use eframe::egui::text::{LayoutJob, TextWrapping};
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

use crate::APP_NAME;
use crate::api::{self, ApiError, Client, NewPost, User};
use crate::favorites::{self, Favorite};
use crate::icon;
use crate::icons::{self, Icon};
use crate::install::{self, InstallState, Version};
use crate::session::{Error, Session};
use crate::store::{self, SendRecord};
use crate::target::{self, Target, TemplateKind};
use crate::theme::{self, CARD_RADIUS, Palette, RADIUS};

const WINDOW_SIZE: [f32; 2] = [520.0, 760.0];
const WINDOW_MIN_SIZE: [f32; 2] = [420.0, 600.0];
const WINDOW_ICON_SIZE: u32 = 64;
const LOGO_SIZE: f32 = 56.0;
const LOGIN_CARD_WIDTH: f32 = 380.0;
/// Rough card height, only used to centre the login card vertically.
const LOGIN_CARD_HEIGHT: f32 = 500.0;
const CONTROL_HEIGHT: f32 = 40.0;
const ICON_BUTTON_SIZE: f32 = 30.0;
const BUTTON_ICON_SIZE: f32 = 16.0;
const BUTTON_FONT_SIZE: f32 = 14.5;
const SEGMENT_HEIGHT: f32 = 28.0;
const SEGMENT_PADDING: f32 = 14.0;
const ROW_HEIGHT: f32 = 52.0;
const AVATAR_RADIUS: f32 = 17.0;
const HEADER_AVATAR_RADIUS: f32 = 18.0;
const SMALL_AVATAR_SIZE: f32 = 22.0;
/// Decoded profile-picture size: sharp for the 36 px circle on 200 % displays.
const AVATAR_PIXELS: u32 = 96;
const PAGE_MARGIN: i8 = 16;
const INPUT_MARGIN: Margin = Margin { left: 10, right: 10, top: 10, bottom: 10 };
const SEARCH_MARGIN: Margin = Margin { left: 36, right: 10, top: 10, bottom: 10 };
const PASSWORD_MARGIN: Margin = Margin { left: 10, right: 42, top: 10, bottom: 10 };
const HOVER_ANIMATION: f32 = 0.12;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);
const FEEDBACK_DURATION: Duration = Duration::from_secs(2);
const MENU_WIDTH: f32 = 176.0;
const MENU_ITEM_HEIGHT: f32 = 34.0;
const DIALOG_WIDTH: f32 = 460.0;
/// Space kept around a dialog when the window is narrower than the dialog.
const DIALOG_SIDE_GAP: f32 = 64.0;
const HISTORY_MAX_HEIGHT: f32 = 460.0;
const ALIAS_FIELD_WIDTH: f32 = 200.0;
/// Window space a dialog needs besides its scrolling list: title, padding, margins.
const DIALOG_CHROME_HEIGHT: f32 = 180.0;
const TOAST_DURATION: Duration = Duration::from_millis(3500);
const TOAST_ERROR_DURATION: Duration = Duration::from_secs(7);
const TOAST_FADE: Duration = Duration::from_millis(250);
/// Gap between the toast and whatever it floats above (the composer, or the window edge).
const TOAST_GAP: f32 = 14.0;
const TOAST_SLIDE: f32 = 10.0;
const TOAST_MAX_WIDTH: f32 = 380.0;
const INSTALL_LOCATION: &str = "安裝至 %LOCALAPPDATA%\\Programs\\mmsend";
const INSTALL_TILE_SIZE: f32 = 38.0;
const BENEFIT_ICON_SIZE: f32 = 15.0;
const CODE_CHIP_FONT_SIZE: f32 = 12.5;
const SEND_BUTTON_SIZE: f32 = 30.0;
/// Distance from the message field's right edge to the send button's centre.
const SEND_BUTTON_INSET: f32 = 21.0;
const MESSAGE_MARGIN: Margin = Margin { left: 10, right: 42, top: 10, bottom: 10 };
const GROUP_NAME_SEPARATOR: &str = "、";

const AVATAR_COLORS: [Color32; 8] = [
    Color32::from_rgb(0x25, 0x63, 0xeb),
    Color32::from_rgb(0x7c, 0x3a, 0xed),
    Color32::from_rgb(0xdb, 0x27, 0x77),
    Color32::from_rgb(0xea, 0x58, 0x0c),
    Color32::from_rgb(0x05, 0x96, 0x69),
    Color32::from_rgb(0x08, 0x91, 0xb2),
    Color32::from_rgb(0xca, 0x8a, 0x04),
    Color32::from_rgb(0x47, 0x55, 0x69),
];

/// Microsoft JhengHei ships with Traditional Chinese Windows; MingLiU is the older
/// fallback. Without either, eframe's system font provider still finds something,
/// but it may pick a Japanese face and render Japanese glyph variants.
const CJK_FONT_FILES: [&str; 2] = ["msjh.ttc", "mingliu.ttc"];
const CJK_FONT_NAME: &str = "cjk";

const TEMPLATE_CHOICES: [(TemplateKind, &str); 4] = [
    (TemplateKind::Inline, "單行文字"),
    (TemplateKind::File, "文字檔"),
    (TemplateKind::Pipe, "指令輸出"),
    (TemplateKind::Attach, "附加檔案"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    People,
    Channels,
}

const FILTER_CHOICES: [(Filter, &str); 3] = [
    (Filter::All, "全部"),
    (Filter::People, "人員"),
    (Filter::Channels, "頻道"),
];

pub fn run() -> ExitCode {
    let icon = egui::IconData {
        rgba: icon::rgba(WINDOW_ICON_SIZE),
        width: WINDOW_ICON_SIZE,
        height: WINDOW_ICON_SIZE,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{APP_NAME} v{}", Version::current()))
            .with_inner_size(WINDOW_SIZE)
            .with_min_inner_size(WINDOW_MIN_SIZE)
            .with_icon(icon),
        ..Default::default()
    };
    let result = eframe::run_native(
        install::COMMAND_NAME,
        options,
        Box::new(|cc| {
            install_cjk_font(&cc.egui_ctx);
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone(), cc.storage)))
        }),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // The console is already detached when double-clicked, so stderr goes nowhere.
            show_fatal_error(&format!("無法開啟視窗：{e}"));
            ExitCode::FAILURE
        }
    }
}

fn show_fatal_error(message: &str) {
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, caption) = (wide(message), wide(APP_NAME));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

fn install_cjk_font(ctx: &egui::Context) {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    let fonts_dir = PathBuf::from(windir).join("Fonts");
    let Some(bytes) = CJK_FONT_FILES
        .iter()
        .find_map(|file| std::fs::read(fonts_dir.join(file)).ok())
    else {
        return;
    };
    ctx.add_font(FontInsert::new(
        CJK_FONT_NAME,
        FontData::from_owned(bytes),
        vec![
            InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Highest,
            },
            // Keep the monospace face for ASCII in commands; CJK falls through to this.
            InsertFontFamily {
                family: FontFamily::Monospace,
                priority: FontPriority::Lowest,
            },
        ],
    ));
}

// ---------------------------------------------------------------------------
// Directory: who the user can send to

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Person,
    PublicChannel,
    PrivateChannel,
    Group,
}

#[derive(Clone)]
struct Entry {
    kind: EntryKind,
    title: String,
    subtitle: String,
    target: Target,
    /// Set for people, whose profile picture is shown.
    user_id: Option<String>,
    /// Last message time (ms), so recent conversations come first.
    last_activity: i64,
}

impl Entry {
    fn person(user: &User, last_activity: i64) -> Self {
        let full_name = user.full_name();
        let handle = format!("@{}", user.username);
        let (title, subtitle) = if full_name.is_empty() {
            (handle, String::new())
        } else {
            (full_name, handle)
        };
        Entry {
            kind: EntryKind::Person,
            title,
            subtitle,
            target: Target::User(user.username.clone()),
            user_id: Some(user.id.clone()),
            last_activity,
        }
    }

    fn matches(&self, needle_lowercase: &str) -> bool {
        self.title.to_lowercase().contains(needle_lowercase)
            || self.subtitle.to_lowercase().contains(needle_lowercase)
    }

    fn fits(&self, filter: Filter) -> bool {
        match filter {
            Filter::All => true,
            Filter::People => self.kind == EntryKind::Person,
            Filter::Channels => self.kind != EntryKind::Person,
        }
    }

    fn pin(&self) -> Favorite {
        Favorite {
            target: self.target.clone(),
            alias: String::new(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            user_id: self.user_id.clone(),
            private: self.kind == EntryKind::PrivateChannel,
        }
    }
}

/// A pinned item as a row: as the directory lists it when it does (names may have
/// changed since pinning), else as it was when pinned; the alias shown in front.
fn favorite_entry(favorite: &Favorite, directory: &Directory) -> Entry {
    let listed = directory
        .people
        .iter()
        .chain(&directory.channels)
        .find(|entry| entry.target == favorite.target);
    let mut entry = listed.cloned().unwrap_or_else(|| Entry {
        kind: match &favorite.target {
            Target::User(_) => EntryKind::Person,
            Target::Group(_) => EntryKind::Group,
            _ if favorite.private => EntryKind::PrivateChannel,
            _ => EntryKind::PublicChannel,
        },
        title: favorite.title.clone(),
        subtitle: favorite.subtitle.clone(),
        target: favorite.target.clone(),
        user_id: favorite.user_id.clone(),
        last_activity: 0,
    });
    if !favorite.alias.is_empty() {
        let alias = format!("別名 {}", favorite.alias);
        entry.subtitle = if entry.subtitle.is_empty() { alias } else { format!("{alias} · {}", entry.subtitle) };
    }
    entry
}

fn display_name(user: &User) -> String {
    let full_name = user.full_name();
    if full_name.is_empty() {
        user.username.clone()
    } else {
        full_name
    }
}

fn initial_of(name: &str) -> String {
    name.trim_start_matches('@')
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

fn avatar_color(seed: &str) -> Color32 {
    let sum: usize = seed.bytes().map(usize::from).sum();
    AVATAR_COLORS[sum % AVATAR_COLORS.len()]
}

fn by_recent_activity(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.last_activity.cmp(&a.last_activity).then_with(|| a.title.cmp(&b.title))
}

struct Directory {
    me: User,
    /// Yourself first (safe for trying things out), then people you have talked to.
    people: Vec<Entry>,
    /// Channels and group messages, most recently active first.
    channels: Vec<Entry>,
}

fn load_directory(session: &mut Session) -> Result<Directory, Error> {
    let me = session.call(|c| c.me())?;
    let teams = session.call(|c| c.my_teams())?;
    let show_team = teams.len() > 1;

    let mut seen = HashSet::new();
    let mut channels = Vec::new();
    let mut group_channels = Vec::new();
    let mut partners: HashMap<String, i64> = HashMap::new();
    for team in &teams {
        // Direct and group channels are repeated in every team's list.
        for channel in session.call(|c| c.my_channels(&team.id))? {
            if channel.delete_at != 0 || !seen.insert(channel.id.clone()) {
                continue;
            }
            match channel.kind.as_str() {
                api::CHANNEL_OPEN | api::CHANNEL_PRIVATE => {
                    let kind = if channel.kind == api::CHANNEL_OPEN {
                        EntryKind::PublicChannel
                    } else {
                        EntryKind::PrivateChannel
                    };
                    channels.push(Entry {
                        kind,
                        title: channel.display_name.clone(),
                        subtitle: if show_team { team.display_name.clone() } else { String::new() },
                        target: Target::Channel {
                            team: team.name.clone(),
                            channel: channel.name,
                        },
                        user_id: None,
                        last_activity: channel.last_post_at,
                    });
                }
                api::CHANNEL_DIRECT => {
                    if let Some(partner) = direct_partner(&channel.name, &me.id)
                        && partner != me.id
                    {
                        partners.insert(partner, channel.last_post_at);
                    }
                }
                api::CHANNEL_GROUP => group_channels.push(channel),
                _ => {}
            }
        }
    }

    // A group's own display name is a list of usernames; show people's names instead.
    for channel in group_channels {
        let members = session.call(|c| c.users_in_channel(&channel.id))?;
        let others: Vec<&User> = members.iter().filter(|u| u.id != me.id).collect();
        let Some(Target::Group(usernames)) =
            Target::parse_group(&others.iter().map(|u| u.username.as_str()).collect::<Vec<_>>().join(","))
        else {
            continue;
        };
        channels.push(Entry {
            kind: EntryKind::Group,
            title: others.iter().map(|u| display_name(u)).collect::<Vec<_>>().join(GROUP_NAME_SEPARATOR),
            subtitle: format!("群組 · {} 人", members.len()),
            target: Target::Group(usernames),
            user_id: None,
            last_activity: channel.last_post_at,
        });
    }

    let mut people: Vec<Entry> = if partners.is_empty() {
        Vec::new()
    } else {
        let ids: Vec<String> = partners.keys().cloned().collect();
        session
            .call(|c| c.users_by_ids(&ids))?
            .iter()
            .filter(|u| u.delete_at == 0)
            .map(|u| Entry::person(u, partners.get(&u.id).copied().unwrap_or_default()))
            .collect()
    };
    people.sort_by(by_recent_activity);
    channels.sort_by(by_recent_activity);

    let mut myself = Entry::person(&me, 0);
    // Mattermost's own zh-TW UI marks yourself the same way.
    myself.title = format!("{} (您)", display_name(&me));
    myself.subtitle = format!("@{}", me.username);
    people.insert(0, myself);
    Ok(Directory { me, people, channels })
}

/// Direct channels are named `<user id>__<user id>`; a self-DM repeats your own id.
fn direct_partner(channel_name: &str, me: &str) -> Option<String> {
    let (a, b) = channel_name.split_once("__")?;
    Some(if a == me { b } else { a }.to_owned())
}

/// Profile pictures, fetched lazily the first time a row asks for one.
#[derive(Default)]
struct Avatars {
    /// `None` while loading, or for good if the picture could not be fetched.
    slots: HashMap<String, Option<TextureHandle>>,
    wanted: Vec<String>,
}

impl Avatars {
    fn texture(&mut self, user_id: &str) -> Option<TextureId> {
        match self.slots.get(user_id) {
            Some(slot) => slot.as_ref().map(TextureHandle::id),
            None => {
                self.slots.insert(user_id.to_owned(), None);
                self.wanted.push(user_id.to_owned());
                None
            }
        }
    }
}

fn decode_avatar(bytes: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(bytes)
        .ok()?
        .resize_to_fill(AVATAR_PIXELS, AVATAR_PIXELS, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    ))
}

// ---------------------------------------------------------------------------
// App state

enum Msg {
    LoggedIn(Result<Box<Session>, Error>),
    Loaded(Result<Directory, Error>),
    Searched {
        query: String,
        result: Result<Vec<User>, Error>,
    },
    Sent(Result<(), Error>),
    Avatar {
        user_id: String,
        image: Option<ColorImage>,
    },
    Installed(Result<(), String>),
    Uninstalled(Result<(), String>),
}

/// The rows on screen, in display order.
struct Visible {
    pinned: Vec<Entry>,
    people: Vec<Entry>,
    channels: Vec<Entry>,
}

enum Action {
    None,
    Login,
    Logout,
    Reload,
    Search(String),
    Send { target: Target, message: String },
    Install,
    ShowHistory,
    ConfirmUninstall,
    Uninstall,
    ShowError(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UninstallStep {
    Idle,
    Confirming,
    Running,
    /// Everything is removed; only closing the window is left.
    Done,
}

#[derive(Default)]
struct LoginForm {
    server: String,
    login_id: String,
    password: String,
    show_password: bool,
    busy: bool,
    focus_pending: bool,
    error: Option<String>,
    error_detail: Option<String>,
}

impl LoginForm {
    fn prefilled(error: Option<String>) -> Self {
        let config = store::load_config().ok().flatten();
        LoginForm {
            server: config
                .as_ref()
                .map(|c| c.server_url.trim_end_matches('/').to_owned())
                .unwrap_or_default(),
            login_id: config.map(|c| c.login_id).unwrap_or_default(),
            focus_pending: true,
            error,
            ..Default::default()
        }
    }

    fn show_error(&mut self, error: &Error) {
        let (headline, detail) = match error {
            Error::Api(ApiError::Unauthorized) => ("帳號或密碼錯誤".to_owned(), None),
            Error::Api(ApiError::InvalidUrl(reason)) => (reason.clone(), None),
            Error::Api(ApiError::NotFound) => ("此網址不是 Mattermost 伺服器，請確認網址".to_owned(), None),
            Error::Api(ApiError::Network(detail)) => {
                ("無法連線至伺服器，請確認網址".to_owned(), Some(detail.clone()))
            }
            other => (other.to_string(), None),
        };
        self.busy = false;
        self.error = Some(headline);
        self.error_detail = detail;
    }
}

struct MainView {
    session: Arc<Mutex<Session>>,
    /// For profile pictures, so they never wait behind the session lock.
    image_client: Client,
    my_user_id: String,
    username: String,
    host: String,
    directory: Option<Directory>,
    load_error: Option<String>,
    avatars: Avatars,
    filter: Filter,
    query: String,
    query_edited_at: Option<Instant>,
    searched_query: String,
    found_people: Vec<Entry>,
    focus_search: bool,
    /// Last frame's composer height; toasts float just above it.
    composer_height: f32,
    selected: Option<Entry>,
    focus_message: bool,
    message: String,
    template: TemplateKind,
    copied_at: Option<Instant>,
    sending: bool,
    sent_at: Option<Instant>,
    send_error: Option<String>,
    favorites: Vec<Favorite>,
    /// The alias field of the selected pin, as typed (it may not be valid yet).
    alias_draft: String,
    alias_error: Option<String>,
}

impl MainView {
    fn select(&mut self, entry: Entry) {
        self.alias_draft = self.alias_of(&entry.target).unwrap_or_default().to_owned();
        self.alias_error = None;
        self.selected = Some(entry);
        self.focus_message = true;
        self.copied_at = None;
        self.sent_at = None;
        self.send_error = None;
    }

    fn favorite_index(&self, target: &Target) -> Option<usize> {
        self.favorites.iter().position(|f| &f.target == target)
    }

    fn alias_of(&self, target: &Target) -> Option<&str> {
        let favorite = &self.favorites[self.favorite_index(target)?];
        (!favorite.alias.is_empty()).then_some(favorite.alias.as_str())
    }

    fn visible_entries(&self) -> Visible {
        let Some(directory) = &self.directory else {
            return Visible { pinned: Vec::new(), people: Vec::new(), channels: Vec::new() };
        };
        let needle = self.query.trim().to_lowercase();
        let pinned: Vec<Entry> = self
            .favorites
            .iter()
            .map(|favorite| favorite_entry(favorite, directory))
            .filter(|e| e.fits(self.filter) && e.matches(&needle))
            .collect();
        let unpinned = |e: &&Entry| self.favorite_index(&e.target).is_none();
        let mut people: Vec<Entry> =
            directory.people.iter().filter(unpinned).filter(|e| e.matches(&needle)).cloned().collect();
        if !needle.is_empty() {
            let known: HashSet<Target> = people.iter().map(|e| e.target.clone()).collect();
            people.extend(
                self.found_people
                    .iter()
                    .filter(unpinned)
                    .filter(|e| !known.contains(&e.target))
                    .cloned(),
            );
        }
        let channels = directory
            .channels
            .iter()
            .filter(unpinned)
            .filter(|e| e.matches(&needle))
            .cloned()
            .collect();
        match self.filter {
            Filter::All => Visible { pinned, people, channels },
            Filter::People => Visible { pinned, people, channels: Vec::new() },
            Filter::Channels => Visible { pinned, people: Vec::new(), channels },
        }
    }
}

/// Copying commands needs the exe installed, so that `mmsend` is on the PATH.
struct Installation {
    state: InstallState,
    installing: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    Success,
    Error,
}

/// A short-lived message floating over the window, for outcomes that need no action.
struct Toast {
    kind: ToastKind,
    text: String,
    shown_at: Instant,
}

impl Toast {
    fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Toast {
            kind,
            text: text.into(),
            shown_at: Instant::now(),
        }
    }

    fn lifetime(&self) -> Duration {
        match self.kind {
            ToastKind::Success => TOAST_DURATION,
            // Errors carry a reason worth reading.
            ToastKind::Error => TOAST_ERROR_DURATION,
        }
    }
}

enum Screen {
    Login(LoginForm),
    Main(Box<MainView>),
}

struct App {
    ctx: egui::Context,
    logo: TextureHandle,
    installation: Installation,
    toast: Option<Toast>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    screen: Screen,
    /// The last window position and size eframe saved while the window was not
    /// minimized, as eframe stores it.
    window_geometry: Option<String>,
    /// The send log, while its dialog is open.
    history: Option<Result<Vec<SendRecord>, String>>,
    uninstall: UninstallStep,
}

/// eframe's (private) storage key for the window's position and size.
const EFRAME_WINDOW_KEY: &str = "window";

/// The part of eframe's saved window settings needed to tell a minimized window apart.
#[derive(serde::Deserialize)]
struct SavedWindow {
    normal_geometry: SavedGeometry,
}

#[derive(serde::Deserialize)]
struct SavedGeometry {
    inner_size_points: Vec2,
}

impl App {
    fn new(ctx: egui::Context, storage: Option<&dyn eframe::Storage>) -> Self {
        let (tx, rx) = mpsc::channel();
        let logo_pixels = (LOGO_SIZE * 2.0) as u32; // sharp on 200% displays
        let logo = ctx.load_texture(
            "logo",
            ColorImage::from_rgba_unmultiplied(
                [logo_pixels as usize, logo_pixels as usize],
                &icon::rgba(logo_pixels),
            ),
            TextureOptions::LINEAR,
        );
        let installation = Installation {
            state: install::state(),
            installing: false,
        };
        let mut app = App {
            ctx,
            logo,
            installation,
            toast: None,
            tx,
            rx,
            screen: Screen::Login(LoginForm::default()),
            window_geometry: storage.and_then(|s| s.get_string(EFRAME_WINDOW_KEY)),
            history: None,
            uninstall: UninstallStep::Idle,
        };
        match Session::load() {
            Ok(session) => app.enter_main(session),
            Err(Error::NotLoggedIn) => app.screen = Screen::Login(LoginForm::prefilled(None)),
            Err(e) => app.screen = Screen::Login(LoginForm::prefilled(Some(e.to_string()))),
        }
        app
    }

    fn spawn(&self, job: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            // The receiver only disappears when the window is closing.
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    fn enter_main(&mut self, session: Session) {
        let host = reqwest::Url::parse(&session.config.server_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default();
        let username = session.config.username.clone();
        let my_user_id = session.config.user_id.clone();
        let image_client = session.client();
        let session = Arc::new(Mutex::new(session));
        self.spawn_load(session.clone());
        let favorites = favorites::load().unwrap_or_else(|e| {
            self.toast = Some(Toast::new(ToastKind::Error, e));
            Vec::new()
        });
        self.screen = Screen::Main(Box::new(MainView {
            session,
            image_client,
            my_user_id,
            username,
            host,
            directory: None,
            load_error: None,
            avatars: Avatars::default(),
            filter: Filter::All,
            query: String::new(),
            query_edited_at: None,
            searched_query: String::new(),
            found_people: Vec::new(),
            focus_search: true,
            composer_height: 0.0,
            selected: None,
            focus_message: false,
            message: String::new(),
            template: TemplateKind::Inline,
            copied_at: None,
            sending: false,
            sent_at: None,
            send_error: None,
            favorites,
            alias_draft: String::new(),
            alias_error: None,
        }));
    }

    fn spawn_load(&self, session: Arc<Mutex<Session>>) {
        self.spawn(move || Msg::Loaded(load_directory(&mut lock(&session))));
    }

    fn fetch_wanted_avatars(&mut self) {
        let Screen::Main(view) = &mut self.screen else {
            return;
        };
        if view.avatars.wanted.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut view.avatars.wanted);
        let client = view.image_client.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            for user_id in ids {
                let image = client.user_image(&user_id).ok().and_then(|b| decode_avatar(&b));
                if tx.send(Msg::Avatar { user_id, image }).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if let Msg::Installed(result) = msg {
                self.installation.installing = false;
                self.toast = Some(match result {
                    Ok(()) => {
                        self.installation.state = InstallState::Installed;
                        Toast::new(ToastKind::Success, "安裝完成，請在新開啟的 cmd 視窗中使用 mmsend")
                    }
                    Err(e) => Toast::new(ToastKind::Error, e),
                });
                continue;
            }
            if let Msg::Uninstalled(result) = msg {
                match result {
                    Ok(()) => {
                        self.uninstall = UninstallStep::Done;
                        self.installation.state = InstallState::NotInstalled;
                    }
                    Err(e) => {
                        self.uninstall = UninstallStep::Idle;
                        self.toast = Some(Toast::new(ToastKind::Error, e));
                    }
                }
                continue;
            }
            match (msg, &mut self.screen) {
                (Msg::LoggedIn(Ok(session)), Screen::Login(_)) => self.enter_main(*session),
                (Msg::LoggedIn(Err(e)), Screen::Login(form)) => form.show_error(&e),
                (Msg::Loaded(Ok(directory)), Screen::Main(view)) => {
                    view.directory = Some(directory);
                    view.load_error = None;
                }
                (Msg::Loaded(Err(Error::Api(ApiError::Unauthorized))), Screen::Main(_)) => {
                    self.screen =
                        Screen::Login(LoginForm::prefilled(Some("登入已失效，請重新登入".into())));
                }
                (Msg::Loaded(Err(e)), Screen::Main(view)) => {
                    view.load_error = Some(format!("載入失敗，{e}"));
                }
                (Msg::Searched { query, result }, Screen::Main(view)) => {
                    if view.query.trim() == query {
                        view.found_people = result
                            .map(|users| {
                                users
                                    .iter()
                                    .filter(|u| u.delete_at == 0)
                                    .map(|u| Entry::person(u, 0))
                                    .collect()
                            })
                            .unwrap_or_default();
                    }
                }
                (Msg::Sent(result), Screen::Main(view)) => {
                    view.sending = false;
                    match result {
                        Ok(()) => view.sent_at = Some(Instant::now()),
                        Err(e) => view.send_error = Some(send_error_text(&e)),
                    }
                }
                (Msg::Avatar { user_id, image }, Screen::Main(view)) => {
                    let texture = image.map(|image| {
                        self.ctx.load_texture(format!("avatar-{user_id}"), image, TextureOptions::LINEAR)
                    });
                    view.avatars.slots.insert(user_id, texture);
                }
                // A reply that arrived after the user switched screens.
                _ => {}
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Login => {
                let Screen::Login(form) = &mut self.screen else {
                    return;
                };
                form.busy = true;
                form.error = None;
                form.error_detail = None;
                let (server, login_id, password) =
                    (form.server.clone(), form.login_id.clone(), form.password.clone());
                self.spawn(move || {
                    Msg::LoggedIn(Session::login(&server, &login_id, &password).map(Box::new))
                });
            }
            Action::Logout => {
                let Screen::Main(view) = &self.screen else {
                    return;
                };
                let session = view.session.clone();
                let error = store::clear_secrets().err();
                std::thread::spawn(move || {
                    // Best effort: the local secrets are already gone either way.
                    let _ = lock(&session).revoke_token();
                });
                self.screen = Screen::Login(LoginForm::prefilled(error));
            }
            Action::Reload => {
                if let Screen::Main(view) = &self.screen {
                    self.spawn_load(view.session.clone());
                }
            }
            Action::Search(query) => {
                let Screen::Main(view) = &mut self.screen else {
                    return;
                };
                view.searched_query = query.clone();
                let session = view.session.clone();
                self.spawn(move || {
                    let result = lock(&session).call(|c| c.search_users(&query));
                    Msg::Searched { query, result }
                });
            }
            Action::Send { target, message } => {
                let Screen::Main(view) = &mut self.screen else {
                    return;
                };
                view.sending = true;
                view.sent_at = None;
                view.send_error = None;
                let session = view.session.clone();
                self.spawn(move || {
                    let mut session = lock(&session);
                    let pending_post_id = session.new_pending_post_id();
                    let result = target.resolve_channel_id(&mut session).and_then(|channel_id| {
                        session.call(|c| {
                            c.create_post(&NewPost {
                                channel_id: &channel_id,
                                message: &message,
                                pending_post_id: &pending_post_id,
                                file_ids: &[],
                            })
                        })
                    });
                    Msg::Sent(result)
                });
            }
            Action::Install => {
                self.installation.installing = true;
                self.spawn(|| Msg::Installed(install::install()));
            }
            Action::ShowHistory => self.history = Some(store::load_send_records()),
            Action::ShowError(text) => self.toast = Some(Toast::new(ToastKind::Error, text)),
            Action::ConfirmUninstall => self.uninstall = UninstallStep::Confirming,
            Action::Uninstall => {
                self.uninstall = UninstallStep::Running;
                let session = match &self.screen {
                    Screen::Main(view) => Some(view.session.clone()),
                    Screen::Login(_) => None,
                };
                self.spawn(move || {
                    if let Some(session) = session {
                        // Best effort: the token is useless once the local copy is gone.
                        let _ = lock(&session).revoke_token();
                    }
                    let result = install::uninstall()
                        .and_then(|()| store::clear_secrets())
                        .and_then(|()| store::remove_app_dir());
                    Msg::Uninstalled(result)
                });
            }
        }
    }

    fn show_uninstall(&mut self, ctx: &egui::Context, p: &Palette) {
        if self.uninstall == UninstallStep::Idle {
            return;
        }
        match uninstall_dialog(ctx, self.uninstall, p) {
            Some(true) => self.apply(Action::Uninstall),
            Some(false) => self.uninstall = UninstallStep::Idle,
            None => {}
        }
    }

    fn show_history(&mut self, ctx: &egui::Context, p: &Palette) {
        if let Some(records) = &self.history
            && history_dialog(ctx, records, p)
        {
            self.history = None;
        }
    }
}

/// `Some(true)`: go ahead; `Some(false)`: cancelled.
fn uninstall_dialog(ctx: &egui::Context, step: UninstallStep, p: &Palette) -> Option<bool> {
    let mut choice = None;
    let modal = egui::Modal::new(egui::Id::unique("uninstall")).frame(dialog_frame(p)).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - DIALOG_SIDE_GAP).min(DIALOG_WIDTH));
        let (icon, color, title) = if step == UninstallStep::Done {
            (Icon::Check, p.success, "已解除安裝")
        } else {
            (Icon::Trash, p.danger, "解除安裝「Mattermost 指令傳訊」")
        };
        ui.horizontal(|ui| {
            paint_icon(ui, icon, 20.0, color, p.surface);
            ui.label(RichText::new(title).size(17.0).strong().color(p.text));
        });
        ui.add_space(10.0);
        let width = ui.available_width();
        if step == UninstallStep::Done {
            if button(ui, ButtonKind::Primary, None, "關閉視窗", vec2(width, CONTROL_HEIGHT), true, None).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        ui.label(RichText::new("移除 mmsend 指令、開始功能表捷徑與登入資料。").color(p.text));
        ui.label(RichText::new("已寫好的批次檔將無法再傳送訊息。").color(p.danger));
        ui.add_space(16.0);
        let running = step == UninstallStep::Running;
        let half = vec2((width - ui.spacing().item_spacing.x) / 2.0, CONTROL_HEIGHT);
        ui.horizontal(|ui| {
            if button(ui, ButtonKind::Secondary, None, "取消", half, !running, None).clicked() {
                choice = Some(false);
            }
            let label = if running { "解除安裝中…" } else { "解除安裝" };
            if button(ui, ButtonKind::Danger, Some(Icon::Trash), label, half, !running, None).clicked() {
                choice = Some(true);
            }
        });
    });
    // Esc or a click outside cancels, but only before anything has been removed.
    let dismissed = step == UninstallStep::Confirming && modal.should_close();
    choice.or(dismissed.then_some(false))
}

/// Returns true when the dialog should close.
fn history_dialog(ctx: &egui::Context, records: &Result<Vec<SendRecord>, String>, p: &Palette) -> bool {
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::unique("history")).frame(dialog_frame(p)).show(ctx, |ui| {
        ui.set_width((ctx.content_rect().width() - DIALOG_SIDE_GAP).min(DIALOG_WIDTH));
        ui.horizontal(|ui| {
            paint_icon(ui, Icon::History, 20.0, p.text, p.surface);
            ui.label(RichText::new("傳送紀錄").size(17.0).strong().color(p.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = icon_button(ui, Icon::Close, "關閉（Esc）").clicked();
            });
        });
        ui.add_space(8.0);
        match records {
            Err(e) => error_box(ui, e, None, p),
            Ok(records) if records.is_empty() => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    paint_icon(ui, Icon::History, 28.0, p.text_weak, p.surface);
                    ui.label(RichText::new("執行 mmsend 指令後，結果會列在這裡").color(p.text_weak));
                });
                ui.add_space(24.0);
            }
            Ok(records) => {
                // Without a minimum the list opens only a few rows tall.
                let height = (ctx.content_rect().height() - DIALOG_CHROME_HEIGHT).min(HISTORY_MAX_HEIGHT);
                egui::ScrollArea::vertical().min_scrolled_height(height).max_height(height).show(ui, |ui| {
                    for record in records {
                        record_row(ui, record, p);
                    }
                });
            }
        }
    });
    close || modal.should_close()
}

fn dialog_frame(p: &Palette) -> Frame {
    Frame::default()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(20))
        .shadow(Shadow {
            offset: [0, 10],
            blur: 32,
            spread: 0,
            color: Color32::from_black_alpha(48),
        })
}

fn record_row(ui: &mut egui::Ui, record: &SendRecord, p: &Palette) {
    ui.horizontal_top(|ui| {
        let (icon, color) = if record.ok { (Icon::Check, p.success) } else { (Icon::Warning, p.danger) };
        ui.vertical(|ui| {
            // Lines the icon up with the first line of text.
            ui.add_space(5.0);
            paint_icon(ui, icon, BUTTON_ICON_SIZE, color, p.surface);
        });
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                let target = if record.target.is_empty() { "指令有誤" } else { &record.target };
                ui.label(RichText::new(target).size(15.0).strong().color(p.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(&record.time).size(12.5).color(p.text_weak));
                });
            });
            if !record.summary.is_empty() {
                ui.add(Label::new(RichText::new(&record.summary).size(13.5).color(p.text_weak)).truncate());
            }
            if !record.ok {
                ui.add(Label::new(RichText::new(&record.detail).size(12.5).color(p.danger)).wrap());
            }
        });
    });
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
}

fn send_error_text(error: &Error) -> String {
    match error {
        Error::Api(ApiError::Unauthorized) => "登入已失效，請登出後重新登入".into(),
        Error::Api(ApiError::Forbidden) => "無權限傳送至此對象".into(),
        Error::Api(ApiError::NotFound) => "找不到收件對象，可能已被刪除".into(),
        Error::Api(ApiError::Network(_)) => "無法連線至伺服器，請稍後再試".into(),
        other => other.to_string(),
    }
}

fn lock(session: &Mutex<Session>) -> std::sync::MutexGuard<'_, Session> {
    session.lock().expect("a background request panicked while holding the session")
}

impl eframe::App for App {
    /// Only the window's position and size are worth keeping (eframe saves those to
    /// %APPDATA%\mmsend\data\app.ron); egui's own memory would just add widget state.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// Runs right after eframe stored the window's geometry. A minimized window
    /// reports a tiny one at the screen corner (closing from the taskbar does
    /// this), which would reopen the window there; keep the last real one instead.
    /// Runs after eframe's final save, which has just written the window position
    /// back into the settings folder that uninstalling removed.
    fn on_exit(&mut self) {
        if self.uninstall == UninstallStep::Done {
            let _ = store::remove_app_dir();
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let minimized = eframe::get_value::<SavedWindow>(storage, EFRAME_WINDOW_KEY).is_some_and(|w| {
            let size = w.normal_geometry.inner_size_points;
            size.x < WINDOW_MIN_SIZE[0] || size.y < WINDOW_MIN_SIZE[1]
        });
        if !minimized {
            self.window_geometry = storage.get_string(EFRAME_WINDOW_KEY);
        } else if let Some(last) = &self.window_geometry {
            storage.set_string(EFRAME_WINDOW_KEY, last.clone());
        } else {
            storage.remove_string(EFRAME_WINDOW_KEY);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_messages();
        let palette = theme::palette(ui.ctx());
        let action = match &mut self.screen {
            Screen::Login(form) => login_ui(ui, form, &self.logo, palette),
            Screen::Main(view) => main_ui(ui, view, &self.installation, palette),
        };
        self.apply(action);
        self.fetch_wanted_avatars();
        self.show_history(ui.ctx(), palette);
        self.show_uninstall(ui.ctx(), palette);
        self.show_toast(ui.ctx(), palette);
    }
}

impl App {
    fn show_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(toast) = &self.toast else {
            return;
        };
        let age = toast.shown_at.elapsed().as_secs_f32();
        let lifetime = toast.lifetime().as_secs_f32();
        if age >= lifetime {
            self.toast = None;
            return;
        }
        let fade = TOAST_FADE.as_secs_f32();
        let opacity = (age / fade).min((lifetime - age) / fade).min(1.0);
        let (icon, accent) = match toast.kind {
            ToastKind::Success => (Icon::Check, p.success),
            ToastKind::Error => (Icon::Warning, p.danger),
        };
        let background = p.toast_background;
        // Snackbar position: just above the composer, so it never covers its buttons.
        let above = match &self.screen {
            Screen::Main(view) => view.composer_height,
            Screen::Login(_) => 0.0,
        } + TOAST_GAP;
        egui::Area::new(egui::Id::unique("toast"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -above + (1.0 - opacity) * TOAST_SLIDE))
            .show(ctx, |ui| {
                ui.set_opacity(opacity);
                Frame::default()
                    .fill(background)
                    .stroke(Stroke::new(1.0, p.toast_border))
                    .corner_radius(CornerRadius::same(CARD_RADIUS))
                    .inner_margin(Margin::symmetric(16, 10))
                    .shadow(Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: Color32::from_black_alpha(40),
                    })
                    .show(ui, |ui| {
                        ui.set_max_width(TOAST_MAX_WIDTH);
                        ui.horizontal(|ui| {
                            paint_icon(ui, icon, BUTTON_ICON_SIZE, accent, background);
                            ui.add(Label::new(RichText::new(&toast.text).color(p.toast_text)).wrap());
                        });
                    });
            });
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------------------
// Shared widgets

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    /// Confirms something that cannot be undone.
    Danger,
    Segment { selected: bool },
}

fn button_colors(kind: ButtonKind, hover: f32, pressed: bool, enabled: bool, p: &Palette) -> (Color32, Color32, Stroke) {
    let transparent = Color32::TRANSPARENT;
    match kind {
        ButtonKind::Primary => {
            let fill = if !enabled {
                p.accent.gamma_multiply(0.45)
            } else if pressed {
                p.accent_pressed
            } else {
                p.accent.lerp_to_gamma(p.accent_hover, hover)
            };
            (fill, p.on_accent, Stroke::NONE)
        }
        ButtonKind::Secondary => {
            let fill = if pressed { p.accent_soft } else { p.surface.lerp_to_gamma(p.hover, hover) };
            let border = if enabled { p.border.lerp_to_gamma(p.accent, hover) } else { p.border };
            let text = if enabled { p.text } else { p.text_weak.gamma_multiply(0.6) };
            (fill, text, Stroke::new(1.0, border))
        }
        ButtonKind::Ghost => {
            let fill = if pressed { p.border } else { transparent.lerp_to_gamma(p.hover, hover) };
            (fill, p.text_weak.lerp_to_gamma(p.text, hover), Stroke::NONE)
        }
        ButtonKind::Danger => {
            const HOVER_DARKEN: f32 = 0.15;
            let fill = if !enabled {
                p.danger_fill.gamma_multiply(0.45)
            } else if pressed {
                p.danger_fill.lerp_to_gamma(Color32::BLACK, HOVER_DARKEN * 2.0)
            } else {
                p.danger_fill.lerp_to_gamma(Color32::BLACK, HOVER_DARKEN * hover)
            };
            (fill, Color32::WHITE, Stroke::NONE)
        }
        ButtonKind::Segment { selected: true } => (p.surface, p.text, Stroke::new(1.0, p.border)),
        ButtonKind::Segment { selected: false } => (
            transparent.lerp_to_gamma(p.surface.gamma_multiply(0.6), hover),
            p.text_weak.lerp_to_gamma(p.text, hover),
            Stroke::NONE,
        ),
    }
}

fn button_font(kind: ButtonKind) -> FontId {
    match kind {
        ButtonKind::Segment { .. } => FontId::proportional(14.0),
        _ => FontId::proportional(BUTTON_FONT_SIZE),
    }
}

/// One button style for the whole app: centred icon + label, hover and press feedback.
/// Icon-only buttons pass an empty `text` and describe themselves with `tooltip`.
fn button(
    ui: &mut egui::Ui,
    kind: ButtonKind,
    icon: Option<Icon>,
    text: &str,
    size: Vec2,
    enabled: bool,
    tooltip: Option<&str>,
) -> Response {
    let p = theme::palette(ui.ctx());
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, mut response) = ui.allocate_exact_size(size, sense);
    let label = if text.is_empty() { tooltip.unwrap_or_default() } else { text };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let hover = ui
            .ctx()
            .animate_bool_with_time(response.id.with("hover"), enabled && response.hovered(), HOVER_ANIMATION);
        let pressed = enabled && response.is_pointer_button_down_on();
        let (fill, foreground, stroke) = button_colors(kind, hover, pressed, enabled, p);
        let painter = ui.painter();
        let corner = match kind {
            ButtonKind::Segment { .. } => CornerRadius::same(RADIUS - 2),
            _ => CornerRadius::same(RADIUS),
        };
        painter.rect(rect, corner, fill, stroke, StrokeKind::Inside);

        let galley = painter.layout_no_wrap(text.to_owned(), button_font(kind), foreground);
        let icon_width = if icon.is_some() { BUTTON_ICON_SIZE } else { 0.0 };
        let gap = if icon.is_some() && !text.is_empty() { 7.0 } else { 0.0 };
        let mut x = rect.center().x - (icon_width + gap + galley.size().x) / 2.0;
        if let Some(icon) = icon {
            let icon_rect = Rect::from_min_size(
                Pos2::new(x, rect.center().y - BUTTON_ICON_SIZE / 2.0),
                Vec2::splat(BUTTON_ICON_SIZE),
            );
            icons::paint(painter, icon, icon_rect, foreground, fill);
            x += icon_width + gap;
        }
        painter.galley(Pos2::new(x, rect.center().y - galley.size().y / 2.0), galley, foreground);
    }
    if let Some(tip) = tooltip {
        response = response.on_hover_text(tip);
    }
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    }
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> Response {
    button(ui, ButtonKind::Ghost, Some(icon), "", Vec2::splat(ICON_BUTTON_SIZE), true, Some(tooltip))
}

/// A row in a drop-down menu: icon and label, left-aligned.
fn menu_item(ui: &mut egui::Ui, icon: Icon, text: &str, color: Color32) -> Response {
    let p = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), MENU_ITEM_HEIGHT), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), HOVER_ANIMATION);
    let fill = Color32::TRANSPARENT.lerp_to_gamma(p.hover, hover);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(RADIUS - 2), fill);
    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 10.0 + BUTTON_ICON_SIZE / 2.0, rect.center().y),
        Vec2::splat(BUTTON_ICON_SIZE),
    );
    icons::paint(painter, icon, icon_rect, color, p.surface);
    painter.text(
        Pos2::new(icon_rect.right() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(BUTTON_FONT_SIZE),
        color,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A pill-shaped single-choice control. Returns true when the choice changed.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, choices: &[(T, &str)], p: &Palette) -> bool {
    let mut changed = false;
    Frame::default()
        .fill(p.hover)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for &(choice, label) in choices {
                    let selected = *value == choice;
                    let kind = ButtonKind::Segment { selected };
                    let width = ui
                        .painter()
                        .layout_no_wrap(label.to_owned(), button_font(kind), p.text)
                        .size()
                        .x
                        + SEGMENT_PADDING * 2.0;
                    let size = vec2(width, SEGMENT_HEIGHT);
                    if button(ui, kind, None, label, size, true, None).clicked() && !selected {
                        *value = choice;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// A single line that ends in "…" instead of overflowing.
fn elided(painter: &Painter, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    painter.layout_job(job)
}

fn paint_initial_circle(painter: &Painter, center: Pos2, radius: f32, initial: &str, color: Color32) {
    painter.circle_filled(center, radius, color);
    painter.text(
        center,
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(radius * 0.95),
        Color32::WHITE,
    );
}

/// The profile picture once loaded; the coloured initial until then (or if it fails).
fn paint_user_avatar(ui: &egui::Ui, avatars: &mut Avatars, rect: Rect, user_id: &str, name: &str) {
    match avatars.texture(user_id) {
        Some(texture) => egui::Image::from_texture(egui::load::SizedTexture::new(texture, rect.size()))
            .corner_radius(CornerRadius::same((rect.width() / 2.0) as u8))
            .paint_at(ui, rect),
        None => paint_initial_circle(ui.painter(), rect.center(), rect.width() / 2.0, &initial_of(name), avatar_color(user_id)),
    }
}

fn paint_avatar(ui: &egui::Ui, avatars: &mut Avatars, rect: Rect, entry: &Entry, p: &Palette) {
    if let (EntryKind::Person, Some(user_id)) = (entry.kind, &entry.user_id) {
        paint_user_avatar(ui, avatars, rect, user_id, &entry.title);
        return;
    }
    let icon = match entry.kind {
        EntryKind::PublicChannel => Icon::Hash,
        EntryKind::PrivateChannel => Icon::Lock,
        EntryKind::Group => Icon::Group,
        EntryKind::Person => Icon::Chat,
    };
    ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), p.accent_soft);
    icons::paint(ui.painter(), icon, rect.shrink(rect.width() * 0.22), p.accent, p.accent_soft);
}

fn paint_icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32, background: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    icons::paint(ui.painter(), icon, rect, color, background);
}

/// Returns the label's id so the field below can be `labelled_by` it (its
/// accessible name for screen readers).
fn field_label(ui: &mut egui::Ui, text: &str, p: &Palette) -> egui::Id {
    ui.label(RichText::new(text).size(13.5).color(p.text_weak)).id
}

fn notice(ui: &mut egui::Ui, icon: Icon, text: &str, color: Color32, fill: Color32) {
    Frame::default()
        .fill(fill)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                paint_icon(ui, icon, BUTTON_ICON_SIZE, color, fill);
                ui.add(Label::new(RichText::new(text).color(color)).wrap());
            });
        });
}

fn error_box(ui: &mut egui::Ui, headline: &str, detail: Option<&str>, p: &Palette) {
    notice(ui, Icon::Warning, headline, p.danger, p.danger_soft);
    if let Some(detail) = detail {
        ui.label(RichText::new(detail).small().color(p.text_weak));
    }
}

fn centered_column(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width.min(ui.available_width()));
            add(ui);
        });
    });
}

// ---------------------------------------------------------------------------
// Login screen

fn login_ui(ui: &mut egui::Ui, form: &mut LoginForm, logo: &TextureHandle, p: &Palette) -> Action {
    let mut action = Action::None;
    egui::CentralPanel::default()
        .frame(Frame::default().fill(p.background))
        .show(ui, |ui| {
            ui.add_space(((ui.available_height() - LOGIN_CARD_HEIGHT) / 2.0).max(PAGE_MARGIN as f32));
            centered_column(ui, LOGIN_CARD_WIDTH, |ui| {
                Frame::default()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(CARD_RADIUS))
                    .inner_margin(Margin::same(28))
                    .shadow(Shadow {
                        offset: [0, 8],
                        blur: 28,
                        spread: 0,
                        color: Color32::from_black_alpha(22),
                    })
                    .show(ui, |ui| login_card(ui, form, logo, p, &mut action));
            });
        });
    action
}

fn login_card(ui: &mut egui::Ui, form: &mut LoginForm, logo: &TextureHandle, p: &Palette, action: &mut Action) {
    ui.set_width(ui.available_width());
    ui.vertical_centered(|ui| {
        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(logo.id(), Vec2::splat(LOGO_SIZE))));
        ui.add_space(8.0);
        ui.label(RichText::new(APP_NAME).size(21.0).strong().color(p.text));
        ui.label(RichText::new("登入您的 Mattermost 帳號").size(13.5).color(p.text_weak));
    });
    ui.add_space(18.0);

    let server_label = field_label(ui, "網址", p);
    let server = ui
        .add(
            TextEdit::singleline(&mut form.server)
                .hint_text("https://chat.example.com")
                .margin(INPUT_MARGIN)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(server_label);
    ui.add_space(4.0);
    let login_label = field_label(ui, "帳號", p);
    let login_id = ui
        .add(
            TextEdit::singleline(&mut form.login_id)
                .margin(INPUT_MARGIN)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(login_label);
    ui.add_space(4.0);
    let password_label = field_label(ui, "密碼", p);
    let password = ui
        .add(
            TextEdit::singleline(&mut form.password)
                .password(!form.show_password)
                .margin(PASSWORD_MARGIN)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(password_label);
    password_toggle(ui, &password, &mut form.show_password, p);
    // Like native password boxes, switch the IME off: a Chinese IME would show the
    // password in its composition window, and none of these fields take Chinese.
    if server.has_focus() || login_id.has_focus() || password.has_focus() {
        ui.ctx().output_mut(|o| o.ime = None);
    }

    if form.focus_pending {
        form.focus_pending = false;
        let first_empty = [
            (form.server.trim().is_empty(), &server),
            (form.login_id.trim().is_empty(), &login_id),
        ]
        .into_iter()
        .find(|(empty, _)| *empty)
        .map(|(_, field)| field)
        .unwrap_or(&password);
        first_empty.request_focus();
    }

    let enter_from = |field: &Response| field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    if enter_from(&server) {
        login_id.request_focus();
    } else if enter_from(&login_id) {
        password.request_focus();
    }
    let pressed_enter = enter_from(&password);

    ui.add_space(16.0);
    let ready = !form.busy
        && !form.server.trim().is_empty()
        && !form.login_id.trim().is_empty()
        && !form.password.is_empty();
    let label = if form.busy { "登入中…" } else { "登入" };
    let size = vec2(ui.available_width(), CONTROL_HEIGHT);
    let clicked = button(ui, ButtonKind::Primary, Some(Icon::Login), label, size, ready, None).clicked();
    if ready && (clicked || pressed_enter) {
        *action = Action::Login;
    }

    if let Some(error) = &form.error {
        ui.add_space(10.0);
        error_box(ui, error, form.error_detail.as_deref(), p);
    }
}

/// The eye inside the right end of the password field.
fn password_toggle(ui: &mut egui::Ui, field: &Response, shown: &mut bool, p: &Palette) {
    let rect = Rect::from_center_size(
        Pos2::new(field.rect.right() - 21.0, field.rect.center().y),
        Vec2::splat(ICON_BUTTON_SIZE - 2.0),
    );
    let label = if *shown { "隱藏密碼" } else { "顯示密碼" };
    let response = ui
        .interact(rect, ui.scope_id().with("password-toggle"), Sense::click())
        .on_hover_text(label)
        .on_hover_cursor(CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), HOVER_ANIMATION);
    if hover > 0.0 {
        ui.painter().circle_filled(rect.center(), rect.width() / 2.0, p.hover.gamma_multiply(hover));
    }
    let icon = if *shown { Icon::EyeOff } else { Icon::Eye };
    let color = p.text_weak.lerp_to_gamma(p.text, hover);
    icons::paint(ui.painter(), icon, rect.shrink(6.0), color, p.surface);
    if response.clicked() {
        *shown = !*shown;
        field.request_focus();
    }
}

// ---------------------------------------------------------------------------
// Main screen

fn main_ui(ui: &mut egui::Ui, view: &mut MainView, installation: &Installation, p: &Palette) -> Action {
    let mut action = Action::None;

    egui::Panel::top("header")
        .frame(Frame::default().fill(p.surface).inner_margin(Margin::symmetric(PAGE_MARGIN, 12)))
        .show(ui, |ui| header(ui, view, installation.state != InstallState::NotInstalled, p, &mut action));

    let composer = egui::Panel::bottom("composer")
        .frame(Frame::default().fill(p.surface).inner_margin(Margin::same(PAGE_MARGIN)))
        .show(ui, |ui| match view.selected.clone() {
            Some(entry) => composer(ui, view, &entry, installation, p, &mut action),
            None => {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    paint_icon(ui, Icon::Chat, 26.0, p.text_weak, p.surface);
                    ui.label(RichText::new("請選擇人員或頻道").color(p.text_weak));
                    ui.add_space(4.0);
                });
            }
        });

    view.composer_height = composer.response.rect.height();

    egui::CentralPanel::default()
        .frame(Frame::default().fill(p.background).inner_margin(Margin::symmetric(PAGE_MARGIN, 12)))
        .show(ui, |ui| directory_panel(ui, view, p, &mut action));

    if view.selected.is_some() && ui.input(|i| i.key_pressed(Key::Escape)) {
        view.selected = None;
    }

    if matches!(action, Action::None)
        && let Some(edited_at) = view.query_edited_at
    {
        let wait = SEARCH_DEBOUNCE.saturating_sub(edited_at.elapsed());
        if wait.is_zero() {
            view.query_edited_at = None;
            let query = view.query.trim().to_owned();
            if query.is_empty() {
                view.found_people.clear();
                view.searched_query.clear();
            } else if query != view.searched_query {
                action = Action::Search(query);
            }
        } else {
            ui.ctx().request_repaint_after(wait);
        }
    }
    action
}

fn header(ui: &mut egui::Ui, view: &mut MainView, installed: bool, p: &Palette, action: &mut Action) {
    let name = view
        .directory
        .as_ref()
        .map(|d| display_name(&d.me))
        .unwrap_or_else(|| view.username.clone());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(HEADER_AVATAR_RADIUS * 2.0), Sense::hover());
        let my_id = view.my_user_id.clone();
        paint_user_avatar(ui, &mut view.avatars, rect, &my_id, &name);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let more = icon_button(ui, Icon::More, "更多");
            egui::Popup::menu(&more)
                .align(egui::RectAlign::BOTTOM_END)
                .width(MENU_WIDTH)
                .show(|ui| {
                    if menu_item(ui, Icon::History, "傳送紀錄", p.text).clicked() {
                        *action = Action::ShowHistory;
                    }
                    if installed && menu_item(ui, Icon::Trash, "解除安裝…", p.danger).clicked() {
                        *action = Action::ConfirmUninstall;
                    }
                });
            let logout = button(ui, ButtonKind::Ghost, Some(Icon::Logout), "登出", vec2(84.0, 34.0), true, None);
            if logout.clicked() {
                *action = Action::Logout;
            }
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.add(Label::new(RichText::new(&name).strong().color(p.text)).truncate());
                let handle = format!("@{} · {}", view.username, view.host);
                ui.add(Label::new(RichText::new(handle).small().color(p.text_weak)).truncate());
            });
        });
    });
}

fn directory_panel(ui: &mut egui::Ui, view: &mut MainView, p: &Palette, action: &mut Action) {
    let search = ui.add(
        TextEdit::singleline(&mut view.query)
            .hint_text("搜尋人員或頻道")
            .margin(SEARCH_MARGIN)
            .desired_width(f32::INFINITY),
    );
    let icon_rect = Rect::from_center_size(search.rect.left_center() + vec2(19.0, 0.0), Vec2::splat(BUTTON_ICON_SIZE));
    icons::paint(ui.painter(), Icon::Search, icon_rect, p.text_weak, p.surface);
    if view.focus_search {
        view.focus_search = false;
        search.request_focus();
    }
    if search.changed() {
        view.query_edited_at = Some(Instant::now());
    }
    let pick_first = search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

    ui.add_space(2.0);
    segmented(ui, &mut view.filter, &FILTER_CHOICES, p);
    ui.add_space(4.0);

    if let Some(error) = view.load_error.clone() {
        error_box(ui, &error, None, p);
        ui.add_space(6.0);
        if button(ui, ButtonKind::Secondary, Some(Icon::Refresh), "重試", vec2(96.0, 36.0), true, None).clicked() {
            view.load_error = None;
            view.directory = None;
            *action = Action::Reload;
        }
        return;
    }
    if view.directory.is_none() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.spinner();
            ui.label(RichText::new("載入中…").color(p.text_weak));
        });
        return;
    }

    let Visible { pinned, people, channels } = view.visible_entries();
    if pick_first && let Some(first) = pinned.first().or(people.first()).or(channels.first()) {
        view.select(first.clone());
    }
    let searching = !view.query.trim().is_empty();
    let show_people = view.filter != Filter::Channels;
    let show_channels = view.filter != Filter::People;

    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        if !pinned.is_empty() {
            section_header(ui, "釘選", pinned.len(), p);
            for entry in &pinned {
                let selected = is_selected(view, entry);
                if entry_row(ui, entry, selected, &mut view.avatars, p).clicked() {
                    view.select(entry.clone());
                }
            }
            ui.add_space(10.0);
        }
        if show_people {
            section_header(ui, "人員", people.len(), p);
            if people.is_empty() {
                empty_hint(ui, "沒有符合的人員", p);
            }
            for entry in &people {
                let selected = is_selected(view, entry);
                if entry_row(ui, entry, selected, &mut view.avatars, p).clicked() {
                    view.select(entry.clone());
                }
            }
            if !searching && view.filter == Filter::People {
                empty_hint(ui, "其他人員請使用上方的搜尋", p);
            }
            ui.add_space(10.0);
        }
        if show_channels {
            section_header(ui, "頻道", channels.len(), p);
            if channels.is_empty() {
                empty_hint(ui, "沒有符合的頻道", p);
            }
            for entry in &channels {
                let selected = is_selected(view, entry);
                if entry_row(ui, entry, selected, &mut view.avatars, p).clicked() {
                    view.select(entry.clone());
                }
            }
        }
    });
}

fn is_selected(view: &MainView, entry: &Entry) -> bool {
    view.selected.as_ref().is_some_and(|s| s.target == entry.target)
}

fn section_header(ui: &mut egui::Ui, title: &str, count: usize, p: &Palette) {
    ui.add_space(4.0);
    ui.label(RichText::new(format!("{title}  {count}")).size(12.5).strong().color(p.text_weak));
}

fn empty_hint(ui: &mut egui::Ui, text: &str, p: &Palette) {
    ui.label(RichText::new(text).size(13.5).color(p.text_weak));
}

fn entry_row(ui: &mut egui::Ui, entry: &Entry, selected: bool, avatars: &mut Avatars, p: &Palette) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    // Custom-painted rows are invisible to screen readers unless described.
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &entry.title));
    if ui.is_rect_visible(rect) {
        let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), HOVER_ANIMATION);
        let fill = if selected { p.accent_soft } else { Color32::TRANSPARENT.lerp_to_gamma(p.hover, hover) };
        ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), fill);
        if selected {
            let bar = Rect::from_min_size(rect.min + vec2(0.0, 13.0), vec2(3.0, ROW_HEIGHT - 26.0));
            ui.painter().rect_filled(bar, CornerRadius::same(2), p.accent);
        }

        let avatar_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 12.0 + AVATAR_RADIUS, rect.center().y),
            Vec2::splat(AVATAR_RADIUS * 2.0),
        );
        paint_avatar(ui, avatars, avatar_rect, entry, p);

        let painter = ui.painter_at(rect);
        let text_x = avatar_rect.right() + 12.0;
        let max_width = rect.right() - 12.0 - text_x;
        let title = elided(&painter, &entry.title, FontId::proportional(15.0), p.text, max_width);
        if entry.subtitle.is_empty() {
            let y = rect.center().y - title.size().y / 2.0;
            painter.galley(Pos2::new(text_x, y), title, p.text);
        } else {
            let subtitle = elided(&painter, &entry.subtitle, FontId::proportional(12.5), p.text_weak, max_width);
            let gap = 2.0;
            let top = rect.center().y - (title.size().y + gap + subtitle.size().y) / 2.0;
            let subtitle_y = top + title.size().y + gap;
            painter.galley(Pos2::new(text_x, top), title, p.text);
            painter.galley(Pos2::new(text_x, subtitle_y), subtitle, p.text_weak);
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

fn composer(
    ui: &mut egui::Ui,
    view: &mut MainView,
    entry: &Entry,
    installation: &Installation,
    p: &Palette,
    action: &mut Action,
) {
    let mut close = false;
    let mut toggle_pin = false;
    let pinned = view.favorite_index(&entry.target);
    // Unpinning would silently break every script that uses the alias.
    let has_alias = view.alias_of(&entry.target).is_some();
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), ICON_BUTTON_SIZE),
        Layout::right_to_left(Align::Center),
        |ui| {
            if icon_button(ui, Icon::Close, "關閉（Esc）").clicked() {
                close = true;
            }
            let (icon, tip) = match (pinned, has_alias) {
                (None, _) => (Icon::Pin, "釘選到清單最上方"),
                (Some(_), false) => (Icon::Pinned, "取消釘選"),
                (Some(_), true) => (Icon::Pinned, "請先清除別名再取消釘選"),
            };
            let size = Vec2::splat(ICON_BUTTON_SIZE);
            if button(ui, ButtonKind::Ghost, Some(icon), "", size, !has_alias, Some(tip)).clicked() {
                toggle_pin = true;
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new("傳送給").color(p.text_weak));
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(SMALL_AVATAR_SIZE), Sense::hover());
                paint_avatar(ui, &mut view.avatars, rect, entry, p);
                ui.add(Label::new(RichText::new(&entry.title).strong().color(p.text)).truncate());
            });
        },
    );
    if close {
        view.selected = None;
        return;
    }
    if toggle_pin {
        match pinned {
            Some(index) => {
                view.favorites.remove(index);
            }
            None => view.favorites.push(entry.pin()),
        }
        view.alias_draft.clear();
        view.alias_error = None;
        if let Err(e) = favorites::save(&view.favorites) {
            *action = Action::ShowError(e);
        }
    }
    if let Some(index) = view.favorite_index(&entry.target) {
        alias_field(ui, view, index, p, action);
    }
    ui.add_space(4.0);

    let message_label = field_label(ui, "訊息", p);
    let message = ui
        .add(
            TextEdit::singleline(&mut view.message)
                .hint_text("例如：報表跑完了")
                .margin(MESSAGE_MARGIN)
                .desired_width(f32::INFINITY),
        )
        .labelled_by(message_label);
    if view.focus_message {
        view.focus_message = false;
        message.request_focus();
    }
    if message.changed() {
        view.copied_at = None;
        view.sent_at = None;
        view.send_error = None;
    }
    let can_send = !view.message.trim().is_empty() && !view.sending;
    let send_by_enter = message.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let now = Instant::now();
    let recently = |at: Option<Instant>| at.is_some_and(|t| now.duration_since(t) < FEEDBACK_DURATION);
    let (copied, sent) = (recently(view.copied_at), recently(view.sent_at));
    let send_clicked = send_button(ui, &message, view.sending, sent, can_send, p);
    if (send_clicked || send_by_enter) && can_send {
        *action = Action::Send {
            target: entry.target.clone(),
            message: view.message.trim().to_owned(),
        };
    }
    if let Some(error) = view.send_error.clone() {
        ui.add_space(2.0);
        error_box(ui, &error, None, p);
    }

    ui.add_space(6.0);
    if installation.state.commands_work() {
        if let InstallState::Newer(installed) = installation.state {
            let text = format!("已安裝較新的 v{installed}，請從開始功能表開啟");
            notice(ui, Icon::Download, &text, p.accent, p.accent_soft);
            ui.add_space(4.0);
        }
        command_section(ui, view, entry, copied, p);
    } else {
        install_card(ui, installation, p, action);
    }
    if copied || sent {
        ui.ctx().request_repaint_after(FEEDBACK_DURATION);
    }
}

/// The round send button inside the right end of the message field.
fn send_button(ui: &mut egui::Ui, field: &Response, sending: bool, sent: bool, enabled: bool, p: &Palette) -> bool {
    let rect = Rect::from_center_size(
        Pos2::new(field.rect.right() - SEND_BUTTON_INSET, field.rect.center().y),
        Vec2::splat(SEND_BUTTON_SIZE),
    );
    let label = "立即傳送（Enter）";
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui
        .interact(rect, ui.scope_id().with("send-now"), sense)
        .on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id, enabled && response.hovered(), HOVER_ANIMATION);
    let radius = rect.width() / 2.0;
    let glyph = rect.shrink(rect.width() * 0.27);
    if sending {
        egui::Spinner::new().color(p.accent).paint_at(ui, rect.shrink(5.0));
    } else if sent {
        ui.painter().circle_filled(rect.center(), radius, p.success);
        icons::paint(ui.painter(), Icon::Check, glyph, Color32::WHITE, p.success);
    } else if enabled {
        let fill = p.accent.lerp_to_gamma(p.accent_hover, hover);
        ui.painter().circle_filled(rect.center(), radius, fill);
        icons::paint(ui.painter(), Icon::Send, glyph, p.on_accent, fill);
    } else {
        icons::paint(ui.painter(), Icon::Send, glyph, p.text_weak.gamma_multiply(0.5), p.surface);
    }
    let clicked = enabled && response.clicked();
    if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand);
    }
    clicked
}

/// The alias of the selected pin. A valid alias is saved as it is typed, so the
/// command below always shows what scripts can use right now.
fn alias_field(ui: &mut egui::Ui, view: &mut MainView, index: usize, p: &Palette, action: &mut Action) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let label = field_label(ui, "別名", p);
        let field = ui
            .add(
                TextEdit::singleline(&mut view.alias_draft)
                    .hint_text("例如：值班")
                    .margin(INPUT_MARGIN)
                    .desired_width(ALIAS_FIELD_WIDTH),
            )
            .labelled_by(label);
        if field.changed() {
            let alias = view.alias_draft.trim().to_owned();
            let target = view.favorites[index].target.clone();
            view.alias_error = favorites::alias_problem(&alias, &view.favorites, &target);
            if view.alias_error.is_none() && view.favorites[index].alias != alias {
                view.favorites[index].alias = alias;
                view.copied_at = None;
                if let Err(e) = favorites::save(&view.favorites) {
                    *action = Action::ShowError(e);
                }
            }
        }
        if let Some(problem) = &view.alias_error {
            ui.add(Label::new(RichText::new(problem).size(12.5).color(p.danger)).wrap());
        }
    });
}

fn command_section(ui: &mut egui::Ui, view: &mut MainView, entry: &Entry, copied: bool, p: &Palette) {
    if segmented(ui, &mut view.template, &TEMPLATE_CHOICES, p) {
        view.copied_at = None;
    }
    let args = view
        .alias_of(&entry.target)
        .map(Target::alias_args)
        .unwrap_or_else(|| entry.target.cli_args());
    let command = target::command_line(install::COMMAND_NAME, &args, view.template, view.message.trim());
    Frame::default()
        .fill(p.code_background)
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(Label::new(RichText::new(&command).monospace().color(p.code_text)).wrap());
        });

    let blocked = view.template.quotes_message() && !Target::fits_inline(&view.message);
    if blocked {
        notice(ui, Icon::Warning, "訊息含有雙引號或換行，請改用「文字檔」", p.danger, p.danger_soft);
    }
    ui.add_space(4.0);
    let (icon, label) = if copied { (Icon::Check, "已複製") } else { (Icon::Copy, "複製指令") };
    let size = vec2(ui.available_width(), CONTROL_HEIGHT);
    if button(ui, ButtonKind::Primary, Some(icon), label, size, !blocked, None).clicked() {
        ui.ctx().copy_text(command);
        view.copied_at = Some(Instant::now());
    }
}

/// Stands in for the command until the exe is installed, which is what makes the
/// short `mmsend` command work.
fn install_card(ui: &mut egui::Ui, installation: &Installation, p: &Palette, action: &mut Action) {
    let (title, button_label) = match installation.state {
        InstallState::Outdated => ("更新後即可複製指令", format!("更新至 v{}", Version::current())),
        _ => ("安裝後即可複製指令", "安裝".to_owned()),
    };
    Frame::default()
        .fill(p.accent_soft)
        .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.25)))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (tile, _) = ui.allocate_exact_size(Vec2::splat(INSTALL_TILE_SIZE), Sense::hover());
                ui.painter().rect_filled(tile, CornerRadius::same(RADIUS), p.accent);
                icons::paint(ui.painter(), Icon::Download, tile.shrink(INSTALL_TILE_SIZE * 0.26), p.on_accent, p.accent);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(title).size(16.0).strong().color(p.text));
                    ui.label(RichText::new(INSTALL_LOCATION).small().color(p.text_weak));
                });
            });
            ui.add_space(10.0);
            benefit(ui, p, |ui| {
                ui.label(RichText::new("指令只需輸入").color(p.text));
                code_chip(ui, install::COMMAND_NAME, p);
            });
            benefit(ui, p, |ui| {
                ui.label(RichText::new("支援 cmd、PowerShell 與批次檔").color(p.text));
            });
            benefit(ui, p, |ui| {
                ui.label(RichText::new(format!("開始功能表新增「{}」", crate::APP_NAME)).color(p.text));
            });
            ui.add_space(12.0);
            let label = if installation.installing { "安裝中…" } else { &button_label };
            let size = vec2(ui.available_width(), CONTROL_HEIGHT);
            let enabled = !installation.installing;
            if button(ui, ButtonKind::Primary, Some(Icon::Download), label, size, enabled, None).clicked() {
                *action = Action::Install;
            }
        });
}

fn benefit(ui: &mut egui::Ui, p: &Palette, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        paint_icon(ui, Icon::Check, BENEFIT_ICON_SIZE, p.accent, p.accent_soft);
        add(ui);
    });
}

fn code_chip(ui: &mut egui::Ui, text: &str, p: &Palette) {
    Frame::default()
        .fill(p.code_background)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().size(CODE_CHIP_FONT_SIZE).color(p.code_text));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    fn login_fields_keep_the_ime_off() {
        let mut form = LoginForm {
            focus_pending: true, // focuses the (empty) URL field
            ..Default::default()
        };
        // CI machines may lack CJK fonts; the labels only need to lay out, not render.
        let mut harness = Harness::builder().allow_missing_glyphs().build_ui(|ui| {
            let logo = ui.ctx().load_texture("logo", ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
            login_ui(ui, &mut form, &logo, &theme::LIGHT);
        });
        harness.run();
        assert!(harness.output().platform_output.ime.is_none());
    }
}
