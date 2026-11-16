//! Thin blocking client for the parts of the Mattermost REST API v4 we use.

use std::fmt;
use std::time::Duration;

use reqwest::blocking::{Client as HttpClient, RequestBuilder, Response};
use reqwest::{Method, StatusCode, Url};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};

const API_PATH: [&str; 2] = ["api", "v4"];
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Attachments can be large and the uplink slow.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// Mattermost returns the session token of a password login in this response header.
const SESSION_TOKEN_HEADER: &str = "Token";
const USER_SEARCH_LIMIT: u32 = 50;
/// Mattermost caps group messages at 8 members; 200 is the API's page maximum.
const GROUP_MEMBER_LIMIT: &str = "200";

pub const CHANNEL_OPEN: &str = "O";
pub const CHANNEL_PRIVATE: &str = "P";
pub const CHANNEL_DIRECT: &str = "D";
pub const CHANNEL_GROUP: &str = "G";

#[derive(Debug)]
pub enum ApiError {
    /// 401: wrong password on login, or an expired / revoked session token.
    Unauthorized,
    NotFound,
    Forbidden,
    Server { status: u16, message: String },
    Network(String),
    InvalidUrl(String),
}

impl ApiError {
    /// Worth trying again: the network dropped, or the server was briefly unable to
    /// answer (overloaded, restarting, behind a proxy that timed out).
    pub fn is_transient(&self) -> bool {
        const TRANSIENT_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];
        match self {
            ApiError::Network(_) => true,
            ApiError::Server { status, .. } => TRANSIENT_STATUSES.contains(status),
            _ => false,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "未授權（401）"),
            ApiError::NotFound => write!(f, "找不到（404）"),
            ApiError::Forbidden => write!(f, "無權限（403）"),
            ApiError::Server { status, message } => write!(f, "伺服器回應 {status}：{message}"),
            ApiError::Network(detail) => write!(f, "連線失敗：{detail}"),
            ApiError::InvalidUrl(reason) => write!(f, "{reason}"),
        }
    }
}

/// Accepts what people actually paste: a bare host, or a full browser URL such as
/// `https://chat.example.com/team/channels/town-square`. Only the origin is kept,
/// so Mattermost installs served under a sub-path are not supported.
///
/// Plain http is refused except for loopback (local test servers): the password
/// would otherwise travel in clear text.
pub fn normalize_server_url(input: &str) -> Result<Url, ApiError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ApiError::InvalidUrl("請輸入網址".into()));
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed =
        Url::parse(&with_scheme).map_err(|_| ApiError::InvalidUrl("網址格式錯誤".into()))?;
    match parsed.scheme() {
        "https" => {}
        "http" if is_loopback(parsed.host_str()) => {}
        _ => return Err(ApiError::InvalidUrl("網址須以 https:// 開頭".into())),
    }
    let origin = parsed.origin().ascii_serialization();
    Url::parse(&origin).map_err(|_| ApiError::InvalidUrl("網址格式錯誤".into()))
}

fn is_loopback(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "[::1]"))
}

#[derive(Deserialize, Clone, Debug)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub delete_at: i64,
}

impl User {
    /// The name shown next to `@username`; empty when the profile has none.
    /// Chinese, Japanese and Korean names read family name first, without a space.
    pub fn full_name(&self) -> String {
        let (first, last) = (self.first_name.trim(), self.last_name.trim());
        let full = if [first, last].iter().any(|s| s.chars().any(is_cjk)) {
            format!("{last}{first}")
        } else {
            format!("{first} {last}").trim().to_owned()
        };
        if full.is_empty() {
            self.nickname.clone()
        } else {
            full
        }
    }

    /// `@alice 王小明`: the handle commands take, and the name a person recognizes.
    /// Not in brackets: names often carry their own, as in `Amy (王小美)`.
    pub fn label(&self) -> String {
        match self.full_name() {
            name if name.is_empty() => format!("@{}", self.username),
            name => format!("@{} {name}", self.username),
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30ff}'   // kana
        | '\u{3400}'..='\u{9fff}' // CJK ideographs incl. extension A
        | '\u{ac00}'..='\u{d7af}' // hangul
    )
}

#[derive(Deserialize, Clone, Debug)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub display_name: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub delete_at: i64,
    /// Milliseconds since epoch; used to list recent conversations first.
    #[serde(default)]
    pub last_post_at: i64,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    login_id: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct UserSearchRequest<'a> {
    term: &'a str,
    limit: u32,
    allow_inactive: bool,
}

#[derive(Serialize)]
pub struct NewPost<'a> {
    pub channel_id: &'a str,
    pub message: &'a str,
    /// Mattermost drops a second post with the same pending id (for about 30 s), so a
    /// retry after a lost response cannot post the message twice.
    pub pending_post_id: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub file_ids: &'a [String],
}

#[derive(Deserialize)]
struct UploadResponse {
    file_infos: Vec<UploadedFile>,
}

#[derive(Deserialize)]
struct UploadedFile {
    id: String,
}

#[derive(Deserialize)]
struct ServerError {
    #[serde(default)]
    message: String,
}

/// Cheap to clone: the HTTP connection pool is shared.
#[derive(Clone)]
pub struct Client {
    http: HttpClient,
    server: Url,
    token: Option<String>,
}

impl Client {
    pub fn new(server: Url, token: Option<String>) -> Result<Self, ApiError> {
        let http = HttpClient::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("mmsend/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ApiError::Network(error_chain(&e)))?;
        Ok(Self {
            http,
            server,
            token,
        })
    }

    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }

    /// Password login. Returns the new session token and the logged-in user.
    pub fn login(&self, login_id: &str, password: &str) -> Result<(String, User), ApiError> {
        let response = self
            .request(Method::POST, &["users", "login"])
            .json(&LoginRequest { login_id, password })
            .send()
            .map_err(|e| ApiError::Network(error_chain(&e)))?;
        let response = check_status(response)?;
        let token = response
            .headers()
            .get(SESSION_TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Server {
                status: response.status().as_u16(),
                message: "登入回應中缺少 token".into(),
            })?;
        let user = parse_json(response)?;
        Ok((token, user))
    }

    /// Revokes the current session token on the server.
    pub fn logout(&self) -> Result<(), ApiError> {
        send_json::<IgnoredAny>(self.request(Method::POST, &["users", "logout"])).map(|_| ())
    }

    pub fn me(&self) -> Result<User, ApiError> {
        send_json(self.request(Method::GET, &["users", "me"]))
    }

    pub fn my_teams(&self) -> Result<Vec<Team>, ApiError> {
        send_json(self.request(Method::GET, &["users", "me", "teams"]))
    }

    /// Channels the user belongs to in a team, including direct and group messages.
    pub fn my_channels(&self, team_id: &str) -> Result<Vec<Channel>, ApiError> {
        send_json(self.request(Method::GET, &["users", "me", "teams", team_id, "channels"]))
    }

    pub fn users_by_ids(&self, ids: &[String]) -> Result<Vec<User>, ApiError> {
        send_json(self.request(Method::POST, &["users", "ids"]).json(ids))
    }

    pub fn search_users(&self, term: &str) -> Result<Vec<User>, ApiError> {
        send_json(
            self.request(Method::POST, &["users", "search"])
                .json(&UserSearchRequest {
                    term,
                    limit: USER_SEARCH_LIMIT,
                    allow_inactive: false,
                }),
        )
    }

    pub fn users_by_usernames(&self, usernames: &[String]) -> Result<Vec<User>, ApiError> {
        send_json(self.request(Method::POST, &["users", "usernames"]).json(usernames))
    }

    pub fn users_in_channel(&self, channel_id: &str) -> Result<Vec<User>, ApiError> {
        send_json(self.request_with_query(
            Method::GET,
            &["users"],
            &[("in_channel", channel_id), ("per_page", GROUP_MEMBER_LIMIT)],
        ))
    }

    /// Profile picture bytes (PNG or JPEG). Users without an upload get the
    /// server-generated initials image.
    pub fn user_image(&self, user_id: &str) -> Result<Vec<u8>, ApiError> {
        let response = self
            .request(Method::GET, &["users", user_id, "image"])
            .send()
            .map_err(|e| ApiError::Network(error_chain(&e)))?;
        let response = check_status(response)?;
        response
            .bytes()
            .map(|b| b.to_vec())
            .map_err(|e| ApiError::Network(error_chain(&e)))
    }

    pub fn user_by_username(&self, username: &str) -> Result<User, ApiError> {
        send_json(self.request(Method::GET, &["users", "username", username]))
    }

    pub fn channel_by_name(&self, team_name: &str, channel_name: &str) -> Result<Channel, ApiError> {
        send_json(self.request(
            Method::GET,
            &["teams", "name", team_name, "channels", "name", channel_name],
        ))
    }

    /// Returns the direct-message channel between two users, creating it on first use.
    pub fn direct_channel(&self, user_a: &str, user_b: &str) -> Result<Channel, ApiError> {
        send_json(self.request(Method::POST, &["channels", "direct"]).json(&[user_a, user_b]))
    }

    /// Returns the group-message channel for exactly these users (3–8, including
    /// yourself), creating it on first use.
    pub fn group_channel(&self, user_ids: &[String]) -> Result<Channel, ApiError> {
        send_json(self.request(Method::POST, &["channels", "group"]).json(user_ids))
    }

    /// Uploads one attachment for a post in `channel_id`; returns its file id.
    pub fn upload_file(&self, channel_id: &str, name: &str, bytes: &[u8]) -> Result<String, ApiError> {
        use reqwest::blocking::multipart::{Form, Part};
        // Raw UTF-8 file names: the default percent-encoding would show up literally
        // ("%E5%A0%B1...") as the attachment's name in Mattermost.
        let form = Form::new()
            .percent_encode_noop()
            .text("channel_id", channel_id.to_owned())
            .part("files", Part::bytes(bytes.to_vec()).file_name(name.to_owned()));
        let response: UploadResponse = send_json(
            self.request(Method::POST, &["files"])
                .timeout(UPLOAD_TIMEOUT)
                .multipart(form),
        )?;
        response
            .file_infos
            .into_iter()
            .next()
            .map(|file| file.id)
            .ok_or_else(|| ApiError::Server {
                status: 200,
                message: "上傳回應中缺少檔案資訊".into(),
            })
    }

    pub fn create_post(&self, post: &NewPost) -> Result<(), ApiError> {
        send_json::<IgnoredAny>(self.request(Method::POST, &["posts"]).json(post)).map(|_| ())
    }

    fn request(&self, method: Method, path: &[&str]) -> RequestBuilder {
        self.request_with_query(method, path, &[])
    }

    fn request_with_query(&self, method: Method, path: &[&str], query: &[(&str, &str)]) -> RequestBuilder {
        let mut url = self.server.clone();
        url.path_segments_mut()
            .expect("server URL is an http(s) origin")
            .clear()
            .extend(API_PATH)
            .extend(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let builder = self.http.request(method, url);
        match &self.token {
            Some(token) => builder.bearer_auth(token),
            None => builder,
        }
    }
}

fn send_json<T: DeserializeOwned>(builder: RequestBuilder) -> Result<T, ApiError> {
    let response = builder
        .send()
        .map_err(|e| ApiError::Network(error_chain(&e)))?;
    parse_json(check_status(response)?)
}

fn check_status(response: Response) -> Result<Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status {
        StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
        StatusCode::NOT_FOUND => Err(ApiError::NotFound),
        StatusCode::FORBIDDEN => Err(ApiError::Forbidden),
        _ => {
            let message = response
                .json::<ServerError>()
                .map(|e| e.message)
                .unwrap_or_default();
            Err(ApiError::Server {
                status: status.as_u16(),
                message,
            })
        }
    }
}

fn parse_json<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
    let status = response.status().as_u16();
    response.json::<T>().map_err(|e| ApiError::Server {
        status,
        message: format!("回應格式不符：{e}"),
    })
}

/// reqwest's top-level message ("error sending request") hides the useful part,
/// e.g. the TLS or DNS failure underneath.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_browser_url_keeps_only_the_origin() {
        let url = normalize_server_url("https://chat.example.com/team/channels/town-square").unwrap();
        assert_eq!(url.as_str(), "https://chat.example.com/");
    }

    #[test]
    fn bare_host_defaults_to_https() {
        let url = normalize_server_url("  chat.example.com:8443 ").unwrap();
        assert_eq!(url.as_str(), "https://chat.example.com:8443/");
    }

    #[test]
    fn plain_http_is_refused_except_for_loopback() {
        assert!(matches!(
            normalize_server_url("http://chat.example.com"),
            Err(ApiError::InvalidUrl(_))
        ));
        assert!(normalize_server_url("http://localhost:8065").is_ok());
    }
}
