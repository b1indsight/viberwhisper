use crate::local::installer::venv_python_path;
use reqwest::StatusCode;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Runtime status for the local inference service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServiceStatus {
    /// True when the service appears to be running.
    pub running: bool,
    /// TCP port used by the local service.
    pub port: u16,
    /// Last HTTP health-check result.
    pub health: String,
    /// Optional persisted process identifier.
    pub pid: Option<u32>,
    /// Optional human-readable memory usage.
    pub memory_usage: Option<String>,
}

/// Manages the Python-based local Gemma inference service.
pub struct LocalServiceManager {
    port: u16,
    model_dir: PathBuf,
    venv_dir: PathBuf,
    quantization: String,
    process: Option<Child>,
    log_file: Option<PathBuf>,
    /// True only when this manager spawned the server process itself.
    owned: bool,
}

impl LocalServiceManager {
    /// Creates a new local service manager using the default `int8` quantization mode.
    pub fn new(port: u16, model_dir: PathBuf, venv_dir: PathBuf) -> Self {
        Self::with_quantization(port, model_dir, venv_dir, "int8".to_string())
    }

    /// Creates a new local service manager with an explicit quantization mode.
    pub fn with_quantization(
        port: u16,
        model_dir: PathBuf,
        venv_dir: PathBuf,
        quantization: String,
    ) -> Self {
        let log_file = model_dir
            .parent()
            .map(|dir| dir.join("server.log"));
        Self {
            port,
            model_dir,
            venv_dir,
            quantization,
            process: None,
            log_file,
            owned: false,
        }
    }

    /// Spawns the local server process and waits for health.
    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.is_running()
            && health_check(&self.base_url(), HEALTH_TIMEOUT, HEALTH_POLL_INTERVAL).is_ok()
        {
            return Ok(());
        }

        fs::create_dir_all(self.state_dir())?;

        let python = venv_python_path(&self.venv_dir);
        if !python.exists() {
            return Err(format!("local Python interpreter not found: {}", python.display()).into());
        }
        if !self.model_dir.exists() {
            return Err(format!("local model directory not found: {}", self.model_dir.display()).into());
        }

        let stderr_stdio = match &self.log_file {
            Some(path) => Stdio::from(File::create(path)?),
            None => Stdio::null(),
        };

        let child = Command::new(python)
            .arg(self.server_script_path())
            .arg("--model-dir")
            .arg(&self.model_dir)
            .arg("--port")
            .arg(self.port.to_string())
            .arg("--quantization")
            .arg(&self.quantization)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr_stdio)
            .spawn()?;

        fs::write(self.pid_file_path(), child.id().to_string())?;
        self.process = Some(child);
        self.owned = true;

        if let Err(error) = health_check(&self.base_url(), HEALTH_TIMEOUT, HEALTH_POLL_INTERVAL) {
            let msg = match &self.log_file {
                Some(path) => format!(
                    "{error} (see server log for details: {})",
                    path.display()
                ),
                None => error.to_string(),
            };
            self.stop();
            return Err(msg.into());
        }

        Ok(())
    }

    /// Stops the managed local server process.
    pub fn stop(&mut self) {
        let pid = if let Some(child) = self.process.as_ref() {
            Some(child.id())
        } else {
            self.read_pid()
        };

        if let Some(pid) = pid {
            let _ = terminate_pid(pid);
            wait_for_exit_or_kill(pid);
        }

        if let Some(child) = self.process.as_mut() {
            let _ = child.wait();
        }

        self.process = None;
        let _ = fs::remove_file(self.pid_file_path());
    }

    /// Stops the server only if this manager owns (spawned) it.
    /// Reused background servers started by `viberwhisper local start` are left running.
    pub fn release(&mut self) {
        if self.owned {
            self.stop();
        }
    }

    /// Returns true when the managed process is still alive.
    pub fn is_running(&self) -> bool {
        if let Some(child) = self.process.as_ref() {
            return is_pid_running(child.id());
        }

        self.read_pid().is_some_and(is_pid_running)
    }

    /// Returns the base HTTP URL for the local service.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Returns the best-effort persisted service status.
    pub fn status(&self) -> Result<LocalServiceStatus, Box<dyn std::error::Error>> {
        let pid = self.read_pid().or_else(|| self.process.as_ref().map(Child::id));
        let running = pid.is_some_and(is_pid_running);
        let health = match health_once(&self.base_url()) {
            Ok(StatusCode::OK) => "ok".to_string(),
            Ok(status) => format!("http {}", status.as_u16()),
            Err(error) => error.to_string(),
        };
        let memory_usage = pid.and_then(read_memory_usage);

        Ok(LocalServiceStatus {
            running,
            port: self.port,
            health,
            pid,
            memory_usage,
        })
    }

    fn state_dir(&self) -> PathBuf {
        self.model_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn pid_file_path(&self) -> PathBuf {
        pid_file_path(&self.state_dir())
    }

    fn server_script_path(&self) -> PathBuf {
        find_server_file("server.py")
    }

    fn read_pid(&self) -> Option<u32> {
        fs::read_to_string(self.pid_file_path())
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
    }
}

/// Locates a file inside the `server/` directory, trying the packaged location
/// (next to the executable) first, then falling back to the development source tree.
fn find_server_file(filename: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(exe_dir) = exe.parent()
    {
        let candidate = exe_dir.join("server").join(filename);
        if candidate.exists() {
            return candidate;
        }
    }

    // Fallback: compile-time source tree (works with `cargo run`).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("server")
        .join(filename)
}

fn health_check(
    base_url: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let deadline = Instant::now() + timeout;
    let url = format!("{base_url}/health");
    let mut last_error = "health check did not start".to_string();

    while Instant::now() < deadline {
        match client.get(&url).send() {
            Ok(resp) if resp.status() == StatusCode::OK => return Ok(()),
            Ok(resp) => {
                last_error = format!("service not ready: http {}", resp.status().as_u16());
            }
            Err(error) => last_error = error.to_string(),
        }

        std::thread::sleep(poll_interval);
    }

    Err(last_error.into())
}

fn health_once(base_url: &str) -> Result<StatusCode, Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    Ok(client.get(format!("{base_url}/health")).send()?.status())
}

/// Waits up to 5 seconds for a process to exit after SIGTERM; sends SIGKILL if it does not.
fn wait_for_exit_or_kill(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !is_pid_running(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Process did not exit in time; force kill.
    let _ = force_kill_pid(pid);
}

fn force_kill_pid(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()?;

    #[cfg(not(target_os = "windows"))]
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to force-kill process {pid}").into())
    }
}

fn pid_file_path(base_dir: &Path) -> PathBuf {
    base_dir.join("local_server.pid")
}

fn is_pid_running(pid: u32) -> bool {
    #[cfg(target_os = "windows")]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()
            .map(|output| {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.contains(&format!(",\"{pid}\""))
            })
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .ok()
            .map(|output| {
                output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
            })
            .unwrap_or(false)
    }
}

fn terminate_pid(pid: u32) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()?;

    #[cfg(not(target_os = "windows"))]
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to terminate process {pid}").into())
    }
}

fn read_memory_usage(pid: u32) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let columns = stdout.trim().trim_matches('"').split("\",\"").collect::<Vec<_>>();
        columns.get(4).map(|value| value.to_string())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "rss="])
            .output()
            .ok()?;
        let rss = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if rss.is_empty() {
            None
        } else {
            Some(format!("{rss} KB"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_health_stub(port: u16, status_line: &'static str, body: &'static str) {
        thread::spawn(move || {
            let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "{status_line}\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
    }

    #[test]
    fn test_base_url_uses_loopback_port() {
        let manager =
            LocalServiceManager::new(17265, PathBuf::from("model"), PathBuf::from("venv"));
        assert_eq!(manager.base_url(), "http://127.0.0.1:17265");
    }

    #[test]
    fn test_health_check_accepts_ok_response() {
        let port = 18765;
        spawn_health_stub(port, "HTTP/1.1 200 OK", "{\"status\":\"ok\"}");
        let result = health_check(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_secs(1),
            Duration::from_millis(25),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_health_check_times_out_on_unhealthy_server() {
        let port = 18766;
        spawn_health_stub(port, "HTTP/1.1 503 Service Unavailable", "{\"status\":\"loading\"}");
        let result = health_check(
            &format!("http://127.0.0.1:{port}"),
            Duration::from_millis(150),
            Duration::from_millis(25),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_pid_file_path_uses_expected_name() {
        assert_eq!(
            pid_file_path(Path::new("/tmp/viberwhisper")),
            PathBuf::from("/tmp/viberwhisper/local_server.pid")
        );
    }
}
