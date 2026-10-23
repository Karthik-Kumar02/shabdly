//! The bundled cleanup engine (spec section 4): walks the fallback chain (MLX helper or
//! llama-server, then Ollama, then the raw transcript), restarts helpers that crash, hang or stop
//! answering, and kills a helper orphaned by a force-quit.
use crate::machine::run;
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use serde::Serialize;
use shabdly_core::model_store::model_dir;
use shabdly_core::models::{EngineKind, ModelSpec};
use shabdly_core::pipeline::{finish_sentence_from, warm_up};
use shabdly_core::prompt::system_prompt;
use shabdly_core::providers::{CleanupProvider, OllamaProvider, OpenAiProvider};
use shabdly_core::settings::{Language, Speed};
use shabdly_core::setup::{OllamaStatus, ollama_status};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

#[async_trait]
pub trait Candidate: Send + Sync {
    fn name(&self) -> &'static str;
    async fn start(&self) -> anyhow::Result<Running>;
    /// E2B drops the final punctuation (spec risk Q1), so its replies go through `finish_sentence_from`.
    fn punctuate(&self) -> bool {
        false
    }
}

pub struct Running {
    pub provider: Arc<dyn CleanupProvider>,
    pub child: Option<Child>,
}

/// A helper process speaking `/v1/chat/completions`: the Mac MLX helper or llama-server.
pub struct SpawnCandidate {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub token_env: &'static str,
    /// The MLX model folder, or the `.gguf` file for llama-server.
    pub model_dir: PathBuf,
    /// Holds `engine.pid` and `engine.log` (the helper's stderr).
    pub data_dir: PathBuf,
    pub punctuate: bool,
}

/// Paths go in as separate OS-string arguments, so spaces and Hindi characters survive. The token
/// goes only into the environment: arguments are visible to other users in `ps`.
pub fn command_for(c: &SpawnCandidate, port: u16, token: &str) -> Command {
    let mut cmd = Command::new(&c.program);
    cmd.arg("--model").arg(&c.model_dir).arg("--port").arg(port.to_string()).args(&c.args).env(c.token_env, token);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

fn new_token() -> anyhow::Result<String> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|e| anyhow!("no random token: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn free_port() -> anyhow::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

async fn healthy(port: u16) -> bool {
    let get = reqwest::get(format!("http://127.0.0.1:{port}/health"));
    matches!(tokio::time::timeout(Duration::from_secs(1), get).await, Ok(Ok(r)) if r.status().is_success())
}

fn kill(c: &mut Child) {
    let _ = c.kill();
    let _ = c.wait();
}

/// Kills a half-started helper when its start fails or is cancelled.
struct KillOnDrop(Option<Child>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            kill(c);
        }
    }
}

/// Windows: the helper dies with Shabdly, even on a force-quit (spec R3).
#[cfg(windows)]
fn contain(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // SAFETY: plain Win32 calls on a live process handle. The job handle is never closed on
    // purpose: Windows closes it when Shabdly exits, which kills the helper.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = std::mem::size_of_val(&info) as u32;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size);
        AssignProcessToJobObject(job, child.as_raw_handle() as _);
    }
}

#[async_trait]
impl Candidate for SpawnCandidate {
    fn name(&self) -> &'static str {
        self.name
    }

    fn punctuate(&self) -> bool {
        self.punctuate
    }

    async fn start(&self) -> anyhow::Result<Running> {
        if !self.model_dir.exists() {
            bail!("model not downloaded: {}", self.model_dir.display());
        }
        let token = new_token()?;
        let log_path = self.data_dir.join("engine.log");
        for attempt in 1..=3 {
            let port = free_port()?;
            let mut cmd = command_for(self, port, &token);
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(fs::File::create(&log_path)?);
            let mut guard = KillOnDrop(Some(cmd.spawn().with_context(|| format!("starting {}", self.program.display()))?));
            let child = guard.0.as_mut().unwrap();
            #[cfg(windows)]
            contain(child);
            let _ = fs::write(self.data_dir.join("engine.pid"), child.id().to_string());

            let deadline = Instant::now() + Duration::from_secs(60);
            let exited = loop {
                if let Some(status) = child.try_wait()? {
                    break Some(status);
                }
                if healthy(port).await {
                    break None;
                }
                if Instant::now() > deadline {
                    bail!("{} not ready after 60 s", self.name);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            };
            if let Some(status) = exited {
                let log = fs::read_to_string(&log_path).unwrap_or_default();
                // The port was taken between the pick and the bind: try another one.
                if attempt < 3 && (log.contains("bind") || log.contains("listen")) {
                    continue;
                }
                bail!("{} exited ({status}): {}", self.name, log.lines().last().unwrap_or(""));
            }

            let provider = OpenAiProvider { base_url: format!("http://127.0.0.1:{port}"), token: token.clone(), model: self.name.into() };
            let system = system_prompt(Language::Auto, &[]);
            let test = provider.clean(&system, "um hello there");
            match tokio::time::timeout(Duration::from_secs(30), test).await {
                Ok(Ok(s)) if !s.is_empty() => {}
                Ok(Err(e)) => bail!("{} test cleanup failed: {e:#}", self.name),
                _ => bail!("{} test cleanup gave no text", self.name),
            }
            return Ok(Running { provider: Arc::new(provider), child: guard.0.take() });
        }
        bail!("{} could not bind a port", self.name)
    }
}

/// Ollama, for upgraders and as a backup: ready when it runs and lists its model.
pub struct OllamaCandidate;

#[async_trait]
impl Candidate for OllamaCandidate {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn start(&self) -> anyhow::Result<Running> {
        let p = OllamaProvider::default();
        if ollama_status(&p.base_url, &p.model).await != OllamaStatus::Ready {
            bail!("Ollama is not running or does not have {}", p.model);
        }
        warm_up(&p).await?;
        Ok(Running { provider: Arc::new(p), child: None })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineStatus {
    Starting,
    Builtin,
    Backup { reason: String },
    Paused { reason: String },
}

struct State {
    index: usize,
    active: Option<Running>,
    status: EngineStatus,
    last_exit: Option<Instant>,
    /// Bumped by stop(): a start already under way then kills its helper instead of keeping it.
    stops: u64,
}

pub struct LocalEngine {
    me: Weak<LocalEngine>,
    candidates: Vec<Box<dyn Candidate>>,
    /// Held while the chain is walked, so starts and restarts never overlap.
    walk: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    timeouts: AtomicU32,
    watching: AtomicBool,
}

fn refused(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::ConnectionRefused))
}

/// Counts a `clean` dropped before it finished (the pipeline's timeout) as a hang.
struct HangGuard<'a> {
    engine: &'a LocalEngine,
    done: bool,
}

impl Drop for HangGuard<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if self.engine.timeouts.fetch_add(1, Ordering::SeqCst) + 1 >= 2 {
            self.engine.timeouts.store(0, Ordering::SeqCst);
            if let (Some(me), Ok(rt)) = (self.engine.me.upgrade(), tokio::runtime::Handle::try_current()) {
                rt.spawn(async move { me.restart().await });
            }
        }
    }
}

impl LocalEngine {
    pub fn new(candidates: Vec<Box<dyn Candidate>>) -> Arc<LocalEngine> {
        Arc::new_cyclic(|me| LocalEngine {
            me: me.clone(),
            candidates,
            walk: tokio::sync::Mutex::new(()),
            state: Mutex::new(State { index: 0, active: None, status: EngineStatus::Starting, last_exit: None, stops: 0 }),
            timeouts: AtomicU32::new(0),
            watching: AtomicBool::new(false),
        })
    }

    pub fn status(&self) -> EngineStatus {
        self.state.lock().unwrap().status.clone()
    }

    /// Starts the chain from the current step unless an engine is already running.
    pub async fn ensure(&self) {
        let _walk = self.walk.lock().await;
        let (idle, index) = { let s = self.state.lock().unwrap(); (s.active.is_none(), s.index) };
        if idle {
            self.walk_from(index, None).await;
        }
    }

    /// Walks the chain again from step 1 (Settings "Retry").
    pub async fn retry(&self) {
        let _walk = self.walk.lock().await;
        {
            // A block, not drop(): the guard must not live across the await (the future stays Send).
            let mut s = self.state.lock().unwrap();
            s.index = 0;
            s.last_exit = None;
        }
        self.walk_from(0, None).await;
    }

    pub fn stop(&self) {
        let active = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // Nothing runs now; the next ensure() starts it again.
            s.status = EngineStatus::Starting;
            s.stops += 1;
            s.active.take()
        };
        if let Some(mut c) = active.and_then(|r| r.child) {
            kill(&mut c);
        }
    }

    /// Kills the current helper and starts the current step again, then the rest of the chain.
    async fn restart(&self) {
        let _walk = self.walk.lock().await;
        let index = self.state.lock().unwrap().index;
        self.walk_from(index, None).await;
    }

    /// Call with `walk` held.
    async fn walk_from(&self, from: usize, reason: Option<String>) {
        let (old, stops) = {
            let mut s = self.state.lock().unwrap();
            s.status = EngineStatus::Starting;
            (s.active.take(), s.stops)
        };
        if let Some(mut c) = old.and_then(|r| r.child) {
            kill(&mut c);
        }
        let mut reason = reason;
        for (i, c) in self.candidates.iter().enumerate().skip(from) {
            match c.start().await {
                Ok(running) => {
                    let status = if i == 0 && c.name() != "ollama" {
                        EngineStatus::Builtin
                    } else {
                        EngineStatus::Backup { reason: reason.unwrap_or_default() }
                    };
                    eprintln!("cleanup engine: {} running ({status:?})", c.name());
                    let mut s = self.state.lock().unwrap();
                    if s.stops != stops {
                        drop(s);
                        if let Some(mut c) = running.child {
                            kill(&mut c);
                        }
                        return;
                    }
                    (s.index, s.active, s.status) = (i, Some(running), status);
                    drop(s);
                    self.timeouts.store(0, Ordering::SeqCst);
                    self.watch();
                    return;
                }
                Err(e) => {
                    eprintln!("cleanup engine: {} did not start: {e:#}", c.name());
                    reason.get_or_insert_with(|| format!("{}: {e:#}", c.name()));
                }
            }
        }
        let mut s = self.state.lock().unwrap();
        if s.stops != stops {
            return;
        }
        s.index = self.candidates.len();
        s.status = EngineStatus::Paused { reason: reason.unwrap_or_else(|| "no engine".into()) };
    }

    fn active_exited(&self) -> bool {
        let mut s = self.state.lock().unwrap();
        let child = s.active.as_mut().and_then(|r| r.child.as_mut());
        child.is_some_and(|c| c.try_wait().is_ok_and(|st| st.is_some()))
    }

    /// A helper exit restarts it once; a second exit within 60 s moves down the chain. Runs as its
    /// own task so a dictation timing out never cancels a restart halfway.
    async fn check_exit(&self) {
        if !self.active_exited() {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        let _ = tokio::spawn(async move {
            let _walk = me.walk.lock().await;
            if !me.active_exited() {
                return; // Already handled.
            }
            let (from, reason) = {
                let mut s = me.state.lock().unwrap();
                let now = Instant::now();
                if s.last_exit.is_some_and(|t| now - t < Duration::from_secs(60)) {
                    s.last_exit = None;
                    (s.index + 1, Some("crashed_twice".to_string()))
                } else {
                    s.last_exit = Some(now);
                    (s.index, None)
                }
            };
            eprintln!("cleanup engine: helper exited, starting step {}", from + 1);
            me.walk_from(from, reason).await;
        })
        .await;
    }

    /// Notices a crash between dictations, so the restart is done before the next one.
    fn watch(&self) {
        if self.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = self.me.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(me) = weak.upgrade() else { break };
                me.check_exit().await;
            }
        });
    }

    /// Waits while the chain is being walked (within the pipeline's timeout).
    async fn active_provider(&self) -> anyhow::Result<(Arc<dyn CleanupProvider>, bool)> {
        let _walk = self.walk.lock().await;
        let s = self.state.lock().unwrap();
        match &s.active {
            Some(r) => Ok((r.provider.clone(), self.candidates[s.index].punctuate())),
            None => bail!("cleanup paused"),
        }
    }
}

impl Drop for LocalEngine {
    fn drop(&mut self) {
        self.stop();
    }
}

#[async_trait]
impl CleanupProvider for LocalEngine {
    fn base_timeout(&self) -> Duration {
        Duration::from_secs(8)
    }

    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        self.check_exit().await;
        let mut retried = false;
        loop {
            let (provider, punctuate) = self.active_provider().await?;
            let mut guard = HangGuard { engine: self, done: false };
            let result = provider.clean(system, text).await;
            guard.done = true;
            self.timeouts.store(0, Ordering::SeqCst);
            match result {
                // Dead after sleep or wake: restart and retry this request once.
                Err(e) if !retried && refused(&e) => {
                    retried = true;
                    if let Some(me) = self.me.upgrade() {
                        let _ = tokio::spawn(async move { me.restart().await }).await;
                    }
                }
                r => return r.map(|t| if punctuate { finish_sentence_from(&t, text) } else { t }),
            }
        }
    }
}

fn spawn_candidate(
    name: &'static str,
    program: PathBuf,
    model_dir: PathBuf,
    args: &[&str],
    token_env: &'static str,
    app_data: &Path,
    spec: &ModelSpec,
) -> Box<dyn Candidate> {
    Box::new(SpawnCandidate {
        name,
        program,
        args: args.iter().map(OsString::from).collect(),
        token_env,
        model_dir,
        data_dir: app_data.to_path_buf(),
        punctuate: spec.speed == Speed::Fast,
    })
}

/// The spec section 4 table for this OS. `spec` is `None` on macOS 11.0 to 13.2 (no bundled
/// engine); `gguf_fallback` is the Mac llama-server backup model, when it is present.
pub fn chain(
    app_data: &Path,
    engines_dir: &Path,
    spec: Option<&ModelSpec>,
    gguf_fallback: Option<&ModelSpec>,
) -> Vec<Box<dyn Candidate>> {
    let models = app_data.join("models");
    let llama_bin = engines_dir.join(if cfg!(windows) { "win/llama/llama-server.exe" } else { "mac/llama/llama-server" });
    let llama = |spec: &ModelSpec, gpu_layers: &str| {
        let gguf = model_dir(&models, spec).join(spec.files[0].name);
        let args = ["--host", "127.0.0.1", "-c", "8192", "-np", "1", "-ngl", gpu_layers];
        spawn_candidate("llama-server", llama_bin.clone(), gguf, &args, "LLAMA_API_KEY", app_data, spec)
    };
    let mut c = Vec::new();
    if let Some(spec) = spec {
        if spec.engine == EngineKind::Mlx {
            let pid = std::process::id().to_string();
            let program = engines_dir.join("mac/shabdly-engine");
            let dir = model_dir(&models, spec);
            c.push(spawn_candidate("mlx", program, dir, &["--parent-pid", &pid], "SHABDLY_ENGINE_TOKEN", app_data, spec));
            if let Some(g) = gguf_fallback {
                c.push(llama(g, "99"));
            }
        } else {
            c.push(llama(spec, "99"));
            if cfg!(windows) {
                c.push(llama(spec, "0"));
            }
        }
    }
    c.push(Box::new(OllamaCandidate));
    c
}

/// Our helper executables, by file name.
fn is_ours(exe: &str) -> bool {
    let name = Path::new(exe.trim()).file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(name, "shabdly-engine" | "llama-server" | "llama-server.exe")
}

/// Orphan fail-safe (llama-server on Mac has no parent watch): kills the PID in `engine.pid` if it
/// is one of our helpers, then deletes the file.
pub fn kill_stale(data_dir: &Path) {
    let file = data_dir.join("engine.pid");
    if let Some(pid) = fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        let pid = pid.to_string();
        if cfg!(windows) {
            let filter = format!("PID eq {pid}");
            let out = run(Path::new("tasklist"), &["/FI", &filter, "/FO", "CSV", "/NH"]).unwrap_or_default();
            if is_ours(out.split(',').next().unwrap_or("").trim_matches('"')) {
                run(Path::new("taskkill"), &["/PID", &pid, "/F"]);
            }
        } else if is_ours(&run(Path::new("/bin/ps"), &["-p", &pid, "-o", "comm="]).unwrap_or_default()) {
            run(Path::new("/bin/kill"), &["-9", &pid]);
        }
    }
    let _ = fs::remove_file(file);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    enum Reply {
        Text(&'static str),
        Refused,
        Hang,
    }

    struct FakeProvider(Reply);

    #[async_trait]
    impl CleanupProvider for FakeProvider {
        fn base_timeout(&self) -> Duration {
            Duration::from_secs(8)
        }
        async fn clean(&self, _system: &str, _text: &str) -> anyhow::Result<String> {
            match self.0 {
                Reply::Text(s) => Ok(s.to_string()),
                Reply::Refused => Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused).into()),
                Reply::Hang => std::future::pending().await,
            }
        }
    }

    type StartFn = Box<dyn Fn(usize) -> anyhow::Result<Running> + Send + Sync>;

    struct Fake {
        name: &'static str,
        starts: Arc<AtomicUsize>,
        punctuate: bool,
        start: StartFn,
    }

    #[async_trait]
    impl Candidate for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn punctuate(&self) -> bool {
            self.punctuate
        }
        async fn start(&self) -> anyhow::Result<Running> {
            (self.start)(self.starts.fetch_add(1, Ordering::SeqCst))
        }
    }

    fn running(reply: Reply) -> anyhow::Result<Running> {
        Ok(Running { provider: Arc::new(FakeProvider(reply)), child: None })
    }

    fn fake(name: &'static str, start: impl Fn(usize) -> anyhow::Result<Running> + Send + Sync + 'static) -> (Box<dyn Candidate>, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        (Box::new(Fake { name, starts: starts.clone(), punctuate: false, start: Box::new(start) }), starts)
    }

    /// A child process that has already exited, as a crashed helper looks.
    fn exited_child() -> Child {
        let mut c = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 1"]).spawn().unwrap()
        } else {
            Command::new("sh").args(["-c", "exit 1"]).spawn().unwrap()
        };
        c.wait().unwrap();
        c
    }

    async fn wait_for(starts: &AtomicUsize, n: usize) {
        for _ in 0..100 {
            if starts.load(Ordering::SeqCst) >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn first_working_candidate_is_builtin() {
        let (a, _) = fake("a", |_| running(Reply::Text("from a")));
        let (b, b_starts) = fake("b", |_| running(Reply::Text("from b")));
        let engine = LocalEngine::new(vec![a, b]);
        assert_eq!(engine.status(), EngineStatus::Starting);
        engine.ensure().await;
        assert_eq!(engine.status(), EngineStatus::Builtin);
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from a");
        assert_eq!(b_starts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stop_then_clean_starts_again() {
        let (a, a_starts) = fake("a", |_| running(Reply::Text("from a")));
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        engine.stop();
        // Nothing runs after stop, so the status must not still say Builtin.
        assert_eq!(engine.status(), EngineStatus::Starting);
        engine.ensure().await;
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from a");
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failed_first_falls_back_and_reports_backup() {
        let (a, _) = fake("a", |_| anyhow::bail!("model missing"));
        let (b, _) = fake("b", |_| running(Reply::Text("from b")));
        let engine = LocalEngine::new(vec![a, b]);
        engine.ensure().await;
        assert!(matches!(engine.status(), EngineStatus::Backup { reason } if reason.contains("model missing")));
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from b");
    }

    #[tokio::test]
    async fn all_fail_reports_paused_and_clean_errs() {
        let (a, _) = fake("a", |_| anyhow::bail!("no"));
        let (b, _) = fake("b", |_| anyhow::bail!("no"));
        let engine = LocalEngine::new(vec![a, b]);
        engine.ensure().await;
        assert!(matches!(engine.status(), EngineStatus::Paused { .. }));
        assert!(engine.clean("s", "t").await.is_err());
    }

    #[tokio::test]
    async fn second_exit_within_60s_advances_chain() {
        let (a, a_starts) =
            fake("a", |_| Ok(Running { provider: Arc::new(FakeProvider(Reply::Text("from a"))), child: Some(exited_child()) }));
        let (b, _) = fake("b", |_| running(Reply::Text("from b")));
        let engine = LocalEngine::new(vec![a, b]);
        engine.ensure().await;
        // First exit: restart the same engine once.
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from a");
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);
        // Second exit within 60 s: next step of the chain.
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from b");
        assert_eq!(engine.status(), EngineStatus::Backup { reason: "crashed_twice".into() });
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refused_connection_restarts_and_retries_once() {
        let (a, a_starts) = fake("a", |n| running(if n == 0 { Reply::Refused } else { Reply::Text("after restart") }));
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        assert_eq!(engine.clean("s", "t").await.unwrap(), "after restart");
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);

        // Still refused after the restart: one retry only, then the error (raw transcript).
        let (a, a_starts) = fake("a", |_| running(Reply::Refused));
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        assert!(engine.clean("s", "t").await.is_err());
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn two_timeouts_restart() {
        let (a, a_starts) = fake("a", |_| running(Reply::Hang));
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        let short = Duration::from_millis(20);
        assert!(tokio::time::timeout(short, engine.clean("s", "t")).await.is_err());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(a_starts.load(Ordering::SeqCst), 1, "one timeout must not restart");
        assert!(tokio::time::timeout(short, engine.clean("s", "t")).await.is_err());
        wait_for(&a_starts, 2).await;
        assert_eq!(a_starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_starts_from_first_candidate() {
        let (a, _) = fake("a", |n| if n == 0 { anyhow::bail!("not yet") } else { running(Reply::Text("from a")) });
        let (b, _) = fake("b", |_| running(Reply::Text("from b")));
        let engine = LocalEngine::new(vec![a, b]);
        engine.ensure().await;
        assert!(matches!(engine.status(), EngineStatus::Backup { .. }));
        engine.retry().await;
        assert_eq!(engine.status(), EngineStatus::Builtin);
        assert_eq!(engine.clean("s", "t").await.unwrap(), "from a");
    }

    #[tokio::test]
    async fn e2b_replies_get_final_punctuation() {
        let starts = Arc::new(AtomicUsize::new(0));
        let a = Box::new(Fake { name: "a", starts, punctuate: true, start: Box::new(|_| running(Reply::Text("kya hua"))) });
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        assert_eq!(engine.clean("s", "t").await.unwrap(), "Kya hua?");
    }

    #[tokio::test]
    async fn real_refused_connection_is_detected() {
        let port = free_port().unwrap();
        let p = OpenAiProvider { base_url: format!("http://127.0.0.1:{port}"), token: "t".into(), model: "m".into() };
        assert!(refused(&p.clean("s", "t").await.unwrap_err()));
        assert!(!refused(&anyhow::anyhow!("500: boom")));
    }

    #[test]
    fn command_keeps_unicode_path_intact() {
        let dir = PathBuf::from("/Users/Sai Karthik/कार्तिक/models/x");
        let c = SpawnCandidate {
            name: "mlx",
            program: PathBuf::from("/Applications/Shabdly.app/engines/mac/shabdly-engine"),
            args: vec!["--parent-pid".into(), "42".into()],
            token_env: "SHABDLY_ENGINE_TOKEN",
            model_dir: dir.clone(),
            data_dir: PathBuf::from("/tmp"),
            punctuate: false,
        };
        let cmd = command_for(&c, 5123, "secret-token");
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        let at = args.iter().position(|a| *a == "--model").unwrap();
        assert_eq!(args[at + 1], dir.as_os_str());
        assert!(args.windows(2).any(|w| w[0] == "--port" && w[1] == "5123"));
        assert!(!args.iter().any(|a| a.to_string_lossy().contains("secret-token")));
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(env.iter().any(|(k, v)| *k == "SHABDLY_ENGINE_TOKEN" && *v == Some("secret-token".as_ref())));
    }

    /// Live: the real Mac helper starts through `chain` and answers with the fix applied.
    /// Run: SHABDLY_TEST_MLX_MODEL=<E2B model dir> cargo test -p shabdly live_mlx -- --ignored
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore]
    async fn live_mlx_helper_starts_and_cleans() {
        let model = PathBuf::from(std::env::var("SHABDLY_TEST_MLX_MODEL").expect("SHABDLY_TEST_MLX_MODEL"));
        let data = tempfile::tempdir().unwrap();
        let spec = shabdly_core::models::pick(shabdly_core::models::Os::Mac, (14, 0), Speed::Fast).unwrap();
        std::fs::create_dir_all(data.path().join("models")).unwrap();
        std::os::unix::fs::symlink(&model, model_dir(&data.path().join("models"), spec)).unwrap();
        let engines = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engines");
        let mut c = chain(data.path(), &engines, Some(spec), None);
        c.truncate(1);
        let engine = LocalEngine::new(c);
        engine.ensure().await;
        assert_eq!(engine.status(), EngineStatus::Builtin);
        let out = engine.clean(&system_prompt(Language::Auto, &[]), "um kya tum kal aa rahe ho").await.unwrap();
        println!("cleaned: {out}");
        assert!(out.ends_with(['.', '?', '!']));
        engine.stop();
    }

    #[cfg(unix)]
    fn sleeper() -> Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
    }

    /// A Speed swap drops the old engine: its helper must not outlive it.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_engine_kills_its_helper() {
        let pid = Arc::new(AtomicU32::new(0));
        let p = pid.clone();
        let (a, _) = fake("a", move |_| {
            let c = sleeper();
            p.store(c.id(), Ordering::SeqCst);
            Ok(Running { provider: Arc::new(FakeProvider(Reply::Text("from a"))), child: Some(c) })
        });
        let engine = LocalEngine::new(vec![a]);
        engine.ensure().await;
        let pid = pid.load(Ordering::SeqCst);
        assert!(alive(pid));
        drop(engine);
        assert!(!alive(pid));
    }

    /// Starts only when told to, as a slow model load does.
    #[cfg(unix)]
    struct Slow {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        pid: Arc<AtomicU32>,
    }

    #[cfg(unix)]
    #[async_trait]
    impl Candidate for Slow {
        fn name(&self) -> &'static str {
            "slow"
        }
        async fn start(&self) -> anyhow::Result<Running> {
            self.entered.notify_one();
            self.release.notified().await;
            let c = sleeper();
            self.pid.store(c.id(), Ordering::SeqCst);
            Ok(Running { provider: Arc::new(FakeProvider(Reply::Text("slow"))), child: Some(c) })
        }
    }

    /// stop() during a start (leaving the built-in engine, a swap): the start must not leave a helper.
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_during_start_leaves_no_helper() {
        let (entered, release, pid) = (Arc::default(), Arc::<tokio::sync::Notify>::default(), Arc::new(AtomicU32::new(0)));
        let slow = Slow { entered: Arc::clone(&entered), release: release.clone(), pid: pid.clone() };
        let engine = LocalEngine::new(vec![Box::new(slow)]);
        let e = engine.clone();
        let walk = tokio::spawn(async move { e.ensure().await });
        entered.notified().await;
        engine.stop();
        release.notify_one();
        walk.await.unwrap();
        assert_eq!(engine.status(), EngineStatus::Starting);
        assert!(!alive(pid.load(Ordering::SeqCst)));
    }

    #[test]
    fn only_our_helpers_are_killed() {
        assert!(is_ours("/Applications/Shabdly.app/Contents/Resources/engines/mac/shabdly-engine"));
        assert!(is_ours("llama-server.exe"));
        assert!(is_ours("/x/engines/mac/llama/llama-server"));
        assert!(!is_ours("/usr/bin/python3"));
        assert!(!is_ours(""));
    }
}
