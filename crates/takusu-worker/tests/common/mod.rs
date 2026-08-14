use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use takusu_types::jwt;

pub const PORT: u16 = 8789;
/// JWT secret shared between the worker and the e2e test process.
pub const JWT_SECRET: &str = "test-secret-do-not-use-in-production";

static WRANGLER_PGID: AtomicU32 = AtomicU32::new(0);

pub struct WranglerGuard {
    child: Option<Child>,
    restored_dev_vars: Option<String>,
    #[allow(dead_code)]
    persist_dir: tempfile::TempDir,
}

impl Drop for WranglerGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            kill_process_group(child.id());
            let _ = child.wait();
        }
        wait_for_port_free();

        let dev_vars = dev_vars_path();
        match &self.restored_dev_vars {
            Some(content) => {
                let _ = std::fs::write(&dev_vars, content);
            }
            None => {
                let _ = std::fs::remove_file(&dev_vars);
            }
        }
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
    // `pid` is a u32 because Rust's `Child::id()` returns u32, while
    // `libc::pid_t` is i32. PIDs on normal Linux systems are limited to
    // 2^22 or smaller, so the cast is safe in practice.
    unsafe {
        libc::killpg(pid as i32, 9);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

#[cfg(unix)]
extern "C" fn kill_wrangler_atexit() {
    let pgid = WRANGLER_PGID.load(Ordering::SeqCst);
    if pgid != 0 {
        kill_process_group(pgid);
    }
}

fn worker_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn dev_vars_path() -> PathBuf {
    worker_dir().join(".dev.vars")
}

/// Generate a fresh root JWT signed with [`JWT_SECRET`].
pub fn root_token() -> String {
    jwt::generate_root_jwt(JWT_SECRET, Some("worker-e2e")).expect("root token generation")
}

pub fn start_wrangler() -> WranglerGuard {
    wait_for_port_free();

    let dev_vars = dev_vars_path();
    let restored = if dev_vars.exists() {
        Some(std::fs::read_to_string(&dev_vars).expect("read .dev.vars"))
    } else {
        None
    };

    std::fs::write(
        &dev_vars,
        format!(
            "TAKUSU_JWT_SECRET=\"{JWT_SECRET}\"\nTAKUSU_ROOT_TOKEN=\"{}\"\n",
            root_token()
        ),
    )
    .expect("write .dev.vars");

    let persist_dir = tempfile::TempDir::new().expect("temp dir for wrangler state");
    let persist_path = persist_dir.path().to_str().unwrap();

    // Apply D1 migrations to a fresh local persistence dir before starting the
    // worker. `wrangler dev` does not run migrations on boot.
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
        &PORT.to_string(),
        "--persist-to",
        persist_path,
        "--log-level",
        "warn",
    ])
    .current_dir(worker_dir())
    .env("TAKUSU_JWT_SECRET", JWT_SECRET);

    // Put wrangler in its own process group so the whole tree (wrangler +
    // workerd + esbuild) is killed on drop instead of orphaning workerd.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().expect("failed to start wrangler dev");

    // Register an atexit handler so the wrangler process group is killed even
    // though Rust statics are not dropped on process exit.
    #[cfg(unix)]
    {
        WRANGLER_PGID.store(child.id(), Ordering::SeqCst);
        unsafe {
            libc::atexit(kill_wrangler_atexit);
        }
    }

    wait_for_ready();
    WranglerGuard {
        child: Some(child),
        restored_dev_vars: restored,
        persist_dir,
    }
}

fn wait_for_port_free() {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let addr = ("127.0.0.1", PORT)
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

fn wait_for_ready() {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if let Ok((200, _)) = http_get("/health", None) {
            return;
        }
        sleep(Duration::from_millis(500));
    }
    panic!("wrangler dev did not become ready within 120s");
}

pub fn http_get(path: &str, auth_token: Option<&str>) -> Result<(u16, String), String> {
    let host = "127.0.0.1";
    let mut stream = TcpStream::connect((host, PORT)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    let auth_line = auth_token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{PORT}\r\n{auth_line}Connection: close\r\n\r\n"
    );
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

#[allow(dead_code)]
pub fn http_post_json(
    path: &str,
    auth_token: Option<&str>,
    body: &str,
) -> Result<(u16, String), String> {
    let host = "127.0.0.1";
    let mut stream = TcpStream::connect((host, PORT)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    let auth_line = auth_token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}:{PORT}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         {auth_line}\
         Connection: close\r\n\r\n\
         {body}",
        len = body.len()
    );
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
