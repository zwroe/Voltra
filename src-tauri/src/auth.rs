use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use keyring::Entry;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::storage::{atomic_json, read_default};

const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const MC: &str = "https://api.minecraftservices.com";
const SCOPE: &str = "XboxLive.signin offline_access";

pub fn launcher_client_id() -> Result<String> {
    std::env::var("VOLTRA_MICROSOFT_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .as_deref()
        .or(option_env!("VOLTRA_MICROSOFT_CLIENT_ID"))
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
        .map(|id| id.to_string())
        .context("Microsoft sign-in is unavailable in this build. Voltra needs its approved Minecraft application registration.")
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub skin_url: Option<String>,
    #[serde(default)]
    pub skin_variant: Option<crate::media::Variant>,
    #[serde(default)]
    pub skin_choice: Option<String>,
    #[serde(default)]
    client_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSkin {
    png: String,
    variant: crate::media::Variant,
    height: u32,
}

impl Account {
    pub fn client_id(&self) -> &str {
        &self.client_id
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Accounts {
    pub active_id: Option<Uuid>,
    pub accounts: Vec<Account>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cape {
    id: Uuid,
    name: String,
    url: String,
    active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSignIn {
    pub id: Uuid,
}

struct Pending {
    url: reqwest::Url,
    listener: Option<tokio::net::TcpListener>,
    client_id: String,
    verifier: String,
    state: String,
    redirect: String,
    expires: Instant,
    cancel: CancellationToken,
}

#[derive(Deserialize)]
struct MicrosoftToken {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Clone)]
pub struct Session {
    pub account: Account,
    pub token: String,
    pub xuid: String,
    expires: Instant,
}

pub struct Auth {
    client: Client,
    path: PathBuf,
    gate: Mutex<()>,
    pending: Mutex<HashMap<Uuid, Pending>>,
    sessions: Mutex<HashMap<Uuid, Session>>,
}

impl Auth {
    pub fn new(client: Client, path: PathBuf) -> Self {
        Self {
            client,
            path,
            gate: Mutex::new(()),
            pending: Mutex::default(),
            sessions: Mutex::default(),
        }
    }

    pub async fn accounts(&self) -> Result<Accounts> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || read_default(&path)).await?
    }

    pub async fn has_profile(&self) -> Result<()> {
        let accounts = self.accounts().await?;
        ensure!(
            accounts
                .accounts
                .iter()
                .any(|account| Some(account.id) == accounts.active_id),
            "Connect a Microsoft account with Minecraft Java Edition before playing."
        );
        Ok(())
    }

    async fn save(&self, accounts: Accounts) -> Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || atomic_json(&path, &accounts)).await?
    }

    pub async fn begin(&self, client_id: String) -> Result<BrowserSignIn> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("Starting the local Microsoft sign-in callback")?;
        let redirect = format!(
            "http://localhost:{}/oauth/callback",
            listener.local_addr()?.port()
        );
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = Uuid::new_v4().simple().to_string();
        let mut url = reqwest::Url::parse(AUTHORIZE_URL)?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("response_mode", "query"),
            ("scope", SCOPE),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ]);

        let id = Uuid::new_v4();
        let mut pending = self.pending.lock().await;
        for entry in pending.values() {
            entry.cancel.cancel();
        }
        pending.clear();
        pending.insert(
            id,
            Pending {
                url,
                listener: Some(listener),
                client_id,
                verifier,
                state,
                redirect,
                expires: Instant::now() + Duration::from_secs(300),
                cancel: CancellationToken::new(),
            },
        );
        Ok(BrowserSignIn { id })
    }

    pub async fn open_browser(&self, id: Uuid) -> Result<()> {
        let url = self
            .pending
            .lock()
            .await
            .get(&id)
            .context("Start Microsoft sign-in again.")?
            .url
            .to_string();
        tokio::task::spawn_blocking(move || open::that(url)).await??;
        Ok(())
    }

    pub async fn cancel(&self, id: Uuid) {
        if let Some(entry) = self.pending.lock().await.remove(&id) {
            entry.cancel.cancel();
        }
    }

    pub async fn finish(&self, id: Uuid) -> Result<Accounts> {
        let (listener, client_id, verifier, state, redirect, expires, cancel) = {
            let mut pending = self.pending.lock().await;
            let entry = pending
                .get_mut(&id)
                .context("Start Microsoft sign-in again.")?;
            let listener = entry
                .listener
                .take()
                .context("This sign-in is already in progress.")?;
            (
                listener,
                entry.client_id.clone(),
                entry.verifier.clone(),
                entry.state.clone(),
                entry.redirect.clone(),
                entry.expires,
                entry.cancel.clone(),
            )
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(anyhow::anyhow!("Sign-in cancelled.")),
            result = self.browser_session(listener, &client_id, &verifier, &state, &redirect, expires) => result,
        };
        let (token, session) = match result {
            Ok(result) => result,
            Err(error) => {
                self.cancel(id).await;
                return Err(error);
            }
        };
        let _guard = self.gate.lock().await;
        ensure!(
            self.pending.lock().await.remove(&id).is_some() && !cancel.is_cancelled(),
            "Sign-in cancelled."
        );
        let refresh = token.refresh_token.context(
            "Microsoft did not return a refresh token. Sign in again with offline access enabled.",
        )?;
        let mut accounts = self.accounts().await?;
        vault_write(&session.account, refresh).await?;
        accounts
            .accounts
            .retain(|account| account.id != session.account.id);
        accounts.accounts.push(session.account.clone());
        accounts.active_id = Some(session.account.id);
        self.save(accounts.clone()).await?;
        self.sessions
            .lock()
            .await
            .insert(session.account.id, session);
        Ok(accounts)
    }

    async fn browser_session(
        &self,
        listener: tokio::net::TcpListener,
        client_id: &str,
        verifier: &str,
        state: &str,
        redirect: &str,
        expires: Instant,
    ) -> Result<(MicrosoftToken, Session)> {
        let code = tokio::time::timeout_at(expires.into(), receive_callback(listener, state))
            .await
            .context("Microsoft sign-in expired. Open the browser again.")??;
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("client_id", client_id),
                ("redirect_uri", redirect),
                ("code_verifier", verifier),
                ("scope", SCOPE),
            ])
            .send()
            .await
            .context("Completing Microsoft sign-in")?;
        ensure!(
            response.status().is_success(),
            "Microsoft rejected the authorization code ({}).",
            response.status()
        );
        let token: MicrosoftToken = response.json().await?;
        let session = self
            .exchange(&token.access_token, client_id.to_string())
            .await?;
        Ok((token, session))
    }

    pub async fn select(&self, id: Uuid) -> Result<Accounts> {
        let _guard = self.gate.lock().await;
        let mut accounts = self.accounts().await?;
        ensure!(
            accounts.accounts.iter().any(|account| account.id == id),
            "Account not found."
        );
        accounts.active_id = Some(id);
        self.save(accounts.clone()).await?;
        Ok(accounts)
    }

    pub async fn remove(&self, id: Uuid) -> Result<Accounts> {
        let _guard = self.gate.lock().await;
        let mut accounts = self.accounts().await?;
        let account = accounts
            .accounts
            .iter()
            .find(|account| account.id == id)
            .context("Account not found.")?;
        let entry = vault_entry(account)?;
        tokio::task::spawn_blocking(move || match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error),
        })
        .await?
        .context("Removing the account from the system credential store")?;
        self.sessions.lock().await.remove(&id);
        accounts.accounts.retain(|account| account.id != id);
        if accounts.active_id == Some(id) {
            accounts.active_id = accounts.accounts.first().map(|account| account.id);
        }
        self.save(accounts.clone()).await?;
        Ok(accounts)
    }

    pub async fn session(&self) -> Result<Session> {
        let _guard = self.gate.lock().await;
        let accounts = self.accounts().await?;
        let account = accounts
            .accounts
            .into_iter()
            .find(|account| Some(account.id) == accounts.active_id)
            .context("Connect a Microsoft account with Minecraft Java Edition before playing.")?;
        if let Some(session) = self.sessions.lock().await.get(&account.id) {
            if session.expires > Instant::now() + Duration::from_secs(60) {
                return Ok(session.clone());
            }
        }
        let entry = vault_entry(&account)?;
        let refresh = tokio::task::spawn_blocking(move || entry.get_password()).await?
            .context("The saved Microsoft credential is missing or inaccessible. Sign in again from Settings.")?;
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
                ("client_id", account.client_id.as_str()),
                ("scope", SCOPE),
            ])
            .send()
            .await
            .context("Refreshing Microsoft sign-in")?;
        ensure!(
            response.status().is_success(),
            "Microsoft rejected the saved session ({}). Sign in again from Settings.",
            response.status()
        );
        let token: MicrosoftToken = response.json().await?;
        if let Some(refresh) = token.refresh_token {
            vault_write(&account, refresh).await?;
        }
        let session = self
            .exchange(&token.access_token, account.client_id)
            .await?;
        ensure!(
            session.account.id == account.id,
            "The Microsoft session returned a different Minecraft profile."
        );
        self.sessions
            .lock()
            .await
            .insert(account.id, session.clone());
        Ok(session)
    }

    pub async fn apply_skin(&self, skin: crate::media::Skin, bytes: Vec<u8>) -> Result<Accounts> {
        let session = self.session().await?;
        let _guard = self.gate.lock().await;
        let mut accounts = self.accounts().await?;
        ensure!(
            accounts.active_id == Some(session.account.id),
            "The active account changed. Try again."
        );
        let boundary = format!("Voltra{}", Uuid::new_v4().simple());
        let head = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"variant\"\r\n\r\n{}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"skin.png\"\r\nContent-Type: image/png\r\n\r\n", skin.variant.as_str());
        let tail = format!("\r\n--{boundary}--\r\n");
        let mut body = head.into_bytes();
        body.extend(bytes);
        body.extend(tail.as_bytes());
        let profile: Value = self
            .client
            .post(format!("{MC}/minecraft/profile/skins"))
            .bearer_auth(&session.token)
            .header(
                "Content-Type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await?
            .error_for_status()
            .context("Minecraft rejected the skin upload")?
            .json()
            .await?;
        let account = accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == session.account.id)
            .context("Account not found.")?;
        account.skin_url = profile_skin(&profile);
        account.skin_variant = Some(skin.variant);
        account.skin_choice = Some(skin.id);
        self.save(accounts.clone()).await?;
        Ok(accounts)
    }

    pub async fn capes(&self, account_id: Uuid) -> Result<Vec<Cape>> {
        let session = self.session().await?;
        ensure!(
            session.account.id == account_id,
            "The active account changed. Try again."
        );
        self.account_capes(&session).await
    }

    pub async fn current_skin(&self, account_id: Uuid) -> Result<Option<AccountSkin>> {
        let session = self.session().await?;
        ensure!(session.account.id == account_id, "The active account changed. Try again.");
        let profile = self.client.get(format!("{MC}/minecraft/profile"))
            .bearer_auth(&session.token).send().await?.error_for_status()?.json::<Value>().await?;
        let Some(url) = profile_skin(&profile) else { return Ok(None) };
        let mut response = self.client.get(url).send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(bytes.len() + chunk.len() <= 1024 * 1024, "Minecraft returned an oversized skin.");
            bytes.extend(chunk);
        }
        Ok(Some(AccountSkin {
            height: crate::media::skin_height(&bytes)?,
            variant: profile_skin_variant(&profile).unwrap_or(crate::media::Variant::Classic),
            png: STANDARD.encode(bytes),
        }))
    }

    pub async fn set_cape(&self, account_id: Uuid, cape_id: Option<Uuid>) -> Result<Vec<Cape>> {
        let session = self.session().await?;
        let _guard = self.gate.lock().await;
        ensure!(
            session.account.id == account_id
                && self.accounts().await?.active_id == Some(account_id),
            "The active account changed. Try again."
        );
        let endpoint = format!("{MC}/minecraft/profile/capes/active");
        let request = match cape_id {
            Some(id) => self.client.put(endpoint).json(&json!({ "capeId": id })),
            None => self.client.delete(endpoint),
        };
        request
            .bearer_auth(&session.token)
            .send()
            .await?
            .error_for_status()
            .context("Minecraft could not change the active cape")?;
        self.account_capes(&session).await
    }

    async fn account_capes(&self, session: &Session) -> Result<Vec<Cape>> {
        let profile = self
            .client
            .get(format!("{MC}/minecraft/profile"))
            .bearer_auth(&session.token)
            .send()
            .await?
            .error_for_status()
            .context("Minecraft could not load the account's capes")?
            .json::<Value>()
            .await?;
        Ok(profile_capes(&profile))
    }

    async fn exchange(&self, microsoft: &str, client_id: String) -> Result<Session> {
        let xbl = self.client.post("https://user.auth.xboxlive.com/user/authenticate").json(&json!({
            "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={microsoft}") },
            "RelyingParty": "http://auth.xboxlive.com", "TokenType": "JWT"
        })).send().await?.error_for_status().context("Xbox Live authentication")?.json::<Value>().await?;
        let user_token = xbl["Token"]
            .as_str()
            .context("Xbox Live returned no user token.")?;
        let response = self
            .client
            .post("https://xsts.auth.xboxlive.com/xsts/authorize")
            .json(&json!({
                "Properties": { "SandboxId": "RETAIL", "UserTokens": [user_token] },
                "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT"
            }))
            .send()
            .await?;
        let status = response.status();
        let xsts: Value = response.json().await?;
        if !status.is_success() {
            match xsts["XErr"].as_u64() {
                Some(2148916233) => bail!("Create an Xbox profile for this Microsoft account before signing in."),
                Some(2148916238) => bail!("This Xbox account needs permission from its Microsoft family organizer."),
                Some(code) => bail!("Xbox account authorization returned error {code}. Check your Xbox account settings."),
                None => bail!("Xbox account authorization failed ({status})."),
            }
        }
        let claims = &xsts["DisplayClaims"]["xui"][0];
        let hash = claims["uhs"]
            .as_str()
            .context("Xbox returned no user hash.")?;
        let xuid = claims["xid"].as_str().unwrap_or_default().to_owned();
        let token = xsts["Token"]
            .as_str()
            .context("Xbox returned no authorization token.")?;
        let response = self
            .client
            .post(format!("{MC}/authentication/login_with_xbox"))
            .json(&json!({ "identityToken": format!("XBL3.0 x={hash};{token}") }))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            bail!("Minecraft Services rejected this build’s client ID. Install a Voltra build configured for Minecraft sign-in.");
        }
        let result: Value = response
            .error_for_status()
            .context("Minecraft authentication")?
            .json()
            .await?;
        let token = result["access_token"]
            .as_str()
            .context("Minecraft returned no access token.")?
            .to_owned();
        let expires = Instant::now()
            + Duration::from_secs(result["expires_in"].as_u64().unwrap_or(3600).min(86400));
        // A Java profile is required even for accounts whose entitlement is provided by Game Pass.
        let profile = self
            .client
            .get(format!("{MC}/minecraft/profile"))
            .bearer_auth(&token)
            .send()
            .await?;
        if profile.status() == reqwest::StatusCode::NOT_FOUND {
            bail!("No Minecraft Java Edition profile was found. Check ownership and finish creating your profile in the official launcher.");
        }
        let profile: Value = profile
            .error_for_status()
            .context("Checking the Minecraft Java Edition profile")?
            .json()
            .await?;
        let id = Uuid::parse_str(
            profile["id"]
                .as_str()
                .context("Minecraft returned no profile ID.")?,
        )?;
        let name = profile["name"]
            .as_str()
            .context("Minecraft returned no profile name.")?
            .to_owned();
        Ok(Session {
            account: Account {
                id,
                name,
                skin_url: profile_skin(&profile),
                skin_variant: profile_skin_variant(&profile),
                skin_choice: None,
                client_id,
            },
            token,
            xuid,
            expires,
        })
    }
}

fn profile_skin(profile: &Value) -> Option<String> {
    let value = profile["skins"]
        .as_array()?
        .iter()
        .find(|skin| skin["state"] == "ACTIVE")?["url"]
        .as_str()?;
    texture_url(value)
}

fn profile_skin_variant(profile: &Value) -> Option<crate::media::Variant> {
    let skin = profile["skins"].as_array()?.iter().find(|skin| skin["state"] == "ACTIVE")?;
    match skin["variant"].as_str()?.to_ascii_lowercase().as_str() {
        "classic" => Some(crate::media::Variant::Classic),
        "slim" => Some(crate::media::Variant::Slim),
        _ => None,
    }
}

fn profile_capes(profile: &Value) -> Vec<Cape> {
    profile["capes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|cape| {
            Some(Cape {
                id: Uuid::parse_str(cape["id"].as_str()?).ok()?,
                name: cape["alias"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Minecraft cape")
                    .to_owned(),
                url: texture_url(cape["url"].as_str()?)?,
                active: cape["state"] == "ACTIVE",
            })
        })
        .collect()
}

fn texture_url(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str() != Some("textures.minecraft.net")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !url.path().starts_with("/texture/")
    {
        return None;
    }
    url.set_scheme("https").ok()?;
    Some(url.to_string())
}

fn callback_code(line: &[u8], expected: &str) -> Option<Result<String>> {
    let target = std::str::from_utf8(line)
        .ok()?
        .strip_prefix("GET ")?
        .split_ascii_whitespace()
        .next()?;
    target.strip_prefix("/oauth/callback?")?;
    let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok()?;
    let mut states = url.query_pairs().filter(|(key, _)| key == "state");
    if states.next()?.1 != expected || states.next().is_some() {
        return None;
    }
    let mut codes = url.query_pairs().filter(|(key, _)| key == "code");
    let code = codes.next().map(|(_, value)| value);
    Some(
        code.filter(|code| !code.is_empty() && codes.next().is_none())
            .map(|code| code.into_owned())
            .context("Microsoft sign-in was declined or rejected."),
    )
}

async fn receive_callback(listener: tokio::net::TcpListener, expected: &str) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    loop {
        let (mut stream, _) = listener.accept().await?;
        let request = tokio::time::timeout(Duration::from_secs(3), async {
            use tokio::io::AsyncReadExt;
            let mut line = Vec::new();
            BufReader::new(&mut stream)
                .take(8192)
                .read_until(b'\n', &mut line)
                .await?;
            Ok::<_, std::io::Error>(line)
        })
        .await;
        let callback = request
            .ok()
            .and_then(std::result::Result::ok)
            .and_then(|line| callback_code(&line, expected));
        let (status, body) = match &callback {
            Some(Ok(_)) => (
                "200 OK",
                "Microsoft sign-in received. You can return to Voltra.",
            ),
            Some(Err(_)) => (
                "400 Bad Request",
                "Microsoft sign-in was declined. Return to Voltra.",
            ),
            None => (
                "400 Bad Request",
                "This request does not match the active Voltra sign-in.",
            ),
        };
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        if let Some(callback) = callback {
            return callback;
        }
    }
}

fn vault_entry(account: &Account) -> Result<Entry> {
    Entry::new(
        "app.voltra.launcher",
        &format!("microsoft:{}:{}", account.client_id, account.id),
    )
    .context("Opening the system credential store")
}

async fn vault_write(account: &Account, refresh: String) -> Result<()> {
    let entry = vault_entry(account)?;
    tokio::task::spawn_blocking(move || entry.set_password(&refresh))
        .await?
        .context("Saving the Microsoft refresh token in the system credential store")
}

#[cfg(test)]
mod tests {
    use super::callback_code;

    #[test]
    fn accepts_a_callback_only_with_one_matching_state() {
        let request = b"GET /oauth/callback?state=valid&code=ab%2Bcd HTTP/1.1\r\n";
        assert_eq!(callback_code(request, "valid").unwrap().unwrap(), "ab+cd");
        assert!(callback_code(request, "unrelated").is_none());
        let repeated = b"GET /oauth/callback?state=no&state=valid&code=abc HTTP/1.1\r\n";
        assert!(callback_code(repeated, "valid").is_none());
        let wrong_route = b"GET /oauth/other?state=valid&code=abc HTTP/1.1\r\n";
        assert!(callback_code(wrong_route, "valid").is_none());
    }

    #[test]
    fn reports_a_decline_only_for_the_active_sign_in() {
        let request = b"GET /oauth/callback?state=valid&error=access_denied HTTP/1.1\r\n";
        assert!(callback_code(request, "valid").unwrap().is_err());
        assert!(callback_code(request, "unrelated").is_none());
    }
}
