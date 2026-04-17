//! bunzo wire protocol v1.
//!
//! Framing: a 4-byte big-endian unsigned length, then that many bytes of JSON.
//! Both directions use the same framing. Messages are versioned via the
//! top-level `v` field on every frame. Frame bodies are capped at 1 MiB —
//! bunzo never expects anything close to that, so the cap exists to fail fast
//! on a desynced stream.
//!
//! The `tokio` feature adds async counterparts of [`read_frame`] and
//! [`write_frame`] under [`async_io`].

use std::io::{self, Read, Write};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_FRAME_BYTES: u32 = 1 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    UserMessage { id: String, text: String },
    Cancel { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerMessage {
    AssistantChunk {
        id: String,
        text: String,
    },
    AssistantEnd {
        id: String,
        finish_reason: String,
    },
    Error {
        id: String,
        code: String,
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub v: u8,
    #[serde(flatten)]
    pub msg: T,
}

impl<T> Envelope<T> {
    pub fn new(msg: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            msg,
        }
    }
}

pub type ClientFrame = Envelope<ClientMessage>;
pub type ServerFrame = Envelope<ServerMessage>;

pub fn write_frame<W, T>(w: &mut W, msg: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME_BYTES as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame body {} exceeds cap {}", body.len(), MAX_FRAME_BYTES),
        ));
    }
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<R, T>(r: &mut R) -> io::Result<T>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame len {} exceeds cap {}", len, MAX_FRAME_BYTES),
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

#[cfg(feature = "tokio")]
pub mod async_io {
    use super::*;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    pub async fn write_frame_async<W, T>(w: &mut W, msg: &T) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        T: Serialize,
    {
        let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
        if body.len() > MAX_FRAME_BYTES as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame body {} exceeds cap {}", body.len(), MAX_FRAME_BYTES),
            ));
        }
        w.write_all(&(body.len() as u32).to_be_bytes()).await?;
        w.write_all(&body).await?;
        w.flush().await
    }

    pub async fn read_frame_async<R, T>(r: &mut R) -> io::Result<T>
    where
        R: AsyncRead + Unpin,
        T: DeserializeOwned,
    {
        let mut len_buf = [0u8; 4];
        r.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf);
        if len > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame len {} exceeds cap {}", len, MAX_FRAME_BYTES),
            ));
        }
        let mut body = vec![0u8; len as usize];
        r.read_exact(&mut body).await?;
        serde_json::from_slice(&body).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn roundtrip_client_user_message() {
        let out = Envelope::new(ClientMessage::UserMessage {
            id: "u1".into(),
            text: "hello".into(),
        });
        let mut buf = Vec::new();
        write_frame(&mut buf, &out).unwrap();
        let mut cur = Cursor::new(buf);
        let back: ClientFrame = read_frame(&mut cur).unwrap();
        assert_eq!(back.v, PROTOCOL_VERSION);
        match back.msg {
            ClientMessage::UserMessage { id, text } => {
                assert_eq!(id, "u1");
                assert_eq!(text, "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn roundtrip_server_frames() {
        for msg in [
            ServerMessage::AssistantChunk {
                id: "u1".into(),
                text: "part ".into(),
            },
            ServerMessage::AssistantEnd {
                id: "u1".into(),
                finish_reason: "stop".into(),
            },
            ServerMessage::Error {
                id: "u1".into(),
                code: "backend_unavailable".into(),
                text: "openai returned 500".into(),
            },
        ] {
            let out = Envelope::new(msg);
            let mut buf = Vec::new();
            write_frame(&mut buf, &out).unwrap();
            let mut cur = Cursor::new(buf);
            let _back: ServerFrame = read_frame(&mut cur).unwrap();
        }
    }

    #[test]
    fn wire_bytes_are_big_endian_length_prefix() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &Envelope::new(ClientMessage::UserMessage {
                id: "x".into(),
                text: "y".into(),
            }),
        )
        .unwrap();
        assert_eq!(&buf[..4], &(buf.len() as u32 - 4).to_be_bytes());
    }

    #[test]
    fn oversize_len_rejected() {
        let mut buf = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
        buf.extend(std::iter::repeat(0u8).take(16));
        let mut cur = Cursor::new(buf);
        let res: io::Result<ServerFrame> = read_frame(&mut cur);
        assert!(res.is_err());
    }
}
