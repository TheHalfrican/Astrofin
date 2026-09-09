use std::fmt::Debug;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::{Listener as SocketListener, Stream as SocketStream};
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Name as SocketName, ToFsName};
use jfn_platform_abi::Instance;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub mod jfn;

/// Longest line-delimited frame accepted from a peer, in bytes.
///
/// Every message on this channel is one small JSON object
/// (`{"message":"Ping"}`), so 64 KiB is orders of magnitude of headroom. The
/// cap exists because the name is reachable by any local process that knows
/// it: without one, a peer that writes forever without sending a newline
/// grows the receive buffer until the app is out of memory.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

pub struct Name {
    path: PathBuf,
}

impl Name {
    pub fn for_instance(instance: &Instance) -> io::Result<Self> {
        Ok(Self {
            path: jfn_paths::instance_listener_path(instance.id())?,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn to_socket(&self) -> io::Result<SocketName<'static>> {
        self.path.clone().to_fs_name::<GenericFilePath>()
    }
}

pub struct Stream {
    inner: BufReader<SocketStream>,
}

impl Stream {
    pub async fn connect(instance: &Instance) -> io::Result<Self> {
        let name = Name::for_instance(instance)?;
        let stream = SocketStream::connect(name.to_socket()?).await?;
        Ok(Self::wrap(stream))
    }

    fn wrap(stream: SocketStream) -> Self {
        Self {
            inner: BufReader::new(stream),
        }
    }

    pub async fn send<M: Serialize>(&mut self, msg: &M) -> io::Result<()> {
        let mut bytes =
            serde_json::to_vec(msg).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
        bytes.push(b'\n');
        let stream = self.inner.get_mut();
        stream.write_all(&bytes).await?;
        stream.flush().await
    }

    pub async fn recv<M: DeserializeOwned>(&mut self) -> io::Result<Option<M>> {
        match read_frame(&mut self.inner).await? {
            Some(frame) => parse_frame(&frame).map(Some),
            None => Ok(None),
        }
    }
}

/// One line-delimited frame with its newline stripped, or `None` at a clean
/// end of stream.
///
/// A frame longer than [`MAX_FRAME_BYTES`] is an error rather than a
/// truncation: every caller drops the connection on error, so there is no
/// resynchronisation to get wrong, and the peer never gets to spend our
/// memory. Bytes that arrive before an end of stream without a newline are
/// handed on as a frame — a truncated write fails to parse, exactly as it did
/// when this was `read_line`.
async fn read_frame<R>(reader: &mut R) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut frame: Vec<u8> = Vec::new();
    loop {
        let (chunk_len, complete) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Ok((!frame.is_empty()).then_some(frame));
            }
            let (chunk, complete) = match available.iter().position(|b| *b == b'\n') {
                Some(idx) => (&available[..idx], true),
                None => (available, false),
            };
            if frame.len() + chunk.len() > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("frame longer than {MAX_FRAME_BYTES} bytes"),
                ));
            }
            frame.extend_from_slice(chunk);
            (chunk.len(), complete)
        };
        reader.consume(chunk_len + usize::from(complete));
        if complete {
            return Ok(Some(frame));
        }
    }
}

/// Decode one frame. Trailing whitespace is stripped (a CR from a CRLF peer,
/// above all), as `read_line` plus `str::trim_end` used to.
fn parse_frame<M: DeserializeOwned>(frame: &[u8]) -> io::Result<M> {
    let end = frame
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |last| last + 1);
    serde_json::from_slice(&frame[..end]).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))
}

#[must_use]
pub enum Start {
    Started(Listener),
    AlreadyRunning,
    Failed(io::Error),
}

pub struct Listener {
    shutdown: Arc<Notify>,
    accept: Option<JoinHandle<()>>,
}

impl Listener {
    pub async fn try_start<Req, Resp>(instance: &Instance, handle: fn(&Req) -> Resp) -> Start
    where
        Req: DeserializeOwned + Debug + Send + 'static,
        Resp: Serialize + Send + Sync + 'static,
    {
        match Name::for_instance(instance) {
            Ok(name) => Self::create(&name, handle).await,
            Err(e) => Start::Failed(e),
        }
    }

    async fn create<Req, Resp>(name: &Name, handle: fn(&Req) -> Resp) -> Start
    where
        Req: DeserializeOwned + Debug + Send + 'static,
        Resp: Serialize + Send + Sync + 'static,
    {
        match Self::make(name, false) {
            Ok(listener) => Self::spawn(listener, handle),
            Err(e) if name_taken(&e) => match Self::probe(name).await {
                Probe::AlreadyRunning => Start::AlreadyRunning,
                Probe::Stale => match Self::make(name, true) {
                    Ok(listener) => Self::spawn(listener, handle),
                    Err(e) => Start::Failed(e),
                },
                Probe::Failed(e) => Start::Failed(e),
            },
            Err(e) => Start::Failed(e),
        }
    }

    fn make(name: &Name, overwrite: bool) -> io::Result<SocketListener> {
        ListenerOptions::new()
            .name(name.to_socket()?)
            .try_overwrite(overwrite)
            .create_tokio()
    }

    async fn probe(name: &Name) -> Probe {
        let sock = match name.to_socket() {
            Ok(sock) => sock,
            Err(e) => return Probe::Failed(e),
        };
        match SocketStream::connect(sock).await {
            Ok(_) => Probe::AlreadyRunning,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => Probe::Stale,
            // Unreachable ≠ dead — never classify (and later reclaim) as stale.
            Err(e) => Probe::Failed(e),
        }
    }

    fn spawn<Req, Resp>(listener: SocketListener, handle: fn(&Req) -> Resp) -> Start
    where
        Req: DeserializeOwned + Debug + Send + 'static,
        Resp: Serialize + Send + Sync + 'static,
    {
        let shutdown = Arc::new(Notify::new());
        let accept = tokio::spawn(accept_loop(listener, handle, shutdown.clone()));
        Start::Started(Listener {
            shutdown,
            accept: Some(accept),
        })
    }

    pub async fn shutdown(mut self) {
        self.shutdown.notify_waiters();
        if let Some(accept) = self.accept.take() {
            accept.abort();
            let _ = accept.await;
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shutdown.notify_waiters();
        if let Some(accept) = self.accept.take() {
            accept.abort();
        }
    }
}

/// Does a failed bind mean somebody already holds the name?
///
/// Unix reports a socket file that is already there — live or stale — as
/// `AddrInUse`. Windows never does: `interprocess` creates the first pipe
/// instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so every later
/// `CreateNamedPipe` on that name fails with `ERROR_ACCESS_DENIED`.
fn name_taken(e: &io::Error) -> bool {
    e.kind() == ErrorKind::AddrInUse || (cfg!(windows) && e.kind() == ErrorKind::PermissionDenied)
}

enum Probe {
    AlreadyRunning,
    Stale,
    Failed(io::Error),
}

async fn accept_loop<Req, Resp>(
    listener: SocketListener,
    handle: fn(&Req) -> Resp,
    shutdown: Arc<Notify>,
) where
    Req: DeserializeOwned + Debug + Send + 'static,
    Resp: Serialize + Send + Sync + 'static,
{
    loop {
        let conn = tokio::select! {
            () = shutdown.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!("accept: {e}");
                    continue;
                }
            },
        };
        tokio::spawn(serve(Stream::wrap(conn), handle, shutdown.clone()));
    }
}

async fn serve<Req, Resp>(mut stream: Stream, handle: fn(&Req) -> Resp, shutdown: Arc<Notify>)
where
    Req: DeserializeOwned + Debug + Send + 'static,
    Resp: Serialize + Send + Sync + 'static,
{
    loop {
        let received = tokio::select! {
            () = shutdown.notified() => break,
            received = stream.recv::<Req>() => received,
        };
        match received {
            Ok(Some(req)) => {
                tracing::debug!("received {req:?}");
                let resp = handle(&req);
                if let Err(e) = stream.send(&resp).await {
                    tracing::warn!("send response: {e}");
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("recv: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{MAX_FRAME_BYTES, name_taken, parse_frame, read_frame};
    use serde::Deserialize;
    use std::io::{self, ErrorKind};
    use tokio::io::BufReader;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Msg {
        payload: String,
    }

    async fn frames(input: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let mut reader = BufReader::new(input);
        let mut out = Vec::new();
        while let Some(frame) = read_frame(&mut reader).await? {
            out.push(frame);
        }
        Ok(out)
    }

    #[tokio::test]
    async fn read_frame_splits_on_newlines_and_strips_them() {
        let got = frames(b"{\"a\":1}\n{\"b\":2}\n").await.unwrap();
        assert_eq!(got, vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]);
    }

    #[tokio::test]
    async fn read_frame_ends_at_a_clean_eof() {
        assert!(frames(b"").await.unwrap().is_empty());
    }

    /// A peer that dies mid-write: the partial bytes still surface as a frame
    /// (they fail to parse), which is what `read_line` did.
    #[tokio::test]
    async fn read_frame_yields_a_partial_frame_at_eof() {
        assert_eq!(
            frames(b"{\"a\":1").await.unwrap(),
            vec![b"{\"a\":1".to_vec()]
        );
    }

    #[tokio::test]
    async fn read_frame_yields_an_empty_frame_for_a_bare_newline() {
        let empty: Vec<Vec<u8>> = vec![Vec::new(), Vec::new()];
        assert_eq!(frames(b"\n\n").await.unwrap(), empty);
    }

    #[tokio::test]
    async fn read_frame_accepts_a_frame_of_exactly_the_cap() {
        let mut input = vec![b'x'; MAX_FRAME_BYTES];
        input.push(b'\n');
        let got = frames(&input).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].len(), MAX_FRAME_BYTES);
    }

    #[tokio::test]
    async fn read_frame_rejects_one_byte_over_the_cap() {
        let mut input = vec![b'x'; MAX_FRAME_BYTES + 1];
        input.push(b'\n');
        let err = frames(&input).await.expect_err("over the cap");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert!(err.to_string().contains("longer than"), "{err}");
    }

    /// The reason the cap exists: a local peer that never sends a newline must
    /// not be able to grow the receive buffer without bound.
    #[tokio::test]
    async fn read_frame_rejects_a_flood_with_no_newline_at_all() {
        let input = vec![b'x'; 4 * 1024 * 1024];
        let err = frames(&input).await.expect_err("unterminated flood");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn read_frame_caps_the_total_frame_across_reads() {
        // Arrives in many small buffer fills, none of which is over the cap on
        // its own; the running total still is.
        let input = vec![b'x'; MAX_FRAME_BYTES + 4096];
        let err = frames(&input).await.expect_err("over the cap");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn parse_frame_reads_a_json_object() {
        let msg: Msg = parse_frame(br#"{"payload":"hi"}"#).unwrap();
        assert_eq!(msg.payload, "hi");
    }

    #[test]
    fn parse_frame_tolerates_a_crlf_peer() {
        let msg: Msg = parse_frame(b"{\"payload\":\"hi\"}\r").unwrap();
        assert_eq!(msg.payload, "hi");
    }

    #[test]
    fn parse_frame_rejects_malformed_json_as_invalid_data() {
        let err = parse_frame::<Msg>(b"{not json").expect_err("malformed");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn parse_frame_rejects_a_well_formed_message_of_the_wrong_shape() {
        let err = parse_frame::<Msg>(br#"{"payload":42}"#).expect_err("wrong type");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn parse_frame_rejects_binary_garbage() {
        let err = parse_frame::<Msg>(&[0x00, 0xff, 0xfe, 0x80]).expect_err("binary");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn parse_frame_rejects_an_empty_frame() {
        let err = parse_frame::<Msg>(b"").expect_err("empty");
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    /// Unix reports an existing socket file as `AddrInUse`; Windows reports a
    /// taken pipe name as `PermissionDenied`, never `AddrInUse`.
    #[test]
    fn name_taken_classifies_the_platform_bind_errors() {
        assert!(name_taken(&io::Error::from(ErrorKind::AddrInUse)));
        assert_eq!(
            name_taken(&io::Error::from(ErrorKind::PermissionDenied)),
            cfg!(windows)
        );
        assert!(!name_taken(&io::Error::from(ErrorKind::NotFound)));
        assert!(!name_taken(&io::Error::from(ErrorKind::ConnectionRefused)));
    }
}
