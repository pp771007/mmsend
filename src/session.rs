//! A logged-in account: API client plus the stored credentials that keep it alive.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::{self, ApiError, Client};
use crate::store::{self, Config, Secrets};

#[derive(Debug)]
pub enum Error {
    NotLoggedIn,
    Api(ApiError),
    Store(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotLoggedIn => write!(f, "尚未登入"),
            Error::Api(e) => write!(f, "{e}"),
            Error::Store(e) => write!(f, "{e}"),
        }
    }
}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Error::Api(e)
    }
}

pub struct Session {
    client: Client,
    pub config: Config,
    secrets: Secrets,
}

impl Session {
    pub fn login(server_input: &str, login_id: &str, password: &str) -> Result<Session, Error> {
        let server = api::normalize_server_url(server_input)?;
        let mut client = Client::new(server.clone(), None)?;
        let (token, user) = client.login(login_id.trim(), password)?;
        client.set_token(token.clone());
        let config = Config {
            server_url: server.to_string(),
            login_id: login_id.trim().to_owned(),
            username: user.username,
            user_id: user.id,
        };
        let secrets = Secrets {
            password: password.to_owned(),
            token,
        };
        store::save_config(&config).map_err(Error::Store)?;
        store::save_secrets(&secrets).map_err(Error::Store)?;
        Ok(Session {
            client,
            config,
            secrets,
        })
    }

    /// Restores the saved session without touching the network.
    pub fn load() -> Result<Session, Error> {
        let config = store::load_config().map_err(Error::Store)?;
        let secrets = store::load_secrets().map_err(Error::Store)?;
        let (Some(config), Some(secrets)) = (config, secrets) else {
            return Err(Error::NotLoggedIn);
        };
        let server = api::normalize_server_url(&config.server_url)?;
        let client = Client::new(server, Some(secrets.token.clone()))?;
        Ok(Session {
            client,
            config,
            secrets,
        })
    }

    /// Runs an API call; if the session token has expired, logs in again with the
    /// stored password and retries once.
    pub fn call<T>(&mut self, request: impl Fn(&Client) -> Result<T, ApiError>) -> Result<T, Error> {
        match request(&self.client) {
            Err(ApiError::Unauthorized) => {
                self.relogin()?;
                Ok(request(&self.client)?)
            }
            result => Ok(result?),
        }
    }

    fn relogin(&mut self) -> Result<(), Error> {
        let (token, _) = self
            .client
            .login(&self.config.login_id, &self.secrets.password)?;
        self.client.set_token(token.clone());
        self.secrets.token = token;
        store::save_secrets(&self.secrets).map_err(Error::Store)
    }

    /// A fresh pending post id, in the form Mattermost's own apps use
    /// (`user_id:milliseconds`). Reuse it when retrying the same post.
    pub fn new_pending_post_id(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        format!("{}:{millis}", self.config.user_id)
    }

    /// A detached copy of the client for background work (profile pictures) that
    /// should not hold the session lock. It does not see later token renewals.
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Revokes the session token on the server. Clearing the local secrets is the
    /// caller's job (`store::clear_secrets`) so it never waits on the network.
    pub fn revoke_token(&self) -> Result<(), ApiError> {
        self.client.logout()
    }
}
