use std::{net::SocketAddr, path::Path, time::Duration};

use anyhow::{anyhow, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

use crate::downloads::Job;

const PREFIX: &str = "voltra1.";
const BLOCK: usize = 64 * 1024;
const TAG: usize = 16;
const MAX_ARCHIVE: u64 = 2 * 1024 * 1024 * 1024;
const IO_LIMIT: Duration = Duration::from_secs(20);

#[derive(Serialize, Deserialize)]
pub struct Invite {
    address: SocketAddr,
    key: [u8; 32],
}

impl Invite {
    pub async fn listen(address: SocketAddr) -> Result<(Self, TcpListener)> {
        validate(address)?;
        let bind = if address.is_ipv4() { "0.0.0.0" } else { "::" };
        let listener = TcpListener::bind((bind, address.port()))
            .await
            .context("Opening the share port. Choose a different port if it's in use.")?;
        Ok((
            Self {
                address,
                key: random()?,
            },
            listener,
        ))
    }

    pub fn code(&self) -> Result<String> {
        Ok(format!(
            "{PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        ))
    }

    pub fn parse(code: &str) -> Result<Self> {
        let code = code.trim();
        ensure!(code.len() <= 512, "That share code is too long.");
        let payload = code
            .strip_prefix(PREFIX)
            .context("Paste a Voltra instance share code.")?;
        let invite: Self = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(payload)
                .context("Invalid share code")?,
        )
        .context("Invalid share code")?;
        validate(invite.address)?;
        Ok(invite)
    }

    fn auth(&self) -> hmac::Key {
        hmac::Key::new(hmac::HMAC_SHA256, &self.key)
    }

    fn cipher(&self) -> Result<aead::LessSafeKey> {
        let key = hmac::sign(&self.auth(), b"Voltra instance encryption v1");
        Ok(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_ref())
                .map_err(|_| anyhow!("Could not initialize share encryption."))?,
        ))
    }
}

fn validate(address: SocketAddr) -> Result<()> {
    ensure!(
        address.port() != 0
            && !address.ip().is_unspecified()
            && !address.ip().is_multicast()
            && address.ip() != std::net::Ipv4Addr::BROADCAST,
        "Enter a reachable IP address and a port from 1 to 65535."
    );
    Ok(())
}

fn random() -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("Could not generate a secure share key."))?;
    Ok(bytes)
}

fn nonce(index: u64) -> aead::Nonce {
    let mut bytes = [0; 12];
    bytes[4..].copy_from_slice(&index.to_be_bytes());
    aead::Nonce::assume_unique_for_key(bytes)
}

async fn send_block(
    stream: &mut TcpStream,
    cipher: &aead::LessSafeKey,
    index: u64,
    bytes: &[u8],
) -> Result<()> {
    let mut sealed = bytes.to_vec();
    cipher
        .seal_in_place_append_tag(nonce(index), aead::Aad::empty(), &mut sealed)
        .map_err(|_| anyhow!("Could not encrypt the instance."))?;
    timeout(IO_LIMIT, stream.write_all(&sealed))
        .await
        .context("Recipient stopped receiving")??;
    Ok(())
}

async fn receive_block<'a>(
    stream: &mut TcpStream,
    cipher: &aead::LessSafeKey,
    index: u64,
    buffer: &'a mut [u8],
) -> Result<&'a [u8]> {
    timeout(IO_LIMIT, stream.read_exact(buffer))
        .await
        .context("The host stopped responding")??;
    cipher
        .open_in_place(nonce(index), aead::Aad::empty(), buffer)
        .map(|bytes| &*bytes)
        .map_err(|_| anyhow!("The instance transfer failed authentication."))
}

pub async fn serve(invite: Invite, listener: TcpListener, archive: &Path, job: &Job) -> Result<()> {
    let mut file = tokio::fs::File::open(archive).await?;
    let length = file.metadata().await?.len();
    ensure!(
        (1..=MAX_ARCHIVE).contains(&length),
        "The share archive exceeds 2 GiB."
    );
    job.stage("Waiting for a recipient", 0);
    let mut stream = timeout(Duration::from_secs(900), async {
        loop {
            let (mut candidate, _) = listener.accept().await?;
            let challenge = random()?;
            let proof = timeout(Duration::from_secs(3), async {
                candidate.write_all(&challenge).await?;
                let mut tag = [0; 32];
                candidate.read_exact(&mut tag).await?;
                Ok::<_, std::io::Error>(tag)
            })
            .await;
            if let Ok(Ok(tag)) = proof {
                if hmac::verify(&invite.auth(), &challenge, &tag).is_ok() {
                    return Ok::<_, anyhow::Error>(candidate);
                }
            }
        }
    })
    .await
    .context("This share code expired after 15 minutes.")??;
    drop(listener);

    let cipher = invite.cipher()?;
    let mut buffer = vec![0; BLOCK];
    job.stage(
        "Sending the encrypted instance",
        length.div_ceil(BLOCK as u64) as usize,
    );
    send_block(&mut stream, &cipher, 0, &length.to_be_bytes()).await?;
    for (index, offset) in (0..length).step_by(BLOCK).enumerate() {
        let width = (length - offset).min(BLOCK as u64) as usize;
        file.read_exact(&mut buffer[..width]).await?;
        send_block(&mut stream, &cipher, index as u64 + 1, &buffer[..width]).await?;
        job.completed();
    }
    stream.shutdown().await?;
    Ok(())
}

pub async fn receive(invite: Invite, archive: &Path, job: &Job) -> Result<()> {
    job.stage("Connecting to the instance host", 0);
    let mut stream = timeout(IO_LIMIT, TcpStream::connect(invite.address))
        .await
        .context("Could not reach the host. Check their IP, port, and firewall.")?
        .context("Could not reach the host. Check that the share is ready on their PC.")?;
    let mut challenge = [0; 32];
    timeout(IO_LIMIT, stream.read_exact(&mut challenge))
        .await
        .context("The host did not start the transfer")??;
    let proof = hmac::sign(&invite.auth(), &challenge);
    timeout(IO_LIMIT, stream.write_all(proof.as_ref())).await??;
    let cipher = invite.cipher()?;
    let mut header = [0; 8 + TAG];
    let length = u64::from_be_bytes(
        receive_block(&mut stream, &cipher, 0, &mut header)
            .await?
            .try_into()?,
    );
    ensure!(
        (1..=MAX_ARCHIVE).contains(&length),
        "The share archive exceeds 2 GiB."
    );
    job.stage(
        "Receiving the encrypted instance",
        length.div_ceil(BLOCK as u64) as usize,
    );
    let mut output = tokio::fs::File::create(archive).await?;
    let mut buffer = vec![0; BLOCK + TAG];
    for (index, offset) in (0..length).step_by(BLOCK).enumerate() {
        let width = (length - offset).min(BLOCK as u64) as usize;
        let body = receive_block(
            &mut stream,
            &cipher,
            index as u64 + 1,
            &mut buffer[..width + TAG],
        )
        .await?;
        output.write_all(body).await?;
        job.completed();
    }
    output.sync_all().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::Jobs;

    #[tokio::test]
    async fn a_share_code_transfers_a_multiblock_archive_once() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let sender = Invite {
            address,
            key: random()?,
        };
        let receiver = Invite::parse(&sender.code()?)?;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.zip");
        let destination = temp.path().join("received.zip");
        let bytes: Vec<u8> = (0..(BLOCK * 2 + 47))
            .map(|byte| (byte % 251) as u8)
            .collect();
        std::fs::write(&source, &bytes)?;
        let jobs = Jobs::default();
        let send_job = jobs.start(None, "Send".into());
        let receive_job = jobs.start(None, "Receive".into());
        timeout(Duration::from_secs(5), async {
            tokio::try_join!(
                serve(sender, listener, &source, &send_job),
                receive(receiver, &destination, &receive_job)
            )
        })
        .await??;
        assert_eq!(std::fs::read(destination)?, bytes);
        assert!(TcpStream::connect(address).await.is_err());
        Ok(())
    }
}
