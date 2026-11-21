use std::{sync::LazyLock, time::Duration};

use anyhow::{ensure, Context, Result};
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_LINES: usize = 25_000;

pub fn redact(text: &str) -> Result<String> {
    ensure!(
        text.len() <= MAX_BYTES,
        "The selected log is too large to preview."
    );
    static FILTERS: LazyLock<[(Regex, &str); 8]> = LazyLock::new(|| {
        [
        (Regex::new(r#"(?i)(\bauthorization["']?\s*[:=]\s*["']?(?:bearer|basic)\s+)[A-Za-z0-9+/=_.-]+"#).unwrap(), "${1}[REDACTED]"),
        (Regex::new(r#"(?i)(\b(?:access[_-]?token|refresh[_-]?token|client[_-]?secret|session[_-]?(?:token|id)|api[_-]?key|rcon\.password|password)\b["']?(?:\s*[:=]\s*|[ \t]+))(?:\[REDACTED\]|"[^"\r\n]*"|'[^'\r\n]*'|[^\s&,"';}\]]+)"#).unwrap(), "${1}[REDACTED]"),
        (Regex::new(r#"(?i)(https?://)[^@/\s]+:[^@/\s]+@"#).unwrap(), "${1}[REDACTED]@"),
        (Regex::new(r#"(?i)([a-z]:[\\/](?:users|documents and settings)[\\/])[^\\/"'\r\n]+"#).unwrap(), "${1}[user]"),
        (Regex::new(r#"(?i)(/(?:home|users)/)[^/"'\s]+"#).unwrap(), "${1}[user]"),
        (Regex::new(r#"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"#).unwrap(), "[email]"),
        (Regex::new(r#"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}:[0-9]{1,5}\b"#).unwrap(), "[address]"),
        (Regex::new(r#"(?im)(\b(?:ip|address|server-ip)\s*[:=]\s*["']?)(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b"#).unwrap(), "${1}[address]"),
    ]
    });
    let mut clean = text.to_owned();
    for (pattern, replacement) in FILTERS.iter() {
        clean = pattern.replace_all(&clean, *replacement).into_owned();
    }
    Ok(clean.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n"))
}

#[derive(Deserialize)]
struct Upload {
    success: bool,
    url: Option<String>,
    error: Option<String>,
}

fn share_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str() == Some("mclo.gs")
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path()[1..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
            && url.path().len() > 1,
        "mclo.gs returned an invalid share link."
    );
    Ok(url)
}

pub async fn upload(content: String) -> Result<String> {
    ensure!(
        !content.trim().is_empty(),
        "There is no log content to share."
    );
    ensure!(
        content.len() <= MAX_BYTES && content.lines().count() <= MAX_LINES,
        "Shorten the log before uploading it."
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client
        .post("https://api.mclo.gs/1/log")
        .json(&serde_json::json!({ "content": content, "source": "Voltra" }))
        .send()
        .await
        .context("Connecting to mclo.gs")?;
    ensure!(
        response.status().is_success(),
        "mclo.gs returned HTTP {}.",
        response.status()
    );
    let mut payload = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            payload.len() + chunk.len() <= 16 * 1024,
            "mclo.gs returned an unexpected response."
        );
        payload.extend_from_slice(&chunk);
    }
    let data: Upload = serde_json::from_slice(&payload).context("Reading the mclo.gs response")?;
    ensure!(
        data.success,
        "mclo.gs could not share this log: {}",
        data.error.as_deref().unwrap_or("unknown error")
    );
    let url = share_url(
        data.url
            .as_deref()
            .context("mclo.gs did not return a share link.")?,
    )?;
    Ok(url.to_string())
}

pub fn open(value: &str) -> Result<()> {
    open::that(share_url(value)?.as_str()).context("Opening the mclo.gs log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_removes_private_values_and_preserves_the_error() -> Result<()> {
        let log = r#"Missing dependency: fabric-api 1.2.3
C:\Users\Jane Doe\Voltra\latest.log /home/jane/.minecraft
"accessToken": "secret-token", refresh_token=refresh-secret
Authorization: Bearer abcd1234
email user@example.org logged in from /192.168.1.10:25566
server-ip=192.168.1.10
https://test:password@example.net/log"#;
        let result = redact(log)?;
        for secret in [
            "Jane Doe",
            "/home/jane",
            "secret-token",
            "refresh-secret",
            "abcd1234",
            "user@example.org",
            "192.168.1.10",
            "test:password",
        ] {
            assert!(!result.contains(secret), "{secret} remains in {result}");
        }
        assert!(result.contains("Missing dependency: fabric-api 1.2.3"));
        assert!(result.contains(r"C:\Users\[user]\Voltra\latest.log"));
        assert_eq!(redact(&result)?, result);
        Ok(())
    }

    #[test]
    fn only_public_mclogs_paste_links_can_be_opened() {
        assert!(share_url("https://mclo.gs/Abc1234").is_ok());
        assert!(share_url("https://mclo.gs.evil.com/Abc1234").is_err());
        assert!(share_url("https://api.mclo.gs/1/log").is_err());
        assert!(share_url("https://mclo.gs/").is_err());
        assert!(share_url("https://private@mclo.gs/Abc1234").is_err());
    }
}
