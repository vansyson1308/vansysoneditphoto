//! Client for the desktop app's JSON-lines control protocol
//! (`docs/control-protocol.md`): one JSON request per line, replies matched
//! by `id`. Keeps one connection, reconnecting after a failure or a call
//! dropped before its reply — or before a call, once the connection has been
//! idle long enough that the app (which closes idle control connections after
//! [`IO_TIMEOUT`](crate::security::IO_TIMEOUT)) may already have closed it.
//!
//! Each connection authenticates with the challenge handshake (NV-6): the client connects to
//! the literal loopback address it was given (never a name lookup that may pick another
//! listener) and the server must prove it holds the control token before the client sends its
//! own proof. The token itself is never sent, so a process that took the port gets nothing.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::AutomationError;
use crate::budgets::MAX_RESPONSE_BYTES;
use crate::security::{
    AUTH_METHOD, CHALLENGE_METHOD, IO_TIMEOUT, MAX_REQUEST_BYTES, ProofRole, generate_nonce, handshake_proof, is_hex256, proof_matches, validate_token,
};

type Conn = (BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf);

/// Reconnect before sending when the cached connection has been idle this long: the app
/// drops idle control connections after [`IO_TIMEOUT`], and reconnecting before the send
/// keeps the request out of the "was it applied?" state. The margin covers a request
/// still in flight when the idle clock runs out.
const IDLE_RECONNECT: Duration = IO_TIMEOUT.saturating_sub(Duration::from_secs(5));

/// The default call timeout sits strictly below the app's 60s reply deadline, so this
/// client's error — not the app's deadline reply, not the caller's own cutoff — is the
/// one the caller sees.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(55);

const NO_REPLY_CAUSES: &str =
    "the command may still be running, or the app was not drawing frames (display asleep, window fully occluded, or a long operation blocked the frame loop)";

/// Starts the desktop app when the bridge finds nothing listening (`--start-app-if-needed`).
/// Runs on a blocking thread; returns once the app was launched (not necessarily listening).
pub type AppStarter = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// Where the control token comes from.
enum TokenSource {
    /// Given on the command line or in the environment.
    Literal(String),
    /// The app's token file, read on the first connection and again after a refused handshake:
    /// the app writes it when it starts, which may be after this server started (launch order).
    File { path: PathBuf, cached: std::sync::Mutex<Option<String>> },
}

pub struct BridgeClient {
    addr: String,
    /// Where to connect: `addr` parsed, with `localhost` taken as 127.0.0.1.
    socket: SocketAddr,
    token: TokenSource,
    /// What to tell a caller when the app is not running (the launcher's path, when known).
    hint: Option<String>,
    starter: Option<AppStarter>,
    conn: Mutex<Option<(Conn, Instant)>>,
    next_id: AtomicU64,
    timeout: Duration,
    idle_reconnect: Duration,
}

impl BridgeClient {
    /// `addr` such as `127.0.0.1:7878`. Only loopback addresses are accepted,
    /// matching the server, which binds to loopback only.
    pub fn new(addr: impl Into<String>, token: impl Into<String>) -> Result<Self, AutomationError> {
        let addr = addr.into();
        let token = token.into();
        validate_token(&token)?;
        Self::with_source(addr, TokenSource::Literal(token.to_ascii_lowercase()))
    }

    /// A client that reads the control token from `path` on its first connection, not now: the
    /// MCP server may start before the app has written the file. A missing file or a stopped app
    /// is reported on the call that needs them, as "MagicPic app is not running".
    pub fn with_token_file(addr: impl Into<String>, path: impl Into<PathBuf>) -> Result<Self, AutomationError> {
        Self::with_source(addr.into(), TokenSource::File { path: path.into(), cached: std::sync::Mutex::new(None) })
    }

    fn with_source(addr: String, token: TokenSource) -> Result<Self, AutomationError> {
        let socket = loopback_socket(&addr)?;
        Ok(BridgeClient {
            addr,
            socket,
            token,
            hint: None,
            starter: None,
            conn: Mutex::new(None),
            next_id: AtomicU64::new(1),
            timeout: DEFAULT_CALL_TIMEOUT,
            idle_reconnect: IDLE_RECONNECT,
        })
    }

    /// How to start the app, added to "not running" errors (e.g. the launcher's path).
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Start the app when nothing listens on the control port, then wait for it to listen.
    pub fn with_starter(mut self, starter: AppStarter) -> Self {
        self.starter = Some(starter);
        self
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Shrink the idle-reconnect threshold; tests use it to run the reconnect path quickly.
    pub fn with_idle_reconnect(mut self, t: Duration) -> Self {
        self.idle_reconnect = t;
        self
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Call a control method; returns its `result` or the app's error. Never resends after
    /// a transport failure: the request may have been applied (#1007).
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        self.dispatch(method, &params, false).await
    }

    /// [`Self::call`] for a method the caller knows only reads state. A transport failure
    /// applied nothing, so one fresh attempt is made on a new connection.
    pub async fn call_read(&self, method: &str, params: Value) -> Result<Value, AutomationError> {
        self.dispatch(method, &params, true).await
    }

    async fn dispatch(&self, method: &str, params: &Value, read: bool) -> Result<Value, AutomationError> {
        match self.attempt(method, params).await {
            Ok(v) => Ok(v),
            // A dropped socket or an unsent request applied nothing: safe to retry once,
            // and only for a call the caller vouched is a read.
            Err(Failure::NotSent(_)) | Err(Failure::Transport(_)) if read => match self.attempt(method, params).await {
                Ok(v) => Ok(v),
                Err(retry) => Err(self.failure(method, retry, read)),
            },
            Err(f) => Err(self.failure(method, f, read)),
        }
    }

    /// The message for a failed attempt, worded by what the failure proves.
    fn failure(&self, method: &str, f: Failure, read: bool) -> AutomationError {
        match f {
            Failure::NotSent(e) => AutomationError::Bridge(format!("`{method}` failed: {e}; the request was not sent")),
            Failure::Transport(e) => AutomationError::Bridge(format!("`{method}` failed: {e}; operation may have completed; inspect state before retrying")),
            Failure::TimedOut(_) => {
                let what = if read { "a read may be retried safely" } else { "operation may have completed; inspect state before retrying" };
                AutomationError::Bridge(format!("`{method}` timed out after {:?}; {NO_REPLY_CAUSES}; {what}", self.timeout))
            }
            Failure::Definite(e) => e,
        }
    }

    /// One exchange over one connection, reconnecting first when the cached one may be dead.
    async fn attempt(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        let mut guard = self.conn.lock().await;
        if let Some((_, last_used)) = guard.as_ref()
            && last_used.elapsed() >= self.idle_reconnect
        {
            // The request below goes to a fresh socket, not one the app may have dropped.
            *guard = None;
        }
        if guard.is_none() {
            *guard = Some((self.connect().await?, Instant::now()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // Out of the cache until answered: a call dropped mid-exchange (a cancelled wait) closes
        // its connection, so no later call shares a socket with a half-sent request or a reply
        // the app is still working on.
        let Some((mut conn, _)) = guard.take() else {
            return Err(Failure::NotSent(format!("not connected to {}", self.addr)));
        };
        let outcome = tokio::time::timeout(self.timeout, exchange(&mut conn, id, method, params)).await;
        match outcome {
            // An oversized frame leaves unread bytes. Drop this connection and
            // report the budget failure without retrying a possibly completed edit.
            Ok(Ok(Err(error @ AutomationError::BadRequest(_)))) => Err(Failure::Definite(error)),
            Ok(Ok(Ok(v))) => {
                *guard = Some((conn, Instant::now()));
                Ok(v)
            }
            Ok(Ok(Err(e))) => {
                // The app answered: the operation did not apply — except the app's own
                // deadline reply, which fires while the command may still be running.
                *guard = Some((conn, Instant::now()));
                Err(Failure::Definite(e))
            }
            // A failed write or lost reply does not prove the edit was not applied.
            // Reconnect on the next call; only a vouched read may resend (#1007).
            Ok(Err(e)) => Err(Failure::Transport(e.to_string())),
            Err(_) => Err(Failure::TimedOut(())),
        }
    }
}

impl BridgeClient {
    /// "MagicPic app is not running" with how to start it.
    fn not_running(&self, why: &str) -> String {
        let how =
            self.hint.as_deref().unwrap_or("start it with the launcher (`magicpic-cli launch`), or `magicpic --control <port> --control-token-file <path>`");
        format!("MagicPic app is not running on {} ({why}): {how}", self.addr)
    }

    async fn tcp(&self) -> Result<TcpStream, Failure> {
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(self.socket)).await {
            Ok(Ok(s)) => Ok(s),
            Ok(Err(e)) => Err(Failure::NotSent(self.not_running(&e.to_string()))),
            Err(_) => Err(Failure::NotSent(format!("timed out connecting to {}", self.addr))),
        }
    }

    /// A fresh, authenticated connection. Starts the app first when a starter is set and nothing
    /// listens; re-reads a token file once when the app refuses the token it held.
    async fn connect(&self) -> Result<Conn, Failure> {
        let stream = match self.tcp().await {
            Ok(s) => s,
            Err(refused) => {
                let Some(starter) = self.starter.clone() else { return Err(refused) };
                tokio::task::spawn_blocking(move || starter())
                    .await
                    .map_err(|e| Failure::NotSent(e.to_string()))?
                    .map_err(|e| Failure::NotSent(format!("could not start the MagicPic app: {e}")))?;
                self.wait_listening().await?
            }
        };
        let fresh = self.token_value(false)?;
        match self.handshake(stream, &fresh).await? {
            Ok(conn) => Ok(conn),
            // The app may have restarted with a new token since the file was read.
            Err(e) if matches!(self.token, TokenSource::File { .. }) => {
                let reread = self.token_value(true)?;
                if reread == fresh {
                    return Err(Failure::Definite(e));
                }
                self.handshake(self.tcp().await?, &reread).await?.map_err(Failure::Definite)
            }
            Err(e) => Err(Failure::Definite(e)),
        }
    }

    /// After a start: wait (up to 30 s) for the app to listen and write its token.
    async fn wait_listening(&self) -> Result<TcpStream, Failure> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match self.tcp().await {
                Ok(s) if self.token_value(true).is_ok() => return Ok(s),
                Ok(_) | Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(250)).await,
                Ok(_) => return Err(Failure::NotSent(self.not_running("it was started but wrote no control token within 30 s"))),
                Err(e) => return Err(e),
            }
        }
    }

    /// The token to authenticate with; `reload` reads a token file again.
    fn token_value(&self, reload: bool) -> Result<String, Failure> {
        match &self.token {
            TokenSource::Literal(t) => Ok(t.clone()),
            TokenSource::File { path, cached } => {
                let mut cached = cached.lock().unwrap_or_else(PoisonError::into_inner);
                if !reload && let Some(t) = cached.as_ref() {
                    return Ok(t.clone());
                }
                if !path.exists() {
                    return Err(Failure::NotSent(self.not_running(&format!("its control token file {} does not exist yet", path.display()))));
                }
                let token = crate::security::client_token(None, Some(path)).map_err(|e| Failure::NotSent(e.to_string()))?;
                *cached = Some(token.clone());
                Ok(token)
            }
        }
    }

    /// Outer `Err`: nothing was sent; inner: the app refused.
    async fn handshake(&self, stream: TcpStream, token: &str) -> Result<Result<Conn, AutomationError>, Failure> {
        let (r, w) = stream.into_split();
        let mut conn = (BufReader::new(r), w);
        let auth = tokio::time::timeout(Duration::from_secs(5), self.authenticate(&mut conn, token))
            .await
            .map_err(|_| Failure::NotSent("control authentication timed out".into()))?
            .map_err(|e| Failure::NotSent(e.to_string()))?;
        Ok(auth.map(|()| conn))
    }

    /// Challenge authentication on a fresh connection. The server must prove it holds the token
    /// before the client sends its own proof; the token itself is never sent (NV-6).
    /// Outer `Err` = transport failure; inner = refused.
    async fn authenticate(&self, conn: &mut Conn, token: &str) -> Result<Result<(), AutomationError>, AutomationError> {
        let client_nonce = generate_nonce()?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let challenge = match exchange(conn, id, CHALLENGE_METHOD, &json!({"clientNonce": client_nonce})).await? {
            Ok(challenge) => challenge,
            Err(AutomationError::App(e)) => {
                return Ok(Err(AutomationError::Bridge(format!(
                    "{} refused challenge authentication ({e}): it is not MagicPic, or a MagicPic too old to support it (update the app); the control token was not sent",
                    self.addr
                ))));
            }
            Err(e) => return Ok(Err(e)),
        };
        let field = |key: &str| challenge.get(key).and_then(Value::as_str).filter(|v| is_hex256(v));
        let server_nonce = match (field("serverNonce"), field("serverProof")) {
            (Some(nonce), Some(proof)) if proof_matches(token, ProofRole::Server, &client_nonce, nonce, proof) => nonce,
            _ => {
                return Ok(Err(AutomationError::Bridge(format!(
                    "{} did not prove it holds this control token: the token does not match the app's, or the listener is not that MagicPic app; the control token was not sent",
                    self.addr
                ))));
            }
        };
        let proof = handshake_proof(token, ProofRole::Client, &client_nonce, server_nonce)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        Ok(exchange(conn, id, AUTH_METHOD, &json!({"proof": proof})).await?.map(|_| ()))
    }
}

/// The literal loopback socket address for `addr`: `127.0.0.1:<port>` or `[::1]:<port>`, with
/// `localhost:<port>` meaning 127.0.0.1, the address the app binds. The name is never looked up,
/// since a lookup may return `[::1]` first and reach another process listening there (NV-6).
fn loopback_socket(addr: &str) -> Result<SocketAddr, AutomationError> {
    let refuse = || AutomationError::BadRequest(format!("bridge address must be loopback `127.0.0.1:<port>` or `[::1]:<port>`, got `{addr}`"));
    let socket = match addr.strip_prefix("localhost:") {
        Some(port) => SocketAddr::from((Ipv4Addr::LOCALHOST, port.parse::<u16>().map_err(|_| refuse())?)),
        None => addr.parse::<SocketAddr>().map_err(|_| refuse())?,
    };
    if socket.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) || socket.ip() == IpAddr::V6(Ipv6Addr::LOCALHOST) { Ok(socket) } else { Err(refuse()) }
}

/// How a single attempt failed, deciding the retry and the wording above.
enum Failure {
    /// The connection could not be established or authenticated: nothing was sent.
    NotSent(String),
    /// The request was sent and the reply was lost: the outcome is uncertain.
    Transport(String),
    /// The call's own timeout elapsed. Retrying would only double the wait.
    TimedOut(()),
    /// The app answered (ok or error) or the request was refused before it was sent.
    Definite(AutomationError),
}

/// Outer `Err` = transport failure with an uncertain outcome; inner = app-level result.
async fn exchange(conn: &mut Conn, id: u64, method: &str, params: &Value) -> Result<Result<Value, AutomationError>, AutomationError> {
    let mut line = serde_json::to_string(&json!({"id": id, "method": method, "params": params})).map_err(|e| AutomationError::Other(e.to_string()))?;
    line.push('\n');
    if line.len() > MAX_REQUEST_BYTES {
        return Ok(Err(AutomationError::BadRequest(format!("request is {} bytes; maximum is {MAX_REQUEST_BYTES}", line.len()))));
    }
    let io = |e: std::io::Error| AutomationError::Bridge(e.to_string());
    conn.1.write_all(line.as_bytes()).await.map_err(io)?;
    conn.1.flush().await.map_err(io)?;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // Read raw bytes so truncation inside a UTF-8 character still reports the
        // budget error instead of a decoding/transport error.
        let n = (&mut conn.0).take((MAX_RESPONSE_BYTES + 1) as u64).read_until(b'\n', &mut buf).await.map_err(io)?;
        if n > MAX_RESPONSE_BYTES {
            return Ok(Err(AutomationError::BadRequest(format!("bridge response exceeds {MAX_RESPONSE_BYTES} bytes; operation may have completed"))));
        }
        if n == 0 {
            return Err(AutomationError::Bridge("connection closed by the app".into()));
        }
        let Ok(v) = serde_json::from_slice::<Value>(&buf) else {
            continue;
        };
        if v.get("id").and_then(Value::as_u64) != Some(id) {
            continue; // stale reply from an earlier, timed-out request
        }
        return Ok(reply_result(&v));
    }
}

/// An app reply as the caller's result. A failed reply can still carry a `result`: a cancelled
/// Batch lists the outputs it already wrote, so the error keeps it rather than dropping it.
fn reply_result(v: &Value) -> Result<Value, AutomationError> {
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    }
    let error = v.get("error").and_then(Value::as_str).unwrap_or("unknown error");
    Err(AutomationError::App(match v.get("result") {
        Some(result) if !result.is_null() => format!("{error}; partial result: {result}"),
        _ => error.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_reply_keeps_its_partial_result() {
        assert_eq!(reply_result(&json!({"ok": true, "result": {"files": ["a.png"]}})).unwrap(), json!({"files": ["a.png"]}));
        let Err(AutomationError::App(message)) = reply_result(&json!({"ok": false, "error": "cancelled", "result": {"files": ["a.png"], "cancelled": true}}))
        else {
            panic!("a failed reply is an app error")
        };
        assert!(message.starts_with("cancelled; partial result: ") && message.contains("a.png"), "{message}");
        let Err(AutomationError::App(message)) = reply_result(&json!({"ok": false, "error": "cancelled"})) else { panic!("a failed reply is an app error") };
        assert_eq!(message, "cancelled");
    }

    #[test]
    fn the_call_timeout_stays_below_the_app_reply_deadline() {
        // At or above the app's 60s deadline, callers would see their own cutoff first.
        assert!(DEFAULT_CALL_TIMEOUT < Duration::from_secs(60));
    }

    /// NV-6: `localhost` is the IPv4 loopback the app binds, never a name lookup.
    #[test]
    fn bridge_addresses_are_literal_loopback_sockets() {
        assert_eq!(loopback_socket("localhost:7878").unwrap(), SocketAddr::from((Ipv4Addr::LOCALHOST, 7878)));
        assert_eq!(loopback_socket("127.0.0.1:7878").unwrap(), SocketAddr::from((Ipv4Addr::LOCALHOST, 7878)));
        assert_eq!(loopback_socket("[::1]:7878").unwrap(), SocketAddr::from((Ipv6Addr::LOCALHOST, 7878)));
        for bad in ["10.0.0.5:7878", "127.0.0.2:7878", "localhost", "localhost:", "localhost:99999", "::1:7878", "example.com:7878", "127.0.0.1"] {
            assert!(loopback_socket(bad).is_err(), "{bad}");
        }
    }

    /// A fake app that serves `session.inspect` on `listener`, authenticating with whatever token
    /// `token` holds when each connection arrives.
    fn fake_app(listener: tokio::net::TcpListener, token: Arc<std::sync::Mutex<String>>) -> tokio::task::JoinHandle<()> {
        use crate::security::{AuthStep, ServerHandshake};
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let token = token.lock().unwrap().clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    let mut handshake = ServerHandshake::new();
                    let mut authed = false;
                    while let Ok(Some(line)) = lines.next_line().await {
                        if !authed {
                            let (reply, step) = handshake.reply(&line, &token);
                            write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                            match step {
                                AuthStep::Authenticated => authed = true,
                                AuthStep::Challenged => {}
                                AuthStep::Rejected => return,
                            }
                            continue;
                        }
                        let request: Value = serde_json::from_str(&line).unwrap();
                        let reply = json!({"id": request["id"], "ok": true, "result": {"documents": []}});
                        write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
                    }
                });
            }
        })
    }

    /// Launch order (setup guide P0.6): the MCP server starts before the app. The token file is
    /// read on the first call, a missing one says the app is not running, and a token that
    /// changed when the app restarted is read again instead of failing every call.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_token_file_is_read_lazily_and_again_after_the_app_restarts() {
        const FIRST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        const SECOND: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
        let dir = tempfile_dir();
        let token_file = dir.join("control.token");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        // Constructing the client reads nothing: the file does not exist yet.
        let client = BridgeClient::with_token_file(addr.clone(), &token_file).unwrap().with_hint("run the launcher /x/start-magicpic.cmd");
        let Err(AutomationError::Bridge(e)) = client.call_read("engine.execute", json!({})).await else { panic!("no app, no token") };
        assert!(e.contains("MagicPic app is not running") && e.contains("does not exist yet") && e.contains("/x/start-magicpic.cmd"), "{e}");

        let token = Arc::new(std::sync::Mutex::new(FIRST.to_string()));
        write_token(&token_file, FIRST);
        let app = fake_app(listener, token.clone());
        assert_eq!(client.call_read("engine.execute", json!({})).await.unwrap(), json!({"documents": []}));

        // The app restarts with a new token; the cached connection is gone.
        *token.lock().unwrap() = SECOND.to_string();
        write_token(&token_file, SECOND);
        *client.conn.lock().await = None;
        assert_eq!(client.call_read("engine.execute", json!({})).await.unwrap(), json!({"documents": []}), "re-read after a refused handshake");
        app.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--start-app-if-needed`: nothing listens, so the starter runs once, and the call waits for
    /// the app to listen and write its token.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_starter_launches_the_app_when_nothing_listens() {
        const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let dir = tempfile_dir();
        let token_file = dir.join("control.token");
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let starts = Arc::new(AtomicU64::new(0));
        let (counted, file, runtime) = (starts.clone(), token_file.clone(), tokio::runtime::Handle::current());
        let starter: AppStarter = Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let file = file.clone();
            // The "app": listens a moment later, then writes its token.
            runtime.spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
                write_token(&file, TOKEN);
                fake_app(listener, Arc::new(std::sync::Mutex::new(TOKEN.to_string()))).await.unwrap();
            });
            Ok(())
        });
        let client = BridgeClient::with_token_file(addr.to_string(), &token_file).unwrap().with_starter(starter);
        assert_eq!(client.call("engine.execute", json!({})).await.unwrap(), json!({"documents": []}));
        assert_eq!(client.call("engine.execute", json!({})).await.unwrap(), json!({"documents": []}));
        assert_eq!(starts.load(Ordering::SeqCst), 1, "started once, then reused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nothing_listening_reads_as_the_app_not_running() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        drop(listener);
        let client = BridgeClient::new(addr, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap();
        let Err(AutomationError::Bridge(e)) = client.call("engine.execute", json!({})).await else { panic!("nothing listens") };
        assert!(e.contains("MagicPic app is not running") && e.contains("was not sent"), "{e}");
    }

    /// As the app writes it: owner-only (the reader refuses a file others can read).
    fn write_token(path: &std::path::Path, token: &str) {
        std::fs::write(path, token).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("magicpic-bridge-{}-{}", std::process::id(), generate_nonce().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn idle_reconnect_stays_below_the_app_idle_cutoff() {
        // Reconnecting later than IO_TIMEOUT would send into a socket the app dropped.
        assert!(IDLE_RECONNECT < IO_TIMEOUT);
    }
}
