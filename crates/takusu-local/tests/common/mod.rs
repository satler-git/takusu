//! Shared test helpers for the storage-suite parameterized tests.
//!
//! `spawn_wrangler` boots the real `takusu-worker` via `wrangler dev --local`
//! so that `WorkersStorage` can be exercised against the actual Cloudflare
//! Worker runtime (workerd) + D1 instead of a hand-rolled mock.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use takusu_local_lib::generate_root_jwt;

// (ToSocketAddrs is used by wait_for_port_free below.)

/// JWT secret shared between the test process and the worker (`.dev.vars`).
pub const JWT_SECRET: &str = "test-secret-do-not-use-in-production";

/// Port the local worker listens on. Tests are serialized
/// (`--test-threads=1` when running `--ignored`), so a fixed port is fine.
pub const WRANGLER_PORT: u16 = 8789;

static ROOT_TOKEN: LazyLock<String> = LazyLock::new(|| {
    generate_root_jwt(JWT_SECRET, None).expect("root token generation should not fail")
});

pub fn root_token() -> &'static str {
    ROOT_TOKEN.as_str()
}

fn worker_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("takusu-worker")
        .canonicalize()
        .expect("takusu-worker crate dir should exist")
}

fn dev_vars_path() -> PathBuf {
    worker_dir().join(".dev.vars")
}

/// RAII guard for a `wrangler dev` process + the temporary `.dev.vars` file
/// + the temporary persistence directory.
///
/// On drop the child is killed, `.dev.vars` removed, and the temp dir deleted
/// so the working tree stays clean.
pub struct WranglerGuard {
    child: Option<Child>,
    dev_vars: PathBuf,
    persist_dir: Option<tempfile::TempDir>,
}

impl Drop for WranglerGuard {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            // wrangler spawns workerd as a child; `child.kill()` only
            // signals the wrangler process and would orphan workerd (which
            // keeps holding the port). We spawned wrangler in its own
            // process group (process_group(0)), so kill the whole group.
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            kill_process_group(pid);
        }
        let _ = std::fs::remove_file(&self.dev_vars);
        // persist_dir dropped last → deletes the temp dir
        drop(self.persist_dir.take());
        wait_for_port_free();
    }
}

/// Send SIGKILL to the process group whose leader is `pid`.
///
/// Wrangler is spawned with `process_group(0)` so it becomes the leader of a
/// fresh process group; this kills workerd (and any other descendants) along
/// with wrangler itself. Best-effort — errors are ignored because the group
/// may already be gone.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    // SAFETY: killpg takes a pid_t and a signal; negative pid means "the
    // process group". SIGKILL=9. Errors (ESRCH if already gone) are ignored.
    unsafe {
        libc::killpg(pid as i32, 9);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

/// PID of the wrangler process group leader, set when wrangler is spawned.
///
/// Rust `static` values are **not** dropped on process exit, so
/// `WranglerGuard::drop` never runs for the `LazyLock`-held guard. We
/// register an `atexit` handler that uses this PID to `killpg` the whole
/// wrangler tree (wrangler + workerd + esbuild) when `cargo test` finishes.
static WRANGLER_PGID: AtomicU32 = AtomicU32::new(0);

#[cfg(unix)]
extern "C" fn kill_wrangler_atexit() {
    let pgid = WRANGLER_PGID.load(Ordering::SeqCst);
    if pgid != 0 {
        kill_process_group(pgid);
    }
}

fn wait_for_port_free() {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let addr = ("127.0.0.1", WRANGLER_PORT)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.next());
        if let Some(addr) = addr
            && TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err()
        {
            return;
        }
        sleep(Duration::from_millis(200));
    }
}

/// Write `.dev.vars` with the test JWT secret, then spawn
/// `wrangler dev --local` in the `takusu-worker` crate directory.
///
/// The worker is built first (`worker-build --release`) by wrangler's
/// `[build]` step; in CI we pre-build to avoid the per-test startup cost.
/// Persistence is redirected to a fresh temp dir so each test process starts
/// from a clean D1 database.
pub fn spawn_wrangler() -> WranglerGuard {
    wait_for_port_free();

    let dev_vars = dev_vars_path();
    std::fs::write(&dev_vars, format!("TAKUSU_JWT_SECRET={JWT_SECRET}\n"))
        .expect("write .dev.vars");

    let persist_dir = tempfile::TempDir::new().expect("temp dir for wrangler state");
    let persist_path = persist_dir.path().to_str().unwrap();

    // Apply D1 migrations to the fresh local persistence dir before starting
    // `wrangler dev` — wrangler dev itself does not run migrations on boot.
    let migration_status = Command::new("wrangler")
        .args([
            "d1",
            "migrations",
            "apply",
            "takusu",
            "--local",
            "--persist-to",
            persist_path,
        ])
        .current_dir(worker_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .expect("failed to run wrangler d1 migrations apply");
    assert!(
        migration_status.success(),
        "wrangler d1 migrations apply failed (exit {migration_status})"
    );

    let mut cmd = Command::new("wrangler");
    cmd.args([
        "dev",
        "--local",
        "--port",
        &WRANGLER_PORT.to_string(),
        "--persist-to",
        persist_path,
        "--log-level",
        "warn",
    ])
    .current_dir(worker_dir())
    .env("TAKUSU_JWT_SECRET", JWT_SECRET)
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit());

    // Put wrangler in its own process group so we can killpg(2) the whole
    // tree (wrangler + workerd + esbuild) on drop instead of orphaning
    // workerd which keeps holding the port.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd
        .spawn()
        .expect("failed to start wrangler dev (is wrangler on PATH?)");

    // Register an atexit handler so the wrangler process group is killed
    // even though Rust statics (LazyLock) are not dropped on process exit.
    #[cfg(unix)]
    {
        WRANGLER_PGID.store(child.id(), Ordering::SeqCst);
        // SAFETY: atexit takes a C function pointer. Our handler is
        // `extern "C" fn()` with no captured state (reads WRANGLER_PGID
        // global). Registering multiple times is harmless.
        unsafe {
            libc::atexit(kill_wrangler_atexit);
        }
    }

    let guard = WranglerGuard {
        child: Some(child),
        dev_vars,
        persist_dir: Some(persist_dir),
    };
    wait_for_ready();
    guard
}

fn wait_for_ready() {
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if let Ok((200, _)) = http_get("/health") {
            return;
        }
        sleep(Duration::from_millis(500));
    }
    panic!("wrangler dev did not become ready within 180s");
}

#[allow(dead_code)]
pub fn worker_url() -> String {
    format!("http://127.0.0.1:{WRANGLER_PORT}")
}

fn http_get(path: &str) -> Result<(u16, String), String> {
    use std::io::{Read, Write};
    let host = "127.0.0.1";
    let mut stream =
        TcpStream::connect((host, WRANGLER_PORT)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {host}:{WRANGLER_PORT}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| format!("read: {e}"))?;
    let response_str = String::from_utf8_lossy(&response);
    let status_line = response_str.lines().next().unwrap_or("");
    let parts: Vec<&str> = status_line.split(' ').collect();
    let status_code: u16 = parts.get(1).unwrap_or(&"500").parse().unwrap_or(500);
    let body = response_str
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .to_string();
    Ok((status_code, body))
}
