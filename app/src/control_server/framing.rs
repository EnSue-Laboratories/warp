//! 4-byte big-endian length-prefixed JSON framing for the control socket.
//!
//! Each message: `[len: u32 BE][JSON bytes …]`. `len` excludes itself.

use anyhow::{anyhow, Context, Result};
use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use serde::{de::DeserializeOwned, Serialize};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024; // 16 MiB hard cap

pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> Result<T> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await.context("read length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(anyhow!("invalid frame length: {len}"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.context("read body")?;
    let value = serde_json::from_slice::<T>(&buf).context("decode JSON")?;
    Ok(value)
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode JSON")?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(anyhow!("frame too large: {} bytes", body.len()));
    }
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).await.context("write length")?;
    writer.write_all(&body).await.context("write body")?;
    writer.flush().await.context("flush")?;
    Ok(())
}

/// Synchronous variant used by the CLI client (which doesn't have an async
/// executor running). Reads a length-prefixed JSON frame.
pub fn read_frame_sync<R: std::io::Read, T: DeserializeOwned>(reader: &mut R) -> Result<T> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).context("read length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(anyhow!("invalid frame length: {len}"));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).context("read body")?;
    let value = serde_json::from_slice::<T>(&buf).context("decode JSON")?;
    Ok(value)
}

/// Synchronous variant used by the CLI client.
pub fn write_frame_sync<W: std::io::Write, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode JSON")?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(anyhow!("frame too large: {} bytes", body.len()));
    }
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).context("write length")?;
    writer.write_all(&body).context("write body")?;
    writer.flush().context("flush")?;
    Ok(())
}
