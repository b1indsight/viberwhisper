//! Real UIA target capture and desktop repaint checks, inside the supervised native child.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

impl NativePreview {
    /// Check real UIA placement and old/new desktop pixels without capturing another app.
    pub(crate) fn smoke_geometry() -> Result<()> {
        use std::os::windows::process::CommandExt;

        let temporary = tempfile::tempdir()?;
        let directory = std::env::var_os("VIBERWHISPER_PREVIEW_ARTIFACTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| temporary.path().to_owned());
        std::fs::create_dir_all(&directory)?;
        // The outer native-test job owns this process tree and its sole hard deadline, as
        // for scrollbar_probe.rs. Unique protocol files also allow repeated CI invocations.
        let protocol = tempfile::tempdir()?;
        let system = std::env::var_os("WINDIR").ok_or_else(|| anyhow!("WINDIR is missing"))?;
        let mut child =
            Command::new(Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-STA",
                    "-File",
                ])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("src/platform/windows/preview/geometry_probe.ps1"),
                )
                .arg("-Protocol")
                .arg(protocol.path())
                .arg("-Directory")
                .arg(&directory)
                .creation_flags(0x08000000) // CREATE_NO_WINDOW; WPF owns the only target window.
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut captured = None;
        for phase in [
            "first-line",
            "last-line",
            "read-only",
            "button",
            "combo",
            "password",
        ] {
            let ready = protocol.path().join(format!("{phase}.json"));
            wait_for_probe(&mut child, &ready, deadline)?;
            let target: serde_json::Value = serde_json::from_slice(&std::fs::read(&ready)?)?;
            // Keep COM interfaces and the HWND-bearing destination on this capture worker;
            // only plain geometry and numeric identity return to the window thread.
            let (window, anchor) = thread::spawn(|| {
                let window = unsafe { GetForegroundWindow() };
                let thread = unsafe { GetWindowThreadProcessId(window, null_mut()) };
                let mut info = GUITHREADINFO {
                    cbSize: size_of::<GUITHREADINFO>() as u32,
                    ..Default::default()
                };
                ensure!(
                    unsafe { GetGUIThreadInfo(thread, &mut info) } != 0 && info.hwndCaret.is_null(),
                    "UIA fixture unexpectedly supplied a native caret"
                );
                NativeTarget::capture().map(|target| (target.window as usize, target.anchor))
            })
            .join()
            .map_err(|_| anyhow!("Geometry capture worker panicked"))??;
            ensure!(
                Some(window as u64) == target["window"].as_u64(),
                "Capture escaped the owned WPF target ({phase})"
            );
            if matches!(phase, "read-only" | "button" | "combo" | "password") {
                ensure!(
                    anchor.is_none(),
                    "Non-editable/password target supplied an anchor"
                );
            } else {
                let anchor = anchor.ok_or_else(|| anyhow!("UIA editor has no anchor ({phase})"))?;
                let expected: [f64; 4] = serde_json::from_value(target["caret"].clone())?;
                let input: [f64; 4] = serde_json::from_value(target["input"].clone())?;
                let close = |expected: [f64; 4]| {
                    [anchor.x, anchor.y, anchor.width, anchor.height]
                        .into_iter()
                        .zip(expected)
                        .all(|(actual, expected)| (actual - expected).abs() <= 3.0)
                };
                // WPF reports physical caret geometry independently. Providers allowed to
                // return no degenerate-range rectangle must fall back to this input only.
                ensure!(
                    close(expected) || close(input),
                    "Wrong UIA screen geometry: {anchor:?}; {target}"
                );
                eprintln!("native UIA anchor {phase}: {anchor:?}; expected={target}");
                captured = Some(anchor);
            }
            std::fs::write(protocol.path().join(format!("{phase}.done")), [])?;
        }
        wait_for_probe(
            &mut child,
            &protocol.path().join("backdrop.ready"),
            deadline,
        )?;
        let anchor = captured.ok_or_else(|| anyhow!("No editable target captured"))?;
        let work = position::work_area(anchor).ok_or_else(|| anyhow!("No target work area"))?;
        let destination = unsafe { GetForegroundWindow() };
        let mut preview = Self::new(Box::new(|_| {}));
        preview.show_streaming(SessionId(1), "让", "Geometry fixture", Some(anchor), false)?;
        let window = preview.state.window.get();
        let long = "中文 English 自动换行。\n".repeat(35);
        let mut previous: Option<[i32; 4]> = None;
        for (corner, anchor) in [
            anchor,
            PreviewRect::new(
                work.x + work.width - 12.0,
                work.y + work.height - 32.0,
                1.0,
                18.0,
            )
            .unwrap(),
        ]
        .into_iter()
        .enumerate()
        {
            preview.state.anchor.set(Some(anchor));
            for (index, (text, ready, warning)) in [
                ("让", false, None),
                ("让我看看吧，求求你了。", false, None),
                (long.as_str(), false, None),
                ("短句", false, None),
                ("", false, None),
                (long.as_str(), true, Some("测试提示：检查文字后确认。")),
                ("短句", true, None),
            ]
            .into_iter()
            .enumerate()
            {
                preview.update_text(text, ready, warning)?;
                ensure!(
                    preview.state.window.get() == window
                        && unsafe { GetForegroundWindow() } == destination,
                    "Resize replaced the HWND or stole destination focus"
                );
                let bounds = screen_bounds(window)?;
                let old = previous.unwrap_or(bounds);
                let capture = [
                    old[0].min(bounds[0]) - 2,
                    old[1].min(bounds[1]) - 2,
                    old[2].max(bounds[2]) + 2,
                    old[3].max(bounds[3]) + 2,
                ];
                let label = format!("{corner}-{index}");
                // This freshly drawn reference does not invalidate/repair the desktop.
                // Compare only parent chrome, excluding EDIT/caret/DComp and footer text.
                preview.smoke_capture(&directory.join(format!("geometry-expected-{label}.png")))?;
                let excluded = [preview.state.text.get(), preview.state.status.get()]
                    .into_iter()
                    .filter(|control| unsafe { IsWindowVisible(*control) } != 0)
                    .map(screen_bounds)
                    .collect::<Result<Vec<_>>>()?;
                let request = protocol.path().join(format!("frame-{label}.json"));
                let temporary_request = request.with_extension("tmp");
                std::fs::write(
                    &temporary_request,
                    serde_json::to_vec(&serde_json::json!({
                        "window": window as usize, "bounds": bounds, "capture": capture,
                        "excluded": excluded, "label": label,
                    }))?,
                )?;
                std::fs::rename(temporary_request, request)?;
                wait_for_probe(
                    &mut child,
                    &protocol.path().join(format!("frame-{label}.done")),
                    deadline,
                )?;
                previous = Some(bounds);
            }
        }
        std::fs::write(protocol.path().join("finish"), [])?;
        let status = child.wait()?;
        ensure!(status.success(), "Native geometry probe failed: {status}");
        Ok(())
    }
}

fn screen_bounds(window: HWND) -> Result<[i32; 4]> {
    let mut bounds = RECT::default();
    ensure!(
        unsafe { GetWindowRect(window, &mut bounds) } != 0,
        "Could not read fixture bounds"
    );
    Ok([bounds.left, bounds.top, bounds.right, bounds.bottom])
}

fn wait_for_probe(child: &mut Child, path: &Path, deadline: Instant) -> Result<()> {
    while !path.exists() {
        ensure!(
            child.try_wait()?.is_none(),
            "Geometry probe exited before {}",
            path.display()
        );
        ensure!(
            Instant::now() < deadline,
            "Geometry probe timed out at {}",
            path.display()
        );
        let mut message = MSG::default();
        // As in scrollbar_probe::desktop_driver, UIA/paint callbacks need this owner thread.
        unsafe {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
