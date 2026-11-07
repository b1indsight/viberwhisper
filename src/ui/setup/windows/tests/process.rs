//! Parent-owned lifetime and input cleanup for the serialized native GUI fixtures.

use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::ptr::{null, null_mut};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput, VK_CONTROL, VK_LBUTTON,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GUITHREADINFO, GetGUIThreadInfo, GetPhysicalCursorPos, GetWindowThreadProcessId,
    SetPhysicalCursorPos,
};

use super::{CHILD_ENV, TEST_NAME};

const CLEANUP_BUDGET: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);
const TERMINATED: u32 = 0xe0000001;

/// A single deadline covers every client, independently of the GUI's modal message pump.
pub(super) fn run(phase: &str, budget: Duration) -> Result<()> {
    let started = Instant::now();
    let mut fixture = NativeChild::new(phase)?;
    let stdout = drain(
        fixture
            .child
            .stdout
            .take()
            .context("missing native stdout")?,
    );
    let stderr = drain(
        fixture
            .child
            .stderr
            .take()
            .context("missing native stderr")?,
    );

    if phase == "input-cleanup" {
        // Exercise the admission race: a not-yet-contained process must stay inert even if the
        // parent is descheduled. Stdin EOF is the parent's-death path, tested separately below.
        let admission = Instant::now() + Duration::from_secs(2);
        while Instant::now() < admission {
            let mut owned = (fixture.child.id(), false);
            let mut cursor = POINT::default();
            ensure!(
                unsafe { EnumWindows(Some(find_owned_window), (&mut owned as *mut _) as LPARAM) }
                    != 0
                    && unsafe { GetPhysicalCursorPos(&mut cursor) } != 0,
                "could not inspect admission desktop"
            );
            ensure!(
                !owned.1
                    && fixture.active_processes()? == 0
                    && pressed_inputs() == [false; 2]
                    && cursor.x == fixture.cursor.x
                    && cursor.y == fixture.cursor.y,
                "native child acted before job admission"
            );
            ensure!(fixture.child.try_wait()?.is_none(), "gated child exited");
            thread::sleep(POLL);
        }
    }
    if phase == "gate-denied" {
        drop(fixture.child.stdin.take());
    } else {
        fixture.admit()?;
    }

    let outcome = (|| -> Result<_> {
        loop {
            if started.elapsed() >= budget {
                return Ok((None, false));
            }
            if let Some(status) = fixture.child.try_wait()? {
                return Ok((Some(status), false));
            }
            if phase == "input-cleanup" && pressed_inputs() == [true; 2] {
                let mut gui = GUITHREADINFO {
                    cbSize: size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                let mut owner = 0;
                // Observe actual native capture, not a driver marker or a manufactured message.
                unsafe {
                    ensure!(GetGUIThreadInfo(0, &mut gui) != 0, "cannot inspect capture");
                    GetWindowThreadProcessId(gui.hwndCapture, &mut owner);
                }
                if owner == fixture.child.id() && fixture.active_processes()? >= 2 {
                    eprintln!(
                        "native cleanup fault: Ctrl/left down, owned capture and live driver"
                    );
                    return Ok((None, true));
                }
            }
            thread::sleep(POLL);
        }
    })();

    // Stop all possible future senders before releasing input. A shared cleanup deadline also
    // bounds pipe collection; detached readers cannot hold up a failing test indefinitely.
    let cleanup_deadline = Instant::now() + CLEANUP_BUDGET;
    let cleanup = fixture.cleanup(cleanup_deadline);
    while !stdout.is_finished() || !stderr.is_finished() {
        ensure!(
            Instant::now() < cleanup_deadline,
            "native output did not close after cleanup; cleanup={cleanup:?}"
        );
        thread::sleep(POLL);
    }
    let stdout = stdout
        .join()
        .map_err(|_| anyhow!("stdout reader panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| anyhow!("stderr reader panicked"))??;
    print!("{}", String::from_utf8_lossy(&stdout));
    eprint!("{}", String::from_utf8_lossy(&stderr));
    cleanup?;
    let (status, forced) = outcome?;
    match phase {
        "gate-denied" => ensure!(
            status.is_some_and(|value| !value.success())
                && String::from_utf8_lossy(&stderr).contains("native fixture admission was denied"),
            "denied admission did not fail before UI initialization"
        ),
        "input-cleanup" => ensure!(
            forced && status.is_none(),
            "cleanup fault never reached held input/capture; status={status:?}"
        ),
        _ => ensure!(
            status.is_some_and(|value| value.success()),
            "native phase failed or exceeded {budget:?}; status={status:?}"
        ),
    }
    eprintln!(
        "native {phase} finished in {:?}; owned processes=0; input released; pointer restored",
        started.elapsed()
    );
    Ok(())
}

struct NativeChild {
    child: Child,
    job: OwnedHandle,
    cursor: POINT,
    contained: bool,
    cleanup_attempted: bool,
}

impl NativeChild {
    fn new(phase: &str) -> Result<Self> {
        ensure!(
            pressed_inputs() == [false; 2],
            "desktop input is already held"
        );
        let mut cursor = POINT::default();
        ensure!(
            unsafe { GetPhysicalCursorPos(&mut cursor) } != 0,
            "cannot save pointer"
        );
        let job = unsafe { CreateJobObjectW(null(), null()) };
        ensure!(
            !job.is_null(),
            "cannot create native job: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: This newly created non-null handle has exactly one owner and is not inherited.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        ensure!(
            unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of_val(&limits) as u32,
                )
            } != 0,
            "cannot configure native job: {}",
            std::io::Error::last_os_error()
        );
        let child = Command::new(std::env::current_exe()?)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, phase)
            .env("SSH_CLIENT", "127.0.0.1 12345 22")
            .env_remove("DISPLAY")
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(Self {
            child,
            job,
            cursor,
            contained: false,
            cleanup_attempted: false,
        })
    }

    fn admit(&mut self) -> Result<()> {
        ensure!(
            unsafe {
                AssignProcessToJobObject(self.job.as_raw_handle(), self.child.as_raw_handle())
            } != 0,
            "cannot contain native child: {}",
            std::io::Error::last_os_error()
        );
        self.contained = true;
        let mut contained = 0;
        ensure!(
            unsafe {
                IsProcessInJob(
                    self.child.as_raw_handle(),
                    self.job.as_raw_handle(),
                    &mut contained,
                )
            } != 0
                && contained != 0,
            "native child containment was not confirmed"
        );
        self.child
            .stdin
            .take()
            .context("missing admission pipe")?
            .write_all(b"G")?;
        Ok(())
    }

    fn active_processes(&self) -> Result<u32> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        ensure!(
            unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of_val(&info) as u32,
                    null_mut(),
                )
            } != 0,
            "cannot inspect native job: {}",
            std::io::Error::last_os_error()
        );
        Ok(info.ActiveProcesses)
    }

    /// Quiesce the process tree before releasing input, sharing one bounded cleanup interval.
    fn cleanup(&mut self, deadline: Instant) -> Result<()> {
        self.cleanup_attempted = true;
        drop(self.child.stdin.take());
        let stopped = (|| -> Result<()> {
            if !self.contained && self.child.try_wait()?.is_none() {
                self.child.kill()?;
            }
            ensure!(
                unsafe { TerminateJobObject(self.job.as_raw_handle(), TERMINATED) } != 0,
                "cannot terminate native job: {}",
                std::io::Error::last_os_error()
            );
            while self.active_processes()? != 0 || self.child.try_wait()?.is_none() {
                ensure!(Instant::now() < deadline, "native process exit unconfirmed");
                thread::sleep(POLL);
            }
            Ok(())
        })();
        // Best-effort release still runs if termination cannot be confirmed, but that phase
        // must fail. The normal path does not release until every contained process is gone.
        let restored = (|| -> Result<()> {
            let release = [
                INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dwFlags: MOUSEEVENTF_LEFTUP,
                            ..Default::default()
                        },
                    },
                },
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_CONTROL,
                            dwFlags: KEYEVENTF_KEYUP,
                            ..Default::default()
                        },
                    },
                },
            ];
            let sent = unsafe {
                SendInput(
                    release.len() as u32,
                    release.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            };
            let moved = unsafe { SetPhysicalCursorPos(self.cursor.x, self.cursor.y) };
            ensure!(
                sent == release.len() as u32 && moved != 0,
                "native input release/restore failed"
            );
            loop {
                let mut cursor = POINT::default();
                ensure!(
                    unsafe { GetPhysicalCursorPos(&mut cursor) } != 0,
                    "cannot verify pointer"
                );
                if pressed_inputs() == [false; 2]
                    && cursor.x == self.cursor.x
                    && cursor.y == self.cursor.y
                {
                    return Ok(());
                }
                ensure!(
                    Instant::now() < deadline,
                    "native input cleanup unconfirmed"
                );
                thread::sleep(POLL);
            }
        })();
        stopped?;
        restored
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        if !self.cleanup_attempted
            && let Err(error) = self.cleanup(Instant::now() + CLEANUP_BUDGET)
        {
            let _ = writeln!(std::io::stderr(), "native cleanup failed: {error:#}");
        }
    }
}

fn pressed_inputs() -> [bool; 2] {
    [VK_LBUTTON, VK_CONTROL].map(|key| unsafe { GetAsyncKeyState(key as i32) } < 0)
}

fn drain(mut pipe: impl Read + Send + 'static) -> JoinHandle<std::io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

unsafe extern "system" fn find_owned_window(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: EnumWindows receives a live, uniquely borrowed (PID, found) pair from run.
    let owned = unsafe { &mut *(data as *mut (u32, bool)) };
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    owned.1 |= process == owned.0;
    1
}
