//! Editable native preview with the production destination capture and delivery path.
//! No microphone, network, configuration or persistent history is needed.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

use super::preview::{
    FinalizationGate, PendingPreview, PreviewCommand, PreviewUpdate, wait_for_confirmation,
};
use crate::platform::{
    DeliveryTarget, NativePreview, NativeTarget, PreviewAction, PreviewEvent, PreviewRect,
};
use crate::session::SessionId;

const SAMPLE_TEXT: &str = "我们明天上午去上海。";
const SMOKE_TEXT: &str = "已编辑的文字：中文、English 和 emoji 🎙️。";

enum DemoEvent {
    Update(PreviewUpdate),
    Progress {
        text: String,
        destination: String,
        anchor: Option<PreviewRect>,
        applied: mpsc::SyncSender<()>,
    },
    Input(PreviewEvent),
    Finished,
}

/// Gives the user three seconds to focus a destination, then opens the editable
/// bubble. Enter really delivers the edited text. `smoke` uses the already focused
/// destination and tests native editing, held Enter and production delivery.
/// On macOS, `artifacts` instead runs a self-contained streaming/render fixture;
/// it never captures a destination, injects text, or changes persistent history.
pub fn run_preview_demo(smoke: bool, artifacts: Option<PathBuf>) -> Result<()> {
    if let Some(directory) = &artifacts {
        std::fs::create_dir_all(directory)?;
    }
    let render_only = artifacts.is_some();
    let event_loop = EventLoop::<DemoEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let notifications = Arc::new(AtomicUsize::new(0));
    let notified = Arc::clone(&notifications);
    let ui_proxy = proxy.clone();
    let gate = Arc::new(FinalizationGate::new());
    let worker_gate = Arc::clone(&gate);
    let typer = crate::platform::preview_typer();
    if render_only {
        println!("Rendering streaming preview fixture without a delivery target.");
    } else {
        println!("Focus a text field within 3 seconds; Enter will insert the edited text.");
    }
    let worker = std::thread::spawn(move || {
        if !smoke && !render_only {
            std::thread::sleep(Duration::from_secs(3));
        }
        let target = if render_only {
            Err(anyhow!("Render fixture has no delivery target"))
        } else {
            NativeTarget::capture()
        };
        let destination = target
            .as_ref()
            .map_or_else(|_| "原应用".into(), DeliveryTarget::name);
        let anchor = target
            .as_ref()
            .ok()
            .and_then(DeliveryTarget::preview_anchor);
        for text in ["我们明天", "我们明天下午去北京。"] {
            let (applied, receiver) = mpsc::sync_channel(0);
            if proxy
                .send_event(DemoEvent::Progress {
                    text: text.into(),
                    destination: destination.clone(),
                    anchor,
                    applied,
                })
                .is_err()
                || receiver.recv().is_err()
            {
                return;
            }
            if !smoke && !render_only {
                std::thread::sleep(Duration::from_millis(900));
            }
        }
        if render_only {
            let (commands, receiver) = mpsc::channel();
            if proxy
                .send_event(DemoEvent::Update(PreviewUpdate::Ready {
                    session_id: SessionId(1),
                    text: SAMPLE_TEXT.into(),
                    destination,
                    anchor,
                    warning: None,
                    commands,
                }))
                .is_ok()
            {
                let _ = receiver.recv();
            }
        } else {
            wait_for_confirmation(
                SessionId(1),
                (SAMPLE_TEXT.into(), None).into(),
                typer.as_ref(),
                None,
                &worker_gate,
                || target,
                &|update| proxy.send_event(DemoEvent::Update(update)).is_ok(),
            );
        }
        let _ = proxy.send_event(DemoEvent::Finished);
    });
    let mut demo = Demo {
        preview: NativePreview::new(Box::new(move |event| {
            notified.fetch_add(1, Ordering::Relaxed);
            let _ = ui_proxy.send_event(DemoEvent::Input(event));
        })),
        smoke,
        sent: false,
        streaming: false,
        artifacts,
        pending: None,
        deadline: Instant::now() + Duration::from_secs(10),
        error: None,
        notifications,
        gate,
    };
    let result = event_loop.run_app(&mut demo);
    demo.gate.cancel();
    demo.pending.take();
    worker
        .join()
        .map_err(|_| anyhow!("Preview delivery worker panicked"))?;
    result?;
    if let Some(error) = demo.error {
        return Err(anyhow!(error));
    }
    Ok(())
}

struct Demo {
    preview: NativePreview,
    smoke: bool,
    sent: bool,
    streaming: bool,
    artifacts: Option<PathBuf>,
    pending: Option<PendingPreview>,
    deadline: Instant,
    error: Option<String>,
    notifications: Arc<AtomicUsize>,
    gate: Arc<FinalizationGate>,
}

impl ApplicationHandler<DemoEvent> for Demo {
    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DemoEvent) {
        match event {
            DemoEvent::Progress {
                text,
                destination,
                anchor,
                applied,
            } => {
                let display = if self.streaming {
                    self.preview.update_text(&text, false, None)
                } else {
                    self.preview
                        .show_streaming(SessionId(1), &text, &destination, anchor, true)
                };
                if let Err(error) = display.and_then(|_| {
                    if (self.smoke || self.artifacts.is_some()) && self.streaming {
                        self.preview.smoke_streaming()?;
                        self.preview.update_text(&text, false, None)?;
                        anyhow::ensure!(
                            self.notifications.load(Ordering::Relaxed) == 0,
                            "Generating preview confirmed before completion"
                        );
                    }
                    self.capture(if self.streaming {
                        "streaming-draft.png"
                    } else {
                        "streaming-prefix.png"
                    })?;
                    Ok(())
                }) {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
                self.streaming = true;
                let _ = applied.send(());
            }
            DemoEvent::Update(PreviewUpdate::Ready {
                session_id,
                text,
                destination,
                anchor,
                warning,
                commands,
            }) => {
                self.pending = Some(PendingPreview::new(session_id, commands));
                if self.smoke {
                    println!("Preview destination: {destination}; captured anchor: {anchor:?}");
                    if let Some(warning) = warning.as_deref() {
                        println!("Preview warning: {warning}");
                    }
                }
                let display = if self.streaming {
                    self.preview.update_text(&text, true, warning.as_deref())
                } else {
                    self.preview
                        .show(session_id, &text, &destination, anchor, warning.as_deref())
                };
                if let Err(error) = display {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                }
                if self.artifacts.is_some() {
                    if let Err(error) = self.capture("ready.png") {
                        self.error = Some(error.to_string());
                    }
                    let _ = self
                        .pending
                        .as_ref()
                        .unwrap()
                        .commands
                        .send(PreviewCommand::Cancel);
                }
            }
            DemoEvent::Input(event) => {
                if self.pending.is_none() && matches!(event.action, PreviewAction::Cancel) {
                    self.gate.cancel();
                    self.preview.close();
                    event_loop.exit();
                    return;
                }
                let Some(pending) = &mut self.pending else {
                    return;
                };
                let command = match event.action {
                    PreviewAction::Confirm(text) if pending.begin(event.session_id) => {
                        if self.smoke && text != SMOKE_TEXT {
                            self.error = Some(format!("Preview returned unedited text: {text}"));
                            event_loop.exit();
                            return;
                        }
                        println!("Preview confirmed: {text}");
                        PreviewCommand::Confirm(text)
                    }
                    PreviewAction::Cancel if pending.begin_cancel(event.session_id) => {
                        PreviewCommand::Cancel
                    }
                    _ => return,
                };
                self.preview.hide();
                if pending.commands.send(command).is_err() {
                    self.error = Some("Preview delivery worker disconnected".into());
                    event_loop.exit();
                }
            }
            DemoEvent::Update(PreviewUpdate::DeliveryFailed {
                session_id,
                error,
                retry_allowed,
            }) => {
                if !self
                    .pending
                    .as_mut()
                    .is_some_and(|pending| pending.failed(session_id, retry_allowed))
                {
                    return;
                }
                eprintln!("Preview delivery failed: {error}");
                if self.smoke {
                    self.error = Some(error);
                    event_loop.exit();
                } else {
                    self.preview.show_error(&error, retry_allowed);
                }
            }
            DemoEvent::Finished => {
                if self.smoke && self.notifications.load(Ordering::Relaxed) != 1 {
                    self.error = Some("Expected exactly one native confirmation".into());
                }
                self.preview.close();
                event_loop.exit();
            }
            DemoEvent::Update(
                PreviewUpdate::Streaming { .. }
                | PreviewUpdate::HistorySaved(_)
                | PreviewUpdate::PermissionRequired,
            ) => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.smoke {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if Instant::now() >= self.deadline {
            self.error = Some("Native preview delivery timed out".into());
            event_loop.exit();
            return;
        }
        if self.pending.is_some() && !self.sent {
            match self.preview.smoke_confirmation(SMOKE_TEXT, &|| {
                anyhow::ensure!(
                    self.notifications.load(Ordering::Relaxed) == 0,
                    "Native window confirmed before Enter was released"
                );
                Ok(())
            }) {
                Ok(sent) => self.sent = sent,
                Err(error) => {
                    self.error = Some(error.to_string());
                    event_loop.exit();
                    return;
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(25),
        ));
    }

    fn window_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::window::WindowId,
        _: winit::event::WindowEvent,
    ) {
    }
}

impl Demo {
    fn capture(&self, name: &str) -> Result<()> {
        if let Some(directory) = &self.artifacts {
            #[cfg(target_os = "macos")]
            self.preview.smoke_capture(&directory.join(name))?;
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (directory, name);
                anyhow::bail!("Preview artifact capture currently requires macOS");
            }
        }
        Ok(())
    }
}
