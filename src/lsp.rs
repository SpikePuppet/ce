use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use url::Url;
use winit::event_loop::EventLoopProxy;

const INITIALIZE_REQUEST_ID: u64 = 1;
const SHUTDOWN_REQUEST_ID: u64 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LspDocument {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct DiagnosticUpdate {
    pub path: PathBuf,
    pub version: Option<i64>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug)]
pub enum LspEvent {
    Initialized,
    Diagnostics(DiagnosticUpdate),
    ServerStopped(String),
}

#[derive(Debug)]
pub enum LspStartError {
    NotInstalled,
    Spawn(io::Error),
    InvalidRoot(PathBuf),
    MissingPipe(&'static str),
}

impl Display for LspStartError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => formatter.write_str(
                "pyright-langserver was not found in PATH. Install Pyright to enable Python diagnostics.",
            ),
            Self::Spawn(error) => write!(formatter, "could not start pyright-langserver: {error}"),
            Self::InvalidRoot(path) => {
                write!(formatter, "could not convert {} to a file URI", path.display())
            }
            Self::MissingPipe(name) => write!(formatter, "language server {name} pipe was unavailable"),
        }
    }
}

struct SyncedDocument {
    text: String,
    version: i64,
}

struct LspServer {
    child: Child,
    outbound: Sender<Value>,
    shutdown_complete: Receiver<()>,
}

impl LspServer {
    fn start(root: &Path, proxy: EventLoopProxy<LspEvent>) -> Result<Self, LspStartError> {
        let mut child = Command::new("pyright-langserver")
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    LspStartError::NotInstalled
                } else {
                    LspStartError::Spawn(error)
                }
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or(LspStartError::MissingPipe("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(LspStartError::MissingPipe("stdout"))?;
        let (outbound, receiver) = mpsc::channel::<Value>();
        let (shutdown_sender, shutdown_complete) = mpsc::channel();

        thread::Builder::new()
            .name("lsp-writer".to_owned())
            .spawn(move || {
                let mut writer = stdin;
                for message in receiver {
                    if write_message(&mut writer, &message).is_err() {
                        break;
                    }
                }
            })
            .map_err(LspStartError::Spawn)?;

        let responses = outbound.clone();
        thread::Builder::new()
            .name("lsp-reader".to_owned())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_message(&mut reader) {
                        Ok(Some(message)) => {
                            if message.get("id").and_then(Value::as_u64)
                                == Some(SHUTDOWN_REQUEST_ID)
                            {
                                let _ = shutdown_sender.send(());
                            } else {
                                handle_server_message(message, &responses, &proxy);
                            }
                        }
                        Ok(None) => {
                            let _ = proxy.send_event(LspEvent::ServerStopped(
                                "Pyright closed its output stream".to_owned(),
                            ));
                            break;
                        }
                        Err(error) => {
                            let _ = proxy.send_event(LspEvent::ServerStopped(format!(
                                "could not read Pyright output: {error}"
                            )));
                            break;
                        }
                    }
                }
            })
            .map_err(LspStartError::Spawn)?;

        let root_uri = Url::from_directory_path(root)
            .map_err(|()| LspStartError::InvalidRoot(root.to_path_buf()))?;
        outbound
            .send(json!({
                "jsonrpc": "2.0",
                "id": INITIALIZE_REQUEST_ID,
                "method": "initialize",
                "params": {
                    "processId": std::process::id(),
                    "clientInfo": { "name": "editor", "version": env!("CARGO_PKG_VERSION") },
                    "rootUri": root_uri.as_str(),
                    "capabilities": {
                        "textDocument": {
                            "publishDiagnostics": { "versionSupport": true }
                        }
                    }
                }
            }))
            .expect("new language-server writer must be connected");

        Ok(Self {
            child,
            outbound,
            shutdown_complete,
        })
    }

    fn send(&self, message: Value) {
        let _ = self.outbound.send(message);
    }
}

impl Drop for LspServer {
    fn drop(&mut self) {
        self.send(json!({
            "jsonrpc": "2.0",
            "id": SHUTDOWN_REQUEST_ID,
            "method": "shutdown",
            "params": null
        }));
        let _ = self
            .shutdown_complete
            .recv_timeout(Duration::from_millis(500));
        self.send(json!({ "jsonrpc": "2.0", "method": "exit", "params": null }));

        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct LspManager {
    proxy: EventLoopProxy<LspEvent>,
    server: Option<LspServer>,
    desired: HashMap<String, LspDocument>,
    synced: HashMap<String, SyncedDocument>,
    ready: bool,
    attempted_start: bool,
}

impl LspManager {
    pub fn new(proxy: EventLoopProxy<LspEvent>) -> Self {
        Self {
            proxy,
            server: None,
            desired: HashMap::new(),
            synced: HashMap::new(),
            ready: false,
            attempted_start: false,
        }
    }

    pub fn reconcile(&mut self, documents: Vec<LspDocument>) -> Result<(), LspStartError> {
        self.desired = documents
            .into_iter()
            .filter_map(|document| {
                let uri = Url::from_file_path(&document.path).ok()?.to_string();
                Some((uri, document))
            })
            .collect();

        if self.server.is_none() && !self.desired.is_empty() && !self.attempted_start {
            self.attempted_start = true;
            let first = self
                .desired
                .values()
                .next()
                .expect("non-empty desired document set");
            let root = project_root(&first.path);
            self.server = Some(LspServer::start(&root, self.proxy.clone())?);
        }
        if self.ready {
            self.sync_documents();
        }
        Ok(())
    }

    pub fn handle_event(&mut self, event: LspEvent) -> Option<DiagnosticUpdate> {
        match event {
            LspEvent::Initialized => {
                self.ready = true;
                if let Some(server) = &self.server {
                    server.send(json!({
                        "jsonrpc": "2.0",
                        "method": "initialized",
                        "params": {}
                    }));
                }
                self.sync_documents();
                None
            }
            LspEvent::Diagnostics(update) => {
                let uri = Url::from_file_path(&update.path).ok()?.to_string();
                let current = self.synced.get(&uri)?;
                if update
                    .version
                    .is_some_and(|version| version != current.version)
                {
                    None
                } else {
                    Some(update)
                }
            }
            LspEvent::ServerStopped(reason) => {
                eprintln!("language server stopped: {reason}");
                self.ready = false;
                self.server = None;
                self.synced.clear();
                self.attempted_start = false;
                None
            }
        }
    }

    pub fn did_save(&self, path: &Path) {
        if !self.ready {
            return;
        }
        let Ok(uri) = Url::from_file_path(path) else {
            return;
        };
        if self.synced.contains_key(uri.as_str())
            && let Some(server) = &self.server
        {
            server.send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didSave",
                "params": { "textDocument": { "uri": uri.as_str() } }
            }));
        }
    }

    fn sync_documents(&mut self) {
        let Some(server) = &self.server else {
            return;
        };
        let closed = self
            .synced
            .keys()
            .filter(|uri| !self.desired.contains_key(*uri))
            .cloned()
            .collect::<Vec<_>>();
        for uri in closed {
            server.send(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didClose",
                "params": { "textDocument": { "uri": uri } }
            }));
            self.synced.remove(&uri);
        }

        for (uri, document) in &self.desired {
            match self.synced.get_mut(uri) {
                None => {
                    server.send(json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didOpen",
                        "params": {
                            "textDocument": {
                                "uri": uri,
                                "languageId": "python",
                                "version": 1,
                                "text": document.text
                            }
                        }
                    }));
                    self.synced.insert(
                        uri.clone(),
                        SyncedDocument {
                            text: document.text.clone(),
                            version: 1,
                        },
                    );
                }
                Some(synced) if synced.text != document.text => {
                    synced.version += 1;
                    synced.text.clone_from(&document.text);
                    server.send(json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didChange",
                        "params": {
                            "textDocument": { "uri": uri, "version": synced.version },
                            "contentChanges": [{ "text": document.text }]
                        }
                    }));
                }
                Some(_) => {}
            }
        }
    }
}

fn project_root(path: &Path) -> PathBuf {
    let directory = path.parent().unwrap_or(path);
    for ancestor in directory.ancestors() {
        if ["pyrightconfig.json", "pyproject.toml", ".git"]
            .iter()
            .any(|marker| ancestor.join(marker).exists())
        {
            return ancestor.to_path_buf();
        }
    }
    directory.to_path_buf()
}

fn handle_server_message(
    message: Value,
    outbound: &Sender<Value>,
    proxy: &EventLoopProxy<LspEvent>,
) {
    if message.get("id").and_then(Value::as_u64) == Some(INITIALIZE_REQUEST_ID)
        && message.get("result").is_some()
    {
        let _ = proxy.send_event(LspEvent::Initialized);
        return;
    }

    if let (Some(id), Some(method)) = (
        message.get("id"),
        message.get("method").and_then(Value::as_str),
    ) {
        let result = if method == "workspace/configuration" {
            let count = message
                .pointer("/params/items")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            Value::Array((0..count).map(|_| Value::Null).collect())
        } else {
            Value::Null
        };
        let _ = outbound.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        return;
    }

    if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        && let Some(update) = parse_diagnostics(&message)
    {
        let _ = proxy.send_event(LspEvent::Diagnostics(update));
    }
}

fn parse_diagnostics(message: &Value) -> Option<DiagnosticUpdate> {
    let params = message.get("params")?;
    let uri = Url::parse(params.get("uri")?.as_str()?).ok()?;
    let path = uri.to_file_path().ok()?;
    let version = params.get("version").and_then(Value::as_i64);
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(|value| {
            let range = value.get("range")?;
            let position = |name: &str| {
                let position = range.get(name)?;
                Some(Position {
                    line: usize::try_from(position.get("line")?.as_u64()?).ok()?,
                    character: usize::try_from(position.get("character")?.as_u64()?).ok()?,
                })
            };
            let severity = match value.get("severity").and_then(Value::as_u64).unwrap_or(1) {
                1 => DiagnosticSeverity::Error,
                2 => DiagnosticSeverity::Warning,
                3 => DiagnosticSeverity::Information,
                _ => DiagnosticSeverity::Hint,
            };
            Some(Diagnostic {
                range: Range {
                    start: position("start")?,
                    end: position("end")?,
                },
                severity,
                message: value.get("message")?.as_str()?.to_owned(),
            })
        })
        .collect();
    Some(DiagnosticUpdate {
        path,
        version,
        diagnostics,
    })
}

fn write_message(writer: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            return Ok(None);
        }
        if header == "\r\n" || header == "\n" {
            break;
        }
        if let Some(value) = header
            .strip_prefix("Content-Length:")
            .and_then(|value| value.trim().parse::<usize>().ok())
        {
            content_length = Some(value);
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufReader, Cursor};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{DiagnosticSeverity, parse_diagnostics, project_root, read_message, write_message};

    #[test]
    fn json_rpc_messages_round_trip_with_content_length_framing() {
        let message = json!({"jsonrpc":"2.0","method":"example","params":{"emoji":"🦀"}});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();

        let decoded = read_message(&mut BufReader::new(Cursor::new(bytes))).unwrap();
        assert_eq!(decoded, Some(message));
    }

    #[test]
    fn diagnostic_notification_preserves_utf16_positions_and_severity() {
        let message = json!({
            "params": {
                "uri": "file:///tmp/example.py",
                "version": 4,
                "diagnostics": [{
                    "range": {
                        "start": {"line": 1, "character": 3},
                        "end": {"line": 1, "character": 7}
                    },
                    "severity": 2,
                    "message": "possibly unbound"
                }]
            }
        });
        let update = parse_diagnostics(&message).unwrap();
        assert_eq!(update.version, Some(4));
        assert_eq!(update.diagnostics[0].severity, DiagnosticSeverity::Warning);
        assert_eq!(update.diagnostics[0].range.start.character, 3);
    }

    #[test]
    fn project_root_uses_nearest_python_or_repository_marker() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("editor-lsp-root-{unique}"));
        let nested = root.join("src/package");
        fs::create_dir_all(&nested).unwrap();
        fs::write(root.join("pyproject.toml"), "[project]").unwrap();

        assert_eq!(project_root(&nested.join("main.py")), root);
        fs::remove_dir_all(root).unwrap();
    }
}
