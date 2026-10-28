//! Shared harness for the end-to-end tests: the accessible-mode line
//! protocol (spawn the built binary with a piped stdin/stdout) and a
//! portable-pty session that accumulates terminal output and lets a test wait
//! for expected substrings (the Go e2e `runJSE`/`waitFrame` helpers).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Warning glyph (icons::WARN in the binary; tests cannot import the crate).
pub const WARN: &str = "\u{f071}";
/// Validation-error glyph (icons::ERR).
pub const ERR: &str = "\u{f057}";

/// The binary cargo built for this test run.
pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_jse")
}

/// Repository root: the Go tests run the binary with `cmd.Dir = ".."`, which
/// is the crate root here.
pub fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

pub fn write_file(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

pub fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn read_json(path: &Path) -> serde_json::Value {
    let data = read_text(path);
    serde_json::from_str(&data).unwrap_or_else(|e| panic!("parse {}: {e}\n{data}", path.display()))
}

/// Top-level object keys in document order (the Go test used jsondoc.Parse;
/// tests cannot import the binary crate, so scan the raw text instead). Only
/// plain JSON produced by the tool is passed here, but strings and escapes
/// are honored so a key containing braces cannot confuse the depth counter.
pub fn top_level_keys(path: &Path) -> Vec<String> {
    let text = read_text(path);
    let b = text.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() {
                    if b[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if b[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                if depth == 1 {
                    let mut k = j + 1;
                    while k < b.len() && (b[k] as char).is_ascii_whitespace() {
                        k += 1;
                    }
                    if k < b.len() && b[k] == b':' {
                        keys.push(text[start..j].to_string());
                    }
                }
                i = j + 1;
            }
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    keys
}

/// Remove ANSI terminal control sequences so assertions read the rendered
/// text. The Go suite matched raw pty bytes because its renderer styles whole
/// values; the Rust renderer styles keys/prefixes separately, so an exact
/// string like "> $schema" only exists after stripping.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameters then a final byte in 0x40..=0x7e
                for c2 in chars.by_ref() {
                    if ('@'..='~').contains(&c2) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // OSC: terminated by BEL or ST (ESC \)
                while let Some(c2) = chars.next() {
                    if c2 == '\x07' {
                        break;
                    }
                    if c2 == '\x1b' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            Some('(') | Some(')') => {
                chars.next();
                chars.next(); // character-set designation
            }
            _ => {
                chars.next(); // two-byte escape
            }
        }
    }
    out
}

// --- accessible mode: run the binary with piped stdio -----------------------

pub struct RunOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// runJSE: run against the crate root with an isolated schema cache unless
/// the caller overrides it.
pub fn run_jse(stdin: &str, args: &[&str]) -> RunOutput {
    let cache = tempfile::tempdir().expect("cache tempdir");
    let cache_dir = cache.path().to_string_lossy().into_owned();
    run_jse_env(
        repo_root(),
        stdin,
        args,
        &[("JSE_CACHE_DIR", cache_dir.as_str())],
        Duration::from_secs(60),
    )
}

/// runJSEIn: explicit working directory plus extra environment.
pub fn run_jse_env(
    cwd: &Path,
    stdin: &str,
    args: &[&str],
    envs: &[(&str, &str)],
    timeout: Duration,
) -> RunOutput {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().unwrap_or_else(|e| panic!("spawn jse: {e}"));

    let mut sin = child.stdin.take().expect("stdin pipe");
    let input = stdin.as_bytes().to_vec();
    let input_thread = thread::spawn(move || {
        let _ = sin.write_all(&input);
        // dropping sin closes the pipe (EOF)
    });

    let mut out = child.stdout.take().expect("stdout pipe");
    let mut err = child.stderr.take().expect("stderr pipe");
    let out_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut code = -1;
    loop {
        match child.try_wait().expect("wait jse") {
            Some(st) => {
                code = st.code().unwrap_or(-1);
                break;
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    let _ = input_thread.join();
    let stdout = String::from_utf8_lossy(&out_thread.join().expect("stdout thread")).into_owned();
    let stderr = String::from_utf8_lossy(&err_thread.join().expect("stderr thread")).into_owned();
    if timed_out {
        panic!("jse timed out after {timeout:?}; stdout so far:\n{stdout}");
    }
    RunOutput {
        stdout,
        stderr,
        code,
    }
}

// --- pty session ------------------------------------------------------------

type Shared = Arc<(Mutex<Vec<u8>>, Condvar)>;

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    state: Shared,
    eof: Arc<AtomicBool>,
    exited: bool,
}

/// No pty available (e.g. a restricted container): tests skip like Go's
/// `t.Skipf("no pty available: ...")`.
pub enum PtySpawn {
    Ok(PtySession),
    Unavailable(String),
}

impl PtySession {
    pub fn spawn(cwd: &Path, args: &[&str], envs: &[(&str, &str)]) -> PtySpawn {
        PtySession::spawn_sized(cwd, args, envs, 120, 30)
    }

    /// spawn_sized starts the binary on a terminal of the given size; the
    /// narrow-width flows need fewer columns than the default 120.
    pub fn spawn_sized(
        cwd: &Path,
        args: &[&str],
        envs: &[(&str, &str)],
        cols: u16,
        rows: u16,
    ) -> PtySpawn {
        let pty = native_pty_system();
        let pair = match pty.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => return PtySpawn::Unavailable(format!("openpty: {e}")),
        };
        let mut cmd = CommandBuilder::new(bin());
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => return PtySpawn::Unavailable(format!("spawn: {e}")),
        };
        drop(pair.slave);
        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => return PtySpawn::Unavailable(format!("reader: {e}")),
        };
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => return PtySpawn::Unavailable(format!("writer: {e}")),
        };
        let state: Shared = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let eof = Arc::new(AtomicBool::new(false));
        {
            let state = Arc::clone(&state);
            let eof = Arc::clone(&eof);
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let (lock, cv) = &*state;
                            lock.lock().extend_from_slice(&buf[..n]);
                            cv.notify_all();
                        }
                        Err(_) => break,
                    }
                }
                eof.store(true, Ordering::SeqCst);
                let (_lock, cv) = &*state;
                cv.notify_all();
            });
        }
        PtySpawn::Ok(PtySession {
            master: pair.master,
            writer,
            child,
            state,
            eof,
            exited: false,
        })
    }

    pub fn snapshot(&self) -> String {
        let (lock, _cv) = &*self.state;
        let buf = lock.lock();
        strip_ansi(&String::from_utf8_lossy(&buf))
    }

    /// waitFrame: block until every `want` appears in output written after
    /// `offset`; returns the whole accumulated output and its length.
    pub fn wait_frame(&mut self, what: &str, offset: usize, want: &[&str]) -> (String, usize) {
        self.wait_frame_for(what, offset, want, Duration::from_secs(15))
    }

    pub fn wait_frame_for(
        &mut self,
        what: &str,
        offset: usize,
        want: &[&str],
        timeout: Duration,
    ) -> (String, usize) {
        let deadline = Instant::now() + timeout;
        let (lock, cv) = &*self.state;
        let mut buf = lock.lock();
        loop {
            let snap = strip_ansi(&String::from_utf8_lossy(&buf));
            let tail = snap.get(offset..).unwrap_or("");
            if want.iter().all(|w| tail.contains(w)) {
                let len = snap.len();
                return (snap, len);
            }
            if self.eof.load(Ordering::SeqCst) {
                panic!("pty closed while waiting for {what}; output so far:\n{snap}");
            }
            let now = Instant::now();
            if now >= deadline {
                let tail = snap.get(snap.len().saturating_sub(4000)..).unwrap_or("");
                panic!("timed out waiting for {what}; last output:\n{tail}");
            }
            // Short slices: a notification that lands between the scan above
            // and the wait would otherwise be lost until the 15s deadline.
            cv.wait_for(&mut buf, (deadline - now).min(Duration::from_millis(50)));
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .unwrap_or_else(|e| panic!("pty write: {e}"));
        self.writer.flush().ok();
    }

    pub fn wait_exit(&mut self, want: i32) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match self.child.try_wait().expect("wait pty child") {
                Some(status) => {
                    self.exited = true;
                    let code = status.exit_code() as i32;
                    assert_eq!(code, want, "exit code");
                    return;
                }
                None => {
                    if Instant::now() >= deadline {
                        let _ = self.child.kill();
                        panic!("timed out waiting for exit");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// sleep a fixed interval while the reader thread keeps accumulating
    /// output (Go's `time.Sleep` between scripted keystrokes).
    pub fn pause(&self, d: Duration) {
        thread::sleep(d);
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if !self.exited {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = &self.master; // keep the master alive until the child is gone
    }
}

// --- tiny_http fixtures -----------------------------------------------------

use tiny_http::{Header, Response, Server};

/// A local HTTP server serving one fixed body for every request; the URL is
/// known after construction (httptest.NewServer equivalent).
pub struct HttpServer {
    pub url: String,
    server: Arc<Server>,
}

impl HttpServer {
    pub fn start(body: Vec<u8>) -> HttpServer {
        let server =
            Arc::new(Server::http("127.0.0.1:0").unwrap_or_else(|e| panic!("start http: {e}")));
        let url = format!("http://{}", server.server_addr());
        let s = Arc::clone(&server);
        thread::spawn(move || {
            for request in s.incoming_requests() {
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("header");
                let resp = Response::from_data(body.clone()).with_header(header);
                let _ = request.respond(resp);
            }
        });
        HttpServer { url, server }
    }

    /// Path handler for the catalog fixture: unknown paths get `default`.
    pub fn start_routes(
        routes: std::collections::HashMap<String, Vec<u8>>,
        default: Vec<u8>,
    ) -> (String, Arc<Server>) {
        let server =
            Arc::new(Server::http("127.0.0.1:0").unwrap_or_else(|e| panic!("start http: {e}")));
        let url = format!("http://{}", server.server_addr());
        let s = Arc::clone(&server);
        thread::spawn(move || {
            for request in s.incoming_requests() {
                let key = request.url().split('?').next().unwrap_or("").to_string();
                let body = routes.get(&key).cloned().unwrap_or_else(|| default.clone());
                let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .expect("header");
                let resp = Response::from_data(body).with_header(header);
                let _ = request.respond(resp);
            }
        });
        (url, server)
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.server.unblock();
    }
}
