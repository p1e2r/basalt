//! Async packet framing with optional zlib compression.
use crate::buf::{Reader, Writer};
use anyhow::{bail, Result};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::io::{Read, Write};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

pub struct Frame {
    pub id: i32,
    pub data: Vec<u8>,
}
impl Frame {
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.data)
    }
}

/// Frame reader. `read` is cancel-safe (usable in `tokio::select!`): bytes are buffered here and a
/// frame is only consumed once it is complete.
pub struct ConnReader {
    inner: OwnedReadHalf,
    buf: Vec<u8>,
    pub threshold: i32,
}
pub struct ConnWriter {
    inner: OwnedWriteHalf,
    pub threshold: i32,
}

pub struct Conn {
    pub r: ConnReader,
    pub w: ConnWriter,
}

impl Conn {
    pub fn new(s: TcpStream) -> Self {
        s.set_nodelay(true).ok();
        let (r, w) = s.into_split();
        Conn {
            r: ConnReader {
                inner: r,
                buf: Vec::with_capacity(1 << 16),
                threshold: -1,
            },
            w: ConnWriter {
                inner: w,
                threshold: -1,
            },
        }
    }
    pub async fn read(&mut self) -> Result<Frame> {
        self.r.read().await
    }
    pub async fn send(&mut self, w: &Writer) -> Result<()> {
        self.w.send(w).await
    }
    pub fn set_threshold(&mut self, t: i32) {
        self.r.threshold = t;
        self.w.threshold = t;
    }
}

/// Parse a varint at the start of `b`: Ok(Some((value, len))) or Ok(None) if incomplete.
fn peek_varint(b: &[u8]) -> Result<Option<(i32, usize)>> {
    let mut v: u32 = 0;
    for i in 0..5 {
        let Some(&x) = b.get(i) else { return Ok(None) };
        v |= ((x & 0x7F) as u32) << (7 * i);
        if x & 0x80 == 0 {
            return Ok(Some((v as i32, i + 1)));
        }
    }
    bail!("varint too long")
}

impl ConnReader {
    pub async fn read(&mut self) -> Result<Frame> {
        let buf = loop {
            if let Some((len, n)) = peek_varint(&self.buf)? {
                if len <= 0 || len > 1 << 23 {
                    bail!("bad frame length {len}")
                }
                let end = n + len as usize;
                if self.buf.len() >= end {
                    let frame = self.buf[n..end].to_vec();
                    self.buf.drain(..end);
                    break frame;
                }
            }
            if self.buf.capacity() - self.buf.len() < 8192 {
                self.buf.reserve(1 << 16);
            }
            // read_buf is cancel-safe: nothing is lost if this future is dropped
            if self.inner.read_buf(&mut self.buf).await? == 0 {
                bail!("connection closed");
            }
        };
        let body = if self.threshold >= 0 {
            let mut r = Reader::new(&buf);
            let dlen = r.varint()?;
            if dlen == 0 {
                r.rest().to_vec()
            } else {
                if dlen > 1 << 23 {
                    bail!("decompressed too large")
                }
                let mut out = Vec::with_capacity(dlen as usize);
                ZlibDecoder::new(r.rest()).read_to_end(&mut out)?;
                out
            }
        } else {
            buf
        };
        let mut r = Reader::new(&body);
        let id = r.varint()?;
        let data = r.rest().to_vec();
        Ok(Frame { id, data })
    }
}

/// Encode a packet (id + body already in `w`) into a length-prefixed frame.
pub fn encode_frame(w: &[u8], threshold: i32) -> Vec<u8> {
    let mut out = Writer::new();
    if threshold >= 0 {
        if w.len() as i32 >= threshold {
            let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
            e.write_all(w).unwrap();
            let comp = e.finish().unwrap();
            let mut inner = Writer::new();
            inner.varint(w.len() as i32);
            inner.bytes(&comp);
            out.varint(inner.len() as i32);
            out.bytes(&inner.buf);
        } else {
            out.varint(w.len() as i32 + 1);
            out.u8(0);
            out.bytes(w);
        }
    } else {
        out.varint(w.len() as i32);
        out.bytes(w);
    }
    out.buf
}

impl ConnWriter {
    pub async fn send(&mut self, w: &Writer) -> Result<()> {
        let f = encode_frame(&w.buf, self.threshold);
        self.inner.write_all(&f).await?;
        Ok(())
    }
    pub async fn send_raw(&mut self, frame: &[u8]) -> Result<()> {
        self.inner.write_all(frame).await?;
        Ok(())
    }
    pub async fn flush(&mut self) -> Result<()> {
        self.inner.flush().await?;
        Ok(())
    }
}
