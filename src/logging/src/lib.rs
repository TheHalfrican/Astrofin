//! Logging backend.
//!
//! Two writers, both wrapped with `tracing_appender::non_blocking`:
//! - stderr (always)
//! - size-rotated file (optional, when `path` is non-empty)
//!
//! Every emitted line is filtered through the `redact` module so auth tokens
//! are 'x'-ed out. Anything other code writes to the real stderr (CEF
//! subprocesses, ffmpeg) is captured by a pipe-and-poll thread and
//! re-emitted as `[CEF]` debug records.

mod redact;

use parking_lot::Mutex;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{EnvFilter, Registry, filter::LevelFilter, fmt, layer::SubscriberExt};

const CATEGORY_COUNT: u8 = 7;
const LEVEL_COUNT: u8 = 5;

// Public category/level constants, and the u8 values accepted by `log` /
// `log_enabled`.
pub const CATEGORY_CEF: u8 = 2;
pub const CATEGORY_JS: u8 = 5;
pub const CATEGORY_RESOURCE: u8 = 6;
pub const LEVEL_DEBUG: u8 = 1;
pub const LEVEL_INFO: u8 = 2;
pub const LEVEL_WARN: u8 = 3;
pub const LEVEL_ERROR: u8 = 4;

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
enum Level {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl Level {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Level::Trace,
            1 => Level::Debug,
            2 => Level::Info,
            3 => Level::Warn,
            _ => Level::Error,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

// =====================================================================
// Rotating file writer
// =====================================================================

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_BACKUPS: usize = 3;

struct RotatingFile {
    path: PathBuf,
    file: File,
    max_bytes: u64,
    max_backups: usize,
    bytes_written: u64,
}

impl RotatingFile {
    fn open(path: PathBuf, max_bytes: u64, max_backups: usize) -> io::Result<Self> {
        // Start each run with a fresh file; prior run's contents shift into
        // the backup chain.
        shift_backups(&path, max_backups);
        let file = create_private(&path)?;
        Ok(Self {
            path,
            file,
            max_bytes,
            max_backups,
            bytes_written: 0,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        shift_backups(&self.path, self.max_backups);
        self.file = create_private(&self.path)?;
        self.bytes_written = 0;
        Ok(())
    }
}

/// Create (or truncate) a log file readable by the owning user only. A log
/// line can carry a token shape the redactor does not know yet, so the file
/// must not be world-readable; on Unix `File::create` would leave it at
/// 0644 & ~umask. Windows inherits the per-user ACL of `%LOCALAPPDATA%`.
fn create_private(path: &Path) -> io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn shift_backups(path: &Path, max_backups: usize) {
    let _ = std::fs::remove_file(backup_path(path, max_backups));
    for i in (1..max_backups).rev() {
        let _ = std::fs::rename(backup_path(path, i), backup_path(path, i + 1));
    }
    let _ = std::fs::rename(path, backup_path(path, 1));
}

fn backup_path(path: &Path, n: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

impl Write for RotatingFile {
    // Rotates before writing when the buffer would cross `max_bytes`, so a
    // record never spans two files.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes_written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.bytes_written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

// =====================================================================
// Per-OS console writer + stderr capture
// =====================================================================

#[cfg(unix)]
mod imp {
    use super::{CATEGORY_CEF, Level, emit};
    use nix::errno::Errno;
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::unistd::{dup, dup2_stderr, isatty, pipe, read, write};
    use std::io::{self, Write};
    use std::os::fd::{AsFd, OwnedFd};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    use jfn_wake_event::WakeEvent;

    // Holds a dup of the original stderr taken before StderrCapture's
    // dup2 redirect; writing via io::stderr() here would feed each log
    // line back into the capture pipe.
    pub(super) struct StderrWriter {
        fd: Option<OwnedFd>,
    }

    impl Write for StderrWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match &self.fd {
                Some(fd) => Ok(write(fd, buf)?),
                None => Err(Errno::EBADF.into()),
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(super) fn make_console_writer() -> (StderrWriter, bool) {
        let fd = dup(io::stderr()).ok();
        let is_tty = fd.as_ref().is_some_and(|fd| isatty(fd).unwrap_or(false));
        (StderrWriter { fd }, is_tty)
    }

    pub(super) struct StderrCapture {
        original_fd: Option<OwnedFd>,
        wake: Arc<WakeEvent>,
        join: Option<JoinHandle<()>>,
    }

    impl StderrCapture {
        pub(super) fn start() -> Option<Self> {
            let original = dup(io::stderr()).ok()?;
            let (pipe_read, pipe_write) = pipe().ok()?;
            let wake = Arc::new(WakeEvent::new()?);

            dup2_stderr(&pipe_write).ok()?;
            drop(pipe_write);

            let thread_wake = Arc::clone(&wake);
            let join = thread::spawn(move || capture_loop(&pipe_read, &thread_wake));

            Some(StderrCapture {
                original_fd: Some(original),
                wake,
                join: Some(join),
            })
        }

        pub(super) fn stop(&mut self) {
            // Order: restore STDERR FIRST (so any concurrent writer drains to
            // the real fd from now on), THEN wake the capture thread, THEN
            // join. The wake outlives the join: it is dropped with the
            // StderrCapture, after the thread is gone.
            if let Some(original) = self.original_fd.take() {
                let _ = dup2_stderr(&original);
            }
            self.wake.signal();
            if let Some(h) = self.join.take()
                && let Err(e) = h.join()
            {
                eprintln!("[logging] stderr capture thread panicked: {e:?}");
            }
        }
    }

    fn capture_loop(pipe_read: &OwnedFd, wake: &WakeEvent) {
        let mut buf = [0u8; 4096];
        let mut partial = Vec::<u8>::new();
        loop {
            let mut pfds = [
                PollFd::new(pipe_read.as_fd(), PollFlags::POLLIN),
                PollFd::new(wake.as_fd(), PollFlags::POLLIN),
            ];
            if poll(&mut pfds, PollTimeout::NONE).is_err() {
                break;
            }
            let readable =
                |pfd: &PollFd| pfd.revents().is_some_and(|r| r.contains(PollFlags::POLLIN));
            if readable(&pfds[1]) {
                break;
            }
            if readable(&pfds[0]) {
                let Ok(n) = read(pipe_read, &mut buf) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                partial.extend_from_slice(&buf[..n]);
                while let Some(pos) = partial.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = partial.drain(..=pos).take(pos).collect();
                    if !line.is_empty() {
                        let msg = String::from_utf8_lossy(&line).into_owned();
                        emit(CATEGORY_CEF, Level::Debug, &msg);
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io::{self, Write};

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
        STD_ERROR_HANDLE, SetConsoleMode,
    };

    fn enable_vt_mode() {
        // Best-effort: tell conhost to honor ANSI SGR escapes on stderr.
        // Win10+ supports ENABLE_VIRTUAL_TERMINAL_PROCESSING; older builds
        // silently fail and we render with no color.
        unsafe {
            let h = GetStdHandle(STD_ERROR_HANDLE);
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                return;
            }
            let mut mode: CONSOLE_MODE = 0;
            if GetConsoleMode(h, &mut mode) == 0 {
                return;
            }
            SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }

    pub(super) struct StderrWriter;

    impl Write for StderrWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            io::stderr().write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            io::stderr().flush()
        }
    }

    pub(super) fn make_console_writer() -> (StderrWriter, bool) {
        use std::io::IsTerminal;
        let is_tty = io::stderr().is_terminal();
        if is_tty {
            enable_vt_mode();
        }
        (StderrWriter, is_tty)
    }

    pub(super) struct StderrCapture;

    impl StderrCapture {
        pub(super) fn start() -> Option<Self> {
            None
        }
        pub(super) fn stop(&mut self) {}
    }
}

// =====================================================================
// State
// =====================================================================

struct State {
    active_log_path: String,
    _console_guard: WorkerGuard,
    _file_guard: Option<WorkerGuard>,
    stderr_capture: Option<imp::StderrCapture>,
}

static STATE: OnceLock<Mutex<Option<State>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<State>> {
    STATE.get_or_init(|| Mutex::new(None))
}

const ISO_FILE_FMT: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
const CONSOLE_TRACE_FMT: &[FormatItem<'static>] =
    format_description!("[hour]:[minute]:[second].[subsecond digits:3]");

// =====================================================================
// Emit
// =====================================================================

// The one place a category number maps to a target string; `$mac` is
// re-invoked with the target literal prepended to its arguments.
macro_rules! with_category_target {
    ($category:expr, $mac:ident $(, $arg:expr)*) => {
        match $category {
            0 => $mac!("Main" $(, $arg)*),
            1 => $mac!("mpv" $(, $arg)*),
            2 => $mac!("CEF" $(, $arg)*),
            3 => $mac!("Media" $(, $arg)*),
            4 => $mac!("Platform" $(, $arg)*),
            5 => $mac!("JS" $(, $arg)*),
            6 => $mac!("Resource" $(, $arg)*),
            _ => $mac!("Unknown" $(, $arg)*),
        }
    };
}

// `tracing::event!` requires a literal `target` (it builds a `static`
// Callsite at the call site), so the level match keeps `target` a literal
// and materializes one static callsite per (category, level).
macro_rules! emit_at {
    ($tgt:expr, $lvl:expr, $msg:expr) => {{
        use tracing::Level as L;
        match $lvl {
            Level::Trace => tracing::event!(target: $tgt, L::TRACE, "{}", $msg),
            Level::Debug => tracing::event!(target: $tgt, L::DEBUG, "{}", $msg),
            Level::Info => tracing::event!(target: $tgt, L::INFO, "{}", $msg),
            Level::Warn => tracing::event!(target: $tgt, L::WARN, "{}", $msg),
            Level::Error => tracing::event!(target: $tgt, L::ERROR, "{}", $msg),
        }
    }};
}

macro_rules! enabled_at {
    ($tgt:expr, $lvl:expr) => {{
        use tracing::Level as L;
        match $lvl {
            Level::Trace => tracing::event_enabled!(target: $tgt, L::TRACE),
            Level::Debug => tracing::event_enabled!(target: $tgt, L::DEBUG),
            Level::Info => tracing::event_enabled!(target: $tgt, L::INFO),
            Level::Warn => tracing::event_enabled!(target: $tgt, L::WARN),
            Level::Error => tracing::event_enabled!(target: $tgt, L::ERROR),
        }
    }};
}

fn emit(category: u8, level: Level, msg: &str) {
    let msg = msg.trim_end_matches(['\r', '\n']);
    with_category_target!(category, emit_at, level, msg);
}

/// True if the filter admits any TRACE-level callsite — used to decide
/// whether to prepend HH:MM:SS.mmm on console lines.
fn filter_is_trace(filter: &EnvFilter) -> bool {
    filter.max_level_hint() == Some(LevelFilter::TRACE)
}

pub fn jfn_log_init(path: &str, filter: &str) {
    let path_str = path.to_string();
    let filter_str_raw = filter.to_string();
    let filter_str = if filter_str_raw.trim().is_empty() {
        "info".to_string()
    } else {
        filter_str_raw
    };

    // Bail early on second init: dispatcher is already installed and the
    // capture pipe / guards live in STATE. Mirrors prior behavior.
    {
        let guard = state().lock();
        if guard.is_some() {
            return;
        }
    }

    // Capture a dup of stderr now so console writes survive the later
    // dup2() redirect installed by StderrCapture, and aren't fed back into
    // the capture pipe.
    let (console_writer, is_tty) = imp::make_console_writer();
    let color = is_tty && std::env::var_os("NO_COLOR").is_none();
    let (console_nb, console_guard) = NonBlockingBuilder::default()
        .lossy(true)
        .finish(console_writer);

    let (file_nb, file_guard) = if !path_str.is_empty() {
        match RotatingFile::open(PathBuf::from(&path_str), MAX_FILE_BYTES, MAX_BACKUPS) {
            Ok(rf) => {
                let (nb, g) = NonBlockingBuilder::default().lossy(true).finish(rf);
                (Some(nb), Some(g))
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    let env_filter = match EnvFilter::try_new(&filter_str) {
        Ok(f) => f,
        Err(_) => EnvFilter::new("info"),
    };

    let trace_mode = filter_is_trace(&env_filter);

    let console_layer = fmt::layer()
        .event_format(ConsoleFormat { trace_mode, color })
        .with_writer(RedactMake(console_nb));

    let subscriber = Registry::default().with(env_filter).with(console_layer);

    // Add file layer conditionally without changing the subscriber type
    // for the install call. SubscriberExt::with returns a new type each
    // time, so we use boxed dispatch.
    let dispatch: tracing::Dispatch = if let Some(file_nb) = file_nb {
        let file_layer = fmt::layer()
            .event_format(FileFormat)
            .with_writer(RedactMake(file_nb));
        subscriber.with(file_layer).into()
    } else {
        subscriber.into()
    };

    // Fail-soft: if a dispatcher was already installed (unlikely given
    // the STATE.is_some() early-return above, but possible across crate
    // boundaries in tests), proceed without panicking.
    let _ = tracing::dispatcher::set_global_default(dispatch);

    let stderr_capture = imp::StderrCapture::start();

    let mut guard = state().lock();
    *guard = Some(State {
        active_log_path: path_str,
        _console_guard: console_guard,
        _file_guard: file_guard,
        stderr_capture,
    });
}

pub fn jfn_log_shutdown() {
    let mut guard = state().lock();
    if let Some(mut s) = guard.take() {
        if let Some(mut cap) = s.stderr_capture.take() {
            cap.stop();
        }
        // Drop file guard first so the file worker flushes before the
        // console worker; final console line therefore appears after
        // file flush completes on exit.
        s._file_guard = None;
        drop(s);
    }
}

pub fn log_enabled(category: u8, level: u8) -> bool {
    if category >= CATEGORY_COUNT || level >= LEVEL_COUNT {
        return false;
    }
    let level = Level::from_u8(level);
    with_category_target!(category, enabled_at, level)
}

pub fn log(category: u8, level: u8, msg: &str) {
    emit(category, Level::from_u8(level), msg);
}

pub fn active_path() -> String {
    let guard = state().lock();
    guard
        .as_ref()
        .map(|st| st.active_log_path.clone())
        .unwrap_or_default()
}

// =====================================================================
// tracing-subscriber FormatEvent impls (commit 2 wires these in)
// =====================================================================

use tracing::{Event, Subscriber, field::Field};
use tracing_subscriber::{
    fmt::{FmtContext, FormatEvent, FormatFields, format::Writer},
    registry::LookupSpan,
};

/// Records only the `"message"` field; ignores any structured fields.
#[derive(Default)]
struct MsgVisitor(String);

impl tracing::field::Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write;
            let _ = write!(self.0, "{value:?}");
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }
}

struct ConsoleFormat {
    trace_mode: bool,
    color: bool,
}

fn level_to_local_level(l: &tracing::Level) -> Level {
    match *l {
        tracing::Level::TRACE => Level::Trace,
        tracing::Level::DEBUG => Level::Debug,
        tracing::Level::INFO => Level::Info,
        tracing::Level::WARN => Level::Warn,
        tracing::Level::ERROR => Level::Error,
    }
}

fn ansi_for(level: &tracing::Level) -> (&'static str, &'static str) {
    match *level {
        tracing::Level::ERROR => ("\x1b[31m", "\x1b[0m"),
        tracing::Level::WARN => ("\x1b[33m", "\x1b[0m"),
        tracing::Level::INFO => ("\x1b[32m", "\x1b[0m"),
        tracing::Level::DEBUG => ("\x1b[36m", "\x1b[0m"),
        tracing::Level::TRACE => ("\x1b[2m", "\x1b[0m"),
    }
}

impl<S, N> FormatEvent<S, N> for ConsoleFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut w: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        if self.trace_mode
            && let Ok(now) = OffsetDateTime::now_local()
            && let Ok(s) = now.format(&CONSOLE_TRACE_FMT)
        {
            write!(w, "{s} ")?;
        }
        let meta = event.metadata();
        let target = meta.target();
        if self.color {
            let (open, close) = ansi_for(meta.level());
            write!(w, "{open}[{target}]{close} ")?;
        } else {
            write!(w, "[{target}] ")?;
        }
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        writeln!(w, "{}", v.0)
    }
}

struct FileFormat;

impl<S, N> FormatEvent<S, N> for FileFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut w: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        if let Ok(now) = OffsetDateTime::now_local()
            && let Ok(s) = now.format(&ISO_FILE_FMT)
        {
            write!(w, "{s} ")?;
        }
        let meta = event.metadata();
        let label = level_to_local_level(meta.level()).label();
        write!(w, "{label}")?;
        for _ in label.len()..7 {
            w.write_char(' ')?;
        }
        write!(w, " [{}] ", meta.target())?;
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        writeln!(w, "{}", v.0)
    }
}

// =====================================================================
// Redacting MakeWriter — runs `jfn_log_redact::censor` on each event's
// bytes before they reach the underlying non-blocking writer.
// =====================================================================

use tracing_subscriber::fmt::MakeWriter;

struct RedactMake<W>(W);

struct RedactGuard<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> Write for RedactGuard<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Drop for RedactGuard<W> {
    fn drop(&mut self) {
        if redact::contains_secret(&self.buf) {
            redact::censor(&mut self.buf);
        }
        let _ = self.inner.write_all(&self.buf);
    }
}

impl<'a, W> MakeWriter<'a> for RedactMake<W>
where
    W: Write + Clone + 'a,
{
    type Writer = RedactGuard<W>;
    fn make_writer(&'a self) -> Self::Writer {
        RedactGuard {
            inner: self.0.clone(),
            buf: Vec::new(),
        }
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as StdMutex;
    use std::sync::Arc;

    #[derive(Clone)]
    struct VecSink(Arc<StdMutex<Vec<u8>>>);
    impl Write for VecSink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn create_private_truncates_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.log");
        std::fs::write(&path, b"old contents").unwrap();
        let mut f = super::create_private(&path).unwrap();
        f.write_all(b"new").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn create_private_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.log");
        drop(super::create_private(&path).unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:o}");
    }
    #[test]
    fn redact_make_writer_censors_before_forwarding() -> io::Result<()> {
        let sink = VecSink(Arc::new(StdMutex::new(Vec::new())));
        let make = RedactMake(sink.clone());
        {
            let mut w = make.make_writer();
            w.write_all(b"GET /Items?api_key=abc123secret&x=1\n")?;
        }
        let bytes = sink.0.lock().clone();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("abc123secret"),
            "secret leaked through redactor: {text}"
        );
        assert!(
            text.contains("api_key=xxx"),
            "expected censored bytes: {text}"
        );
        Ok(())
    }

    #[test]
    fn redact_make_writer_passes_clean_bytes() -> io::Result<()> {
        let sink = VecSink(Arc::new(StdMutex::new(Vec::new())));
        let make = RedactMake(sink.clone());
        {
            let mut w = make.make_writer();
            w.write_all(b"[mpv] hello\n")?;
        }
        assert_eq!(&*sink.0.lock(), b"[mpv] hello\n");
        Ok(())
    }

    /// Emit `msg` through `log()` into a scoped subscriber wired exactly like
    /// the real one (same event format, same redacting writer) and return the
    /// bytes that reached the sink.
    fn emit_through_layer(msg: &str, file_layer: bool) -> String {
        let sink = VecSink(Arc::new(StdMutex::new(Vec::new())));
        let dispatch: tracing::Dispatch = if file_layer {
            let layer = fmt::layer()
                .event_format(FileFormat)
                .with_writer(RedactMake(sink.clone()));
            Registry::default().with(layer).into()
        } else {
            let layer = fmt::layer()
                .event_format(ConsoleFormat {
                    trace_mode: false,
                    color: false,
                })
                .with_writer(RedactMake(sink.clone()));
            Registry::default().with(layer).into()
        };
        tracing::dispatcher::with_default(&dispatch, || {
            log(CATEGORY_CEF, LEVEL_INFO, msg);
        });
        let bytes = sink.0.lock().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn log_writes_the_category_target_and_the_message() {
        let line = emit_through_layer("hello", false);
        assert!(line.starts_with("[CEF] "), "unexpected line: {line:?}");
        assert!(line.ends_with("hello\n"), "unexpected line: {line:?}");
    }

    #[test]
    fn a_token_logged_through_log_reaches_neither_writer_in_clear() {
        // Both writers wrap RedactMake, so a token has to be censored on the
        // console *and* in the rotating file, not just one of them.
        let hostile = concat!(
            "GET http://u:PWSECRET@host/Items?api_key=KEYSECRET ",
            r#"hdr X-Emby-Token: HDRSECRET body {"AccessToken":"JSONSECRET"}"#
        );
        for file_layer in [false, true] {
            let line = emit_through_layer(hostile, file_layer);
            for secret in ["PWSECRET", "KEYSECRET", "HDRSECRET", "JSONSECRET"] {
                assert!(
                    !line.contains(secret),
                    "{secret} leaked (file_layer={file_layer}): {line}"
                );
            }
            assert!(line.contains("api_key=xxx"), "not censored: {line}");
        }
    }

    #[test]
    fn a_message_is_flattened_to_one_record_before_redaction() {
        // Redaction works per record, so a message that carries its own
        // newlines must not smuggle a token onto a second, unscanned line:
        // assert the whole thing is still censored as one buffer.
        let line = emit_through_layer("first\n?api_key=SECRETVAL\nlast", true);
        assert!(!line.contains("SECRETVAL"), "leaked: {line}");
    }

    #[test]
    fn level_label_padding_matches_file_format() {
        // FileFormat pads level label to 7 chars via a fill loop.
        // Sanity-check Level::label widths so padding code stays correct.
        assert_eq!(Level::Trace.label(), "TRACE");
        assert_eq!(Level::Debug.label(), "DEBUG");
        assert_eq!(Level::Info.label(), "INFO");
        assert_eq!(Level::Warn.label(), "WARN");
        assert_eq!(Level::Error.label(), "ERROR");
        for l in [
            Level::Trace,
            Level::Debug,
            Level::Info,
            Level::Warn,
            Level::Error,
        ] {
            assert!(l.label().len() <= 7);
        }
    }

    fn enabled_under(directive: &str, category: u8, level: u8) -> bool {
        let dispatch = tracing::Dispatch::new(Registry::default().with(EnvFilter::new(directive)));
        tracing::dispatcher::with_default(&dispatch, || log_enabled(category, level))
    }

    #[test]
    fn log_enabled_respects_global_filter() {
        // "warn" → Info disabled, Warn/Error enabled for any category.
        assert!(!enabled_under("warn", 0 /* Main */, 2 /* Info */));
        assert!(enabled_under("warn", 0, 3 /* Warn */));
        assert!(enabled_under("warn", 0, 4 /* Error */));
    }

    #[test]
    fn log_enabled_respects_target_override() {
        // Global warn, but mpv=trace → mpv Trace enabled, Main Trace not.
        assert!(enabled_under(
            "warn,mpv=trace",
            1, /* mpv */
            0  /* Trace */
        ));
        assert!(!enabled_under("warn,mpv=trace", 0 /* Main */, 0));
    }

    #[test]
    fn log_enabled_respects_off_directive() {
        // Global info, CEF=off → CEF Error disabled, Main Error enabled.
        assert!(!enabled_under(
            "info,CEF=off",
            2, /* CEF */
            4  /* Error */
        ));
        assert!(enabled_under("info,CEF=off", 0 /* Main */, 4));
    }

    #[test]
    fn log_enabled_rejects_out_of_range_category_and_level() {
        assert!(!enabled_under("trace", CATEGORY_COUNT, 4));
        assert!(!enabled_under("trace", 0, LEVEL_COUNT));
    }

    #[test]
    fn filter_is_trace_detects_bare_and_target_directives() {
        for directive in [
            "trace",
            "info,mpv=trace",
            "debug,CEF=trace,mpv=warn",
            "mpv=5",
            "CEF[{field}]=trace",
        ] {
            assert!(
                filter_is_trace(&EnvFilter::new(directive)),
                "expected trace mode for {directive:?}"
            );
        }
        for directive in ["info", "debug,mpv=warn", "warn,CEF=off"] {
            assert!(
                !filter_is_trace(&EnvFilter::new(directive)),
                "unexpected trace mode for {directive:?}"
            );
        }
    }

    #[test]
    fn open_shifts_previous_run_into_backup_one() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("run.log");
        std::fs::write(&path, b"previous run\n")?;
        let mut rf = RotatingFile::open(path.clone(), 1024, 3)?;
        rf.write_all(b"current run\n")?;
        rf.flush()?;
        assert_eq!(std::fs::read(&path)?, b"current run\n");
        assert_eq!(std::fs::read(backup_path(&path, 1))?, b"previous run\n");
        Ok(())
    }

    #[test]
    fn write_past_limit_rotates_without_splitting_a_record() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("run.log");
        let mut rf = RotatingFile::open(path.clone(), 8, 3)?;
        rf.write_all(b"aaaaa\n")?;
        rf.write_all(b"bbbbb\n")?;
        rf.flush()?;
        assert_eq!(std::fs::read(&path)?, b"bbbbb\n");
        assert_eq!(std::fs::read(backup_path(&path, 1))?, b"aaaaa\n");
        Ok(())
    }

    #[test]
    fn backups_beyond_max_are_dropped() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("run.log");
        let mut rf = RotatingFile::open(path.clone(), 8, 2)?;
        for record in [&b"1111\n"[..], b"2222\n", b"3333\n", b"4444\n"] {
            rf.write_all(record)?;
        }
        rf.flush()?;
        assert_eq!(std::fs::read(&path)?, b"4444\n");
        assert_eq!(std::fs::read(backup_path(&path, 1))?, b"3333\n");
        assert_eq!(std::fs::read(backup_path(&path, 2))?, b"2222\n");
        assert!(!backup_path(&path, 3).exists());
        Ok(())
    }

    fn capture_console(color: bool) -> String {
        let sink = VecSink(Arc::new(StdMutex::new(Vec::new())));
        let layer = fmt::layer()
            .event_format(ConsoleFormat {
                trace_mode: false,
                color,
            })
            .with_writer({
                let sink = sink.clone();
                move || sink.clone()
            });
        let subscriber = Registry::default().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::event!(target: "mpv", tracing::Level::ERROR, "boom");
        });
        let bytes = sink.0.lock().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn console_format_no_color_is_plain() {
        let s = capture_console(false);
        assert!(s.starts_with("[mpv] "), "unexpected line: {s:?}");
        assert!(!s.contains('\x1b'), "unexpected ANSI in: {s:?}");
        assert!(s.ends_with("boom\n"), "unexpected line: {s:?}");
    }

    #[test]
    fn console_format_color_wraps_target() {
        let s = capture_console(true);
        let (open, close) = ansi_for(&tracing::Level::ERROR);
        let expected_prefix = format!("{open}[mpv]{close} ");
        assert!(
            s.starts_with(&expected_prefix),
            "expected colored prefix in: {s:?}"
        );
    }

    #[test]
    fn ansi_for_each_level_distinct() {
        let palette = [
            tracing::Level::ERROR,
            tracing::Level::WARN,
            tracing::Level::INFO,
            tracing::Level::DEBUG,
            tracing::Level::TRACE,
        ];
        let mut seen = std::collections::HashSet::new();
        for l in palette {
            let (open, _) = ansi_for(&l);
            assert!(open.starts_with("\x1b["), "missing ANSI escape: {open:?}");
            assert!(seen.insert(open), "duplicate color for {l:?}");
        }
    }
    // --- process lifecycle -------------------------------------------------

    /// `STATE` and the global tracing dispatcher are process-wide, so the two
    /// lifecycle tests below serialise and each leaves the logger shut down.
    static LIFECYCLE: StdMutex<()> = StdMutex::new(());

    #[test]
    fn log_init_opens_the_named_file_and_shutdown_clears_the_active_path() {
        let _g = LIFECYCLE.lock();
        jfn_log_shutdown();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("astrofin.log");
        let path_str = path.to_string_lossy().into_owned();

        jfn_log_init(&path_str, "debug");
        assert_eq!(active_path(), path_str);
        assert!(path.exists(), "the log file is created at init");
        log(CATEGORY_CEF, LEVEL_INFO, "hello from the lifecycle test");

        jfn_log_shutdown();
        assert_eq!(
            active_path(),
            "",
            "the active path is cleared once the workers are flushed"
        );
    }

    #[test]
    fn a_second_init_is_ignored_while_one_is_active() {
        let _g = LIFECYCLE.lock();
        jfn_log_shutdown();
        let dir = tempfile::tempdir().expect("tempdir");
        let first = dir.path().join("first.log").to_string_lossy().into_owned();
        let second = dir.path().join("second.log").to_string_lossy().into_owned();

        jfn_log_init(&first, "");
        jfn_log_init(&second, "trace");
        assert_eq!(
            active_path(),
            first,
            "the dispatcher is already installed; the second init is a no-op"
        );
        assert!(
            !dir.path().join("second.log").exists(),
            "the second init must not open a file either"
        );
        jfn_log_shutdown();
    }

    #[test]
    fn shutting_down_twice_is_harmless() {
        let _g = LIFECYCLE.lock();
        jfn_log_shutdown();
        jfn_log_shutdown();
        assert_eq!(active_path(), "");
    }

    #[test]
    fn log_init_falls_back_to_info_for_a_blank_or_unparseable_filter() {
        // Both are normalised inside init; the observable effect is that init
        // still completes and installs state rather than bailing.
        let _g = LIFECYCLE.lock();
        jfn_log_shutdown();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("blank.log").to_string_lossy().into_owned();
        jfn_log_init(&path, "   ");
        assert_eq!(active_path(), path);
        jfn_log_shutdown();

        let path = dir.path().join("bogus.log").to_string_lossy().into_owned();
        jfn_log_init(&path, "=not a filter=");
        assert_eq!(active_path(), path);
        jfn_log_shutdown();
    }

    #[test]
    fn log_init_with_an_empty_path_logs_to_the_console_only() {
        let _g = LIFECYCLE.lock();
        jfn_log_shutdown();
        jfn_log_init("", "info");
        assert_eq!(active_path(), "");
        jfn_log_shutdown();
    }

    // --- writer plumbing ---------------------------------------------------

    #[test]
    fn rotating_file_flush_pushes_the_bytes_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("flush.log");
        let mut rf = RotatingFile::open(path.clone(), MAX_FILE_BYTES, MAX_BACKUPS)
            .expect("open rotating file");
        rf.write_all(b"one record\n").expect("write");
        rf.flush().expect("flush");
        let read = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(read, "one record\n");
    }

    #[test]
    fn a_redact_guard_forwards_nothing_until_it_is_dropped() {
        let sink = VecSink(Arc::new(StdMutex::new(Vec::new())));
        let make = RedactMake(sink.clone());
        {
            let mut w = make.make_writer();
            w.write_all(b"a partial ").expect("write");
            w.write_all(b"record\n").expect("write");
            // `flush` is a no-op: the whole record must be censored as one
            // buffer, so nothing may leave before Drop.
            w.flush().expect("flush");
            assert!(sink.0.lock().is_empty(), "nothing forwarded before drop");
        }
        assert_eq!(&*sink.0.lock(), b"a partial record\n");
    }

    #[test]
    fn msg_visitor_records_only_the_message_field() {
        let out = emit_through_layer("visitor payload", false);
        assert!(out.contains("visitor payload"), "{out}");
    }
}
