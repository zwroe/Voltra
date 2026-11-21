use std::{io, time::Duration};

use anyhow::{bail, ensure, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::watch,
    time::{sleep, timeout},
};
use uuid::Uuid;

const APPLICATION_ID: &str = "1573172497464033340";
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const RETRY_DELAY: Duration = Duration::from_secs(15);
const MAX_FRAME: usize = 64 * 1024;
const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

#[derive(Clone, PartialEq)]
pub struct Game {
    pub version: String,
    pub pack: String,
    pub started_at: u64,
    pub test: bool,
}

#[derive(Default)]
struct State {
    enabled: bool,
    show_pack: bool,
    game: Option<Game>,
}

impl State {
    fn activity(&self) -> Option<Value> {
        if !self.enabled {
            return None;
        }
        let mut activity = json!({
            "type": 0,
            "details": "Launcher open",
            "assets": { "large_image": "voltra_logo", "large_text": "Voltra" },
        });
        let Some(game) = &self.game else {
            return Some(activity);
        };
        let mode = if game.test { "Testing" } else { "Playing" };
        activity["details"] = json!(format!("{mode} Minecraft {}", game.version));
        activity["timestamps"] = json!({ "start": game.started_at });
        if self.show_pack {
            activity["state"] = json!(format!("Pack: {}", game.pack));
        }
        Some(activity)
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Off,
    WaitingForDiscord,
    Published,
    UpdateFailed,
}

pub struct Presence {
    state: watch::Sender<State>,
    status: watch::Receiver<Status>,
}

impl Presence {
    pub fn new() -> Self {
        let (state, updates) = watch::channel(State::default());
        let (status_updates, status) = watch::channel(Status::Off);
        tauri::async_runtime::spawn(run(updates, status_updates));
        Self { state, status }
    }

    pub fn status(&self) -> Status {
        *self.status.borrow()
    }

    pub fn configure(&self, enabled: bool, show_pack: bool) {
        self.state.send_if_modified(|state| {
            if state.enabled == enabled && state.show_pack == show_pack {
                return false;
            }
            state.enabled = enabled;
            state.show_pack = show_pack;
            true
        });
    }

    pub fn set_game(&self, game: Option<Game>) {
        self.state.send_if_modified(|state| {
            if state.game == game {
                return false;
            }
            state.game = game;
            true
        });
    }
}

struct Connection<T> {
    transport: T,
    buffer: Vec<u8>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> Connection<T> {
    fn new(transport: T) -> Self {
        Self {
            transport,
            buffer: Vec::new(),
        }
    }

    async fn send(&mut self, opcode: u32, payload: &[u8]) -> Result<()> {
        let mut packet = Vec::with_capacity(8 + payload.len());
        packet.extend_from_slice(&opcode.to_le_bytes());
        packet.extend_from_slice(&u32::try_from(payload.len())?.to_le_bytes());
        packet.extend_from_slice(payload);
        self.transport.write_all(&packet).await?;
        Ok(())
    }

    async fn frame(&mut self) -> Result<Value> {
        loop {
            while self.buffer.len() < 8 {
                self.read_more().await?;
            }
            let opcode = u32::from_le_bytes(self.buffer[0..4].try_into()?);
            let length = u32::from_le_bytes(self.buffer[4..8].try_into()?) as usize;
            ensure!(length <= MAX_FRAME, "Discord IPC frame is too large.");
            let end = 8 + length;
            while self.buffer.len() < end {
                self.read_more().await?;
            }
            let payload: Vec<_> = self.buffer.drain(..end).skip(8).collect();
            match opcode {
                FRAME => return Ok(serde_json::from_slice(&payload)?),
                CLOSE => bail!("Discord closed the IPC connection."),
                PING => timeout(IO_TIMEOUT, self.send(PONG, &payload)).await??,
                _ => {}
            }
        }
    }

    async fn read_more(&mut self) -> Result<()> {
        // Keep partially read frames when a presence change interrupts the idle read.
        let mut bytes = [0; 4096];
        let count = self.transport.read(&mut bytes).await?;
        ensure!(count != 0, "Discord disconnected.");
        self.buffer.extend_from_slice(&bytes[..count]);
        Ok(())
    }

    async fn handshake(&mut self) -> Result<()> {
        let payload = serde_json::to_vec(&json!({"v": 1, "client_id": APPLICATION_ID}))?;
        self.send(HANDSHAKE, &payload).await?;
        let response = self.frame().await?;
        ensure!(
            response["evt"] == "READY",
            "Discord rejected the Rich Presence handshake."
        );
        Ok(())
    }

    async fn update(&mut self, activity: Option<&Value>) -> Result<()> {
        let nonce = Uuid::new_v4().to_string();
        let payload = serde_json::to_vec(&json!({
            "cmd": "SET_ACTIVITY",
            "nonce": nonce,
            "args": { "pid": std::process::id(), "activity": activity },
        }))?;
        self.send(FRAME, &payload).await?;
        loop {
            let frame = self.frame().await?;
            if frame["nonce"] == nonce {
                ensure!(frame["evt"] != "ERROR", "Discord rejected the activity.");
                return Ok(());
            }
        }
    }
}

#[cfg(windows)]
async fn transport() -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;

    for index in 0..10 {
        if let Ok(pipe) = ClientOptions::new().open(format!(r"\\.\pipe\discord-ipc-{index}")) {
            return Ok(pipe);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Discord desktop IPC is unavailable",
    ))
}

#[cfg(unix)]
async fn transport() -> io::Result<tokio::net::UnixStream> {
    let directory = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|key| std::env::var_os(key).filter(|path| !path.is_empty()))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    for index in 0..10 {
        let socket = directory.join(format!("discord-ipc-{index}"));
        if let Ok(stream) = tokio::net::UnixStream::connect(socket).await {
            return Ok(stream);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Discord desktop IPC is unavailable",
    ))
}

async fn run(mut updates: watch::Receiver<State>, status: watch::Sender<Status>) {
    let mut connection = None;
    let mut published = None;
    loop {
        let activity = updates.borrow_and_update().activity();
        if activity != published {
            if let Some(activity) = &activity {
                let result = timeout(IO_TIMEOUT, async {
                    if connection.is_none() {
                        let pipe = transport().await.map_err(|_| Status::WaitingForDiscord)?;
                        let mut client = Connection::new(pipe);
                        client.handshake().await.map_err(|_| Status::UpdateFailed)?;
                        connection = Some(client);
                    }
                    connection
                        .as_mut()
                        .ok_or(Status::WaitingForDiscord)?
                        .update(Some(activity))
                        .await
                        .map_err(|_| Status::UpdateFailed)
                })
                .await
                .unwrap_or(Err(Status::UpdateFailed));
                match result {
                    Ok(()) => {
                        status.send_replace(Status::Published);
                    }
                    Err(problem) => {
                        status.send_replace(problem);
                        connection = None;
                        published = None;
                        if !retry(&mut updates).await {
                            break;
                        }
                        continue;
                    }
                }
            } else if let Some(mut client) = connection.take() {
                let _ = timeout(IO_TIMEOUT, client.update(None)).await;
            }
            published = activity;
        }
        if published.is_none() {
            status.send_replace(Status::Off);
        }

        let changed = if let Some(client) = &mut connection {
            tokio::select! {
                result = updates.changed() => result.is_ok(),
                result = client.frame() => {
                    if result.is_err() {
                        status.send_replace(Status::WaitingForDiscord);
                        connection = None;
                        published = None;
                        retry(&mut updates).await
                    } else {
                        true
                    }
                }
            }
        } else {
            updates.changed().await.is_ok()
        };
        if !changed {
            break;
        }
    }
}

async fn retry(updates: &mut watch::Receiver<State>) -> bool {
    tokio::select! {
        result = updates.changed() => result.is_ok(),
        _ = sleep(RETRY_DELAY) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Game {
        Game {
            version: "1.21.1".into(),
            pack: "Private pack".into(),
            started_at: 1700000000,
            test: true,
        }
    }

    #[test]
    fn activity_is_opt_in_and_pack_name_is_optional() {
        let mut state = State {
            game: Some(game()),
            ..State::default()
        };
        assert!(state.activity().is_none());
        state.enabled = true;
        let activity = state.activity().unwrap();
        assert_eq!(activity["details"], "Testing Minecraft 1.21.1");
        assert_eq!(activity["timestamps"]["start"], 1700000000);
        assert_eq!(activity["assets"]["large_image"], "voltra_logo");
        assert!(activity.get("state").is_none());
        state.show_pack = true;
        assert_eq!(state.activity().unwrap()["state"], "Pack: Private pack");
        state.game = None;
        let idle = state.activity().unwrap();
        assert_eq!(idle["details"], "Launcher open");
        assert!(idle.get("state").is_none());
        assert!(idle.get("timestamps").is_none());
        state.enabled = false;
        assert!(state.activity().is_none());
    }

    #[tokio::test]
    async fn discord_ipc_registers_updates_and_clears_activity() -> Result<()> {
        let (client, server) = tokio::io::duplex(2048);
        let mut client = Connection::new(client);
        let mut server = Connection::new(server);
        let idle = State {
            enabled: true,
            ..State::default()
        }
        .activity()
        .unwrap();
        let activity = State {
            enabled: true,
            show_pack: false,
            game: Some(game()),
        }
        .activity()
        .unwrap();
        let sender = async {
            client.handshake().await?;
            client.update(Some(&idle)).await?;
            client.update(Some(&activity)).await?;
            client.update(Some(&idle)).await?;
            client.update(None).await
        };
        let receiver = async {
            let mut header = [0; 8];
            server.transport.read_exact(&mut header).await?;
            assert_eq!(u32::from_le_bytes(header[0..4].try_into()?), HANDSHAKE);
            let mut handshake = vec![0; u32::from_le_bytes(header[4..8].try_into()?) as usize];
            server.transport.read_exact(&mut handshake).await?;
            assert_eq!(
                serde_json::from_slice::<Value>(&handshake)?["client_id"],
                APPLICATION_ID
            );
            server.send(FRAME, br#"{"evt":"READY"}"#).await?;

            for expected in [&idle, &activity, &idle, &Value::Null] {
                let request = server.frame().await?;
                assert_eq!(request["cmd"], "SET_ACTIVITY");
                assert_eq!(request["args"]["pid"], std::process::id());
                assert_eq!(&request["args"]["activity"], expected);
                let response = json!({ "cmd": "SET_ACTIVITY", "nonce": request["nonce"] });
                server.send(FRAME, &serde_json::to_vec(&response)?).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        timeout(IO_TIMEOUT, async { tokio::try_join!(sender, receiver) }).await??;
        Ok(())
    }
}
