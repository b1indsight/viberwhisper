//! Shared preview protocol, confirmation state and worker-owned text delivery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use anyhow::Result;
use tracing::{debug, error};

use crate::dictation::Confirmation;
use crate::history::HistoryStore;
use crate::input::typer::TextTyper;
use crate::platform::{DeliveryTarget, PreviewRect};
use crate::session::SessionId;

/// Preview content and the optional capability to learn after successful delivery.
pub(super) struct PreviewTranscription {
    pub text: String,
    pub warning: Option<String>,
    pub confirmation: Option<Confirmation>,
}

impl From<(String, Option<String>)> for PreviewTranscription {
    fn from((text, warning): (String, Option<String>)) -> Self {
        Self {
            text,
            warning,
            confirmation: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum PreviewUpdate {
    Ready {
        session_id: SessionId,
        text: String,
        destination: String,
        anchor: Option<PreviewRect>,
        warning: Option<String>,
        commands: mpsc::Sender<PreviewCommand>,
    },
    HistorySaved(String),
    PermissionRequired,
    DeliveryFailed {
        session_id: SessionId,
        error: String,
        retry_allowed: bool,
    },
}

#[derive(Debug, Clone)]
pub(super) enum PreviewCommand {
    Confirm(String),
    Cancel,
}

enum ConfirmationPhase {
    Ready,
    Processing,
    RetryBlocked,
}

/// A candidate accepts one confirmation at a time, including queued native callbacks.
pub(super) struct PendingPreview {
    pub(super) session_id: SessionId,
    pub(super) commands: mpsc::Sender<PreviewCommand>,
    phase: ConfirmationPhase,
}

impl PendingPreview {
    pub(super) fn new(session_id: SessionId, commands: mpsc::Sender<PreviewCommand>) -> Self {
        Self {
            session_id,
            commands,
            phase: ConfirmationPhase::Ready,
        }
    }

    pub(super) fn begin(&mut self, session_id: SessionId) -> bool {
        if self.session_id != session_id || !matches!(self.phase, ConfirmationPhase::Ready) {
            return false;
        }
        self.phase = ConfirmationPhase::Processing;
        true
    }

    /// A blocked retry can still be cancelled; an accepted command must finish first.
    pub(super) fn begin_cancel(&mut self, session_id: SessionId) -> bool {
        if self.session_id != session_id || self.is_busy() {
            return false;
        }
        self.phase = ConfirmationPhase::Processing;
        true
    }

    pub(super) fn is_busy(&self) -> bool {
        matches!(self.phase, ConfirmationPhase::Processing)
    }

    pub(super) fn failed(&mut self, session_id: SessionId, retry_allowed: bool) -> bool {
        if self.session_id != session_id {
            return false;
        }
        self.phase = if retry_allowed {
            ConfirmationPhase::Ready
        } else {
            ConfirmationPhase::RetryBlocked
        };
        true
    }
}

/// Cancellation is also checked by the typer after its platform focus delay.
pub(super) struct FinalizationGate(AtomicBool);

impl FinalizationGate {
    pub(super) fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(super) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(super) fn type_text_if_active(
        &self,
        typer: &dyn TextTyper,
        text: &str,
        validate: &dyn Fn() -> Result<()>,
    ) -> Result<bool> {
        if self.is_cancelled() {
            return Ok(false);
        }
        typer.type_text_checked(text, &|| {
            anyhow::ensure!(!self.is_cancelled(), "Text delivery cancelled");
            validate()
        })?;
        Ok(true)
    }
}

/// Commits only on explicit confirmation. The worker owns its native destination
/// throughout preview, retries and delivery, without moving native objects between threads.
pub(super) fn confirm_delivery(
    typer: &dyn TextTyper,
    target: &dyn DeliveryTarget,
    text: &str,
    history: Option<&HistoryStore>,
    saved: &mut Option<String>,
    gate: &FinalizationGate,
    notify_saved: impl FnOnce(String),
) -> Result<bool> {
    if gate.is_cancelled() {
        return Ok(false);
    }
    target.restore()?;
    if gate.is_cancelled() {
        return Ok(false);
    }
    // Validate before persisting and once more after the platform's focus delay.
    target.validate()?;
    if saved.as_deref() != Some(text)
        && let Some(history) = history
    {
        match history.append(text) {
            Ok(()) => {
                *saved = Some(text.to_owned());
                notify_saved(text.to_owned());
            }
            Err(error) => tracing::error!(%error, "Failed to persist confirmed transcription"),
        }
    }
    gate.type_text_if_active(typer, text, &|| target.validate())
}

/// Keeps native destination objects on this worker until confirmation or cancellation.
/// Capture and event publishing are injected so the real command loop can be tested
/// without a desktop, network, or native target crossing a thread boundary.
pub(super) fn wait_for_confirmation<T: DeliveryTarget>(
    session_id: SessionId,
    transcription: PreviewTranscription,
    typer: &dyn TextTyper,
    history: Option<&HistoryStore>,
    gate: &FinalizationGate,
    capture: impl FnOnce() -> Result<T>,
    notify: &impl Fn(PreviewUpdate) -> bool,
) {
    if gate.is_cancelled() {
        return;
    }
    let PreviewTranscription {
        text,
        warning,
        mut confirmation,
    } = transcription;
    let target = capture();
    let anchor = target
        .as_ref()
        .ok()
        .and_then(DeliveryTarget::preview_anchor);
    let destination = target
        .as_ref()
        .map_or_else(|_| "无法确定输入目标".into(), DeliveryTarget::name);
    let capture_permission_denied = target
        .as_ref()
        .err()
        .is_some_and(crate::platform::is_text_delivery_permission_error);
    let warning = if target.is_err() {
        let unavailable = "无法获取输入目标；可复制文字后手动粘贴。";
        Some(warning.map_or_else(
            || unavailable.into(),
            |warning| format!("{warning}\n{unavailable}"),
        ))
    } else {
        warning
    };
    let (commands, receiver) = mpsc::channel();
    if gate.is_cancelled()
        || !notify(PreviewUpdate::Ready {
            session_id,
            text,
            destination,
            anchor,
            warning,
            commands,
        })
    {
        return;
    }
    let mut saved = None;
    while let Ok(command) = receiver.recv() {
        if gate.is_cancelled() {
            return;
        }
        match command {
            PreviewCommand::Cancel => {
                if let Ok(target) = &target
                    && let Err(error) = target.restore()
                {
                    debug!(%error, "Could not restore destination after preview cancellation");
                }
                return;
            }
            PreviewCommand::Confirm(text) => {
                if text.trim().is_empty() {
                    if let Ok(target) = &target
                        && let Err(error) = target.restore()
                    {
                        debug!(%error, "Could not restore destination after clearing preview");
                    }
                    return;
                }
                let result = match &target {
                    Ok(target) => {
                        confirm_delivery(typer, target, &text, history, &mut saved, gate, |text| {
                            notify(PreviewUpdate::HistorySaved(text));
                        })
                    }
                    Err(error) => Err(anyhow::anyhow!("无法恢复输入目标：{error}")),
                };
                match result {
                    Ok(delivered) => {
                        if delivered
                            && let Some(confirmation) = confirmation.take()
                            && let Err(error) = confirmation.confirm(text)
                        {
                            error!(%error, "Confirmed text was delivered but dictation learning failed");
                        }
                        return;
                    }
                    Err(error) => {
                        if gate.is_cancelled() {
                            return;
                        }
                        error!(%error, "Confirmed text delivery failed");
                        if capture_permission_denied
                            || crate::platform::is_text_delivery_permission_error(&error)
                        {
                            notify(PreviewUpdate::PermissionRequired);
                        }
                        let retry_allowed = !error.is::<crate::input::typer::PartialTextDelivery>();
                        if !notify(PreviewUpdate::DeliveryFailed {
                            session_id,
                            error: error.to_string(),
                            retry_allowed,
                        }) {
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use anyhow::{Result, bail};
    use tempfile::tempdir;

    use super::*;
    use crate::platform::DeliveryTarget;

    struct Target(Cell<bool>);

    impl DeliveryTarget for Target {
        fn name(&self) -> String {
            "Test editor".into()
        }
        fn restore(&self) -> Result<()> {
            if self.0.get() {
                Ok(())
            } else {
                bail!("destination closed")
            }
        }
        fn validate(&self) -> Result<()> {
            self.restore()
        }
    }

    struct RetryingTyper(std::sync::Mutex<usize>, HistoryStore);

    #[test]
    fn preview_receives_captured_coordinates_before_focus_restoration() {
        use crate::platform::PreviewRect;
        struct Positioned<'a>(&'a Cell<bool>, PreviewRect);
        impl DeliveryTarget for Positioned<'_> {
            fn name(&self) -> String {
                "Test editor".into()
            }
            fn preview_anchor(&self) -> Option<PreviewRect> {
                Some(self.1)
            }
            fn restore(&self) -> Result<()> {
                self.0.set(true);
                Ok(())
            }
            fn validate(&self) -> Result<()> {
                Ok(())
            }
        }
        struct UnusedTyper;
        impl TextTyper for UnusedTyper {
            fn type_text(&self, _: &str) -> Result<()> {
                panic!("cancel must not type")
            }
        }
        // The UI must receive coordinates while the original caret still owns
        // focus; only plain geometry crosses the worker/UI boundary.
        let restored = Cell::new(false);
        let anchor = PreviewRect::new(100.0, 200.0, 0.0, 18.0).unwrap();
        wait_for_confirmation(
            SessionId(23),
            ("candidate".into(), None).into(),
            &UnusedTyper,
            None,
            &FinalizationGate::new(),
            || Ok(Positioned(&restored, anchor)),
            &|event| {
                if let PreviewUpdate::Ready {
                    anchor: captured,
                    commands,
                    ..
                } = event
                {
                    assert_eq!(captured, Some(anchor));
                    assert!(!restored.get());
                    commands.send(PreviewCommand::Cancel).unwrap();
                }
                true
            },
        );
        assert!(restored.get());
    }

    impl TextTyper for RetryingTyper {
        fn type_text(&self, _: &str) -> Result<()> {
            assert!(
                !self.1.load_recent().unwrap().is_empty(),
                "confirmed text must be recoverable before injection"
            );
            let mut calls = self.0.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                bail!("temporary delivery failure")
            }
            Ok(())
        }
    }

    #[test]
    fn failed_confirmation_retry_saves_history_once() {
        // A user retries after the destination rejected the first paste; history must
        // retain the confirmed text without adding it again on that retry.
        let directory = tempdir().unwrap();
        let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
        let target = Target(Cell::new(true));
        let typer = RetryingTyper(std::sync::Mutex::new(0), history.clone());
        let gate = FinalizationGate::new();
        let mut saved = None;
        let saves = Cell::new(0);
        let notify = |_| saves.set(saves.get() + 1);

        assert!(
            confirm_delivery(
                &typer,
                &target,
                "待确认文字",
                Some(&history),
                &mut saved,
                &gate,
                notify
            )
            .is_err()
        );
        assert!(
            confirm_delivery(
                &typer,
                &target,
                "待确认文字",
                Some(&history),
                &mut saved,
                &gate,
                notify
            )
            .unwrap()
        );
        assert_eq!(saves.get(), 1);
        assert_eq!(history.load_recent().unwrap(), ["待确认文字"]);
        assert_eq!(*typer.0.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn failed_delivery_learns_only_the_successful_final_revision() {
        // A user edits again after a failed paste. Only the revision accepted by
        // the destination may train recognition, although both confirmations are recoverable.
        let directory = tempdir().unwrap();
        let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
        let worker_history = history.clone();
        let (confirmation, mut feedback) = Confirmation::channel();
        let (notify, events) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            wait_for_confirmation(
                SessionId(8),
                PreviewTranscription {
                    text: "识别原稿".into(),
                    warning: None,
                    confirmation: Some(confirmation),
                },
                &RetryingTyper(std::sync::Mutex::new(0), worker_history.clone()),
                Some(&worker_history),
                &FinalizationGate::new(),
                || Ok(Target(Cell::new(true))),
                &|event| notify.send(event).is_ok(),
            );
        });
        let PreviewUpdate::Ready { commands, .. } = events
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
        else {
            panic!("expected preview");
        };
        commands
            .send(PreviewCommand::Confirm("首次修改".into()))
            .unwrap();
        while !matches!(
            events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap(),
            PreviewUpdate::DeliveryFailed { .. }
        ) {}
        assert!(matches!(
            feedback.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        commands
            .send(PreviewCommand::Confirm("最终修改 🎙️\nViberWhisper".into()))
            .unwrap();
        let feedback = tokio::time::timeout(std::time::Duration::from_secs(1), &mut feedback)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(feedback.text, "最终修改 🎙️\nViberWhisper");
        feedback.acknowledged.send(Ok(())).unwrap();
        worker.join().unwrap();
        assert_eq!(
            history.load_recent().unwrap(),
            ["最终修改 🎙️\nViberWhisper", "首次修改"]
        );
    }

    #[test]
    fn edited_retry_saves_revised_text() {
        // After a failed paste, a user can correct the bubble before retrying;
        // that newly confirmed revision must also be available in history.
        let directory = tempdir().unwrap();
        let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
        let target = Target(Cell::new(true));
        let typer = RetryingTyper(std::sync::Mutex::new(0), history.clone());
        let gate = FinalizationGate::new();
        let mut saved = None;
        assert!(
            confirm_delivery(
                &typer,
                &target,
                "原稿",
                Some(&history),
                &mut saved,
                &gate,
                |_| {}
            )
            .is_err()
        );
        assert!(
            confirm_delivery(
                &typer,
                &target,
                "修正后",
                Some(&history),
                &mut saved,
                &gate,
                |_| {}
            )
            .unwrap()
        );
        assert_eq!(history.load_recent().unwrap(), ["修正后", "原稿"]);
    }

    #[test]
    fn cancelled_or_missing_destination_never_saves_or_types() {
        // Closing a destination or exiting while a candidate is pending must not
        // paste the text into whichever unrelated application gains focus next.
        let directory = tempdir().unwrap();
        let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
        let typer = RetryingTyper(std::sync::Mutex::new(0), history.clone());
        let target = Target(Cell::new(false));
        let gate = FinalizationGate::new();
        let mut saved = None;
        assert!(
            confirm_delivery(
                &typer,
                &target,
                "text",
                Some(&history),
                &mut saved,
                &gate,
                |_| {}
            )
            .is_err()
        );
        gate.cancel();
        target.0.set(true);
        assert!(
            !confirm_delivery(
                &typer,
                &target,
                "text",
                Some(&history),
                &mut saved,
                &gate,
                |_| {}
            )
            .unwrap()
        );
        assert!(saved.is_none());
        assert!(history.load_recent().unwrap().is_empty());
        assert_eq!(*typer.0.lock().unwrap(), 0);
    }

    #[test]
    fn duplicate_and_stale_confirmation_events_are_rejected() {
        // Native keyboard and close callbacks can queue before the worker replies.
        let (commands, _) = mpsc::channel();
        let mut cancelled = PendingPreview::new(SessionId(7), commands.clone());
        assert!(cancelled.begin_cancel(SessionId(7)));
        assert!(cancelled.is_busy());
        assert!(!cancelled.begin(SessionId(7)));
        let mut pending = PendingPreview::new(SessionId(7), commands);
        assert!(!pending.begin(SessionId(6)));
        assert!(!pending.begin_cancel(SessionId(6)));
        assert!(pending.begin(SessionId(7)));
        assert!(!pending.begin(SessionId(7)));
        assert!(!pending.begin_cancel(SessionId(7)));
        pending.failed(SessionId(6), true);
        assert!(!pending.begin(SessionId(7)));
        pending.failed(SessionId(7), true);
        assert!(pending.begin(SessionId(7)));
        pending.failed(SessionId(7), false);
        assert!(!pending.begin(SessionId(7)));
        assert!(!pending.begin_cancel(SessionId(6)));
        assert!(pending.begin_cancel(SessionId(7)));
        assert!(pending.is_busy());
        assert!(!pending.begin_cancel(SessionId(7)));
    }

    #[test]
    fn cancellation_during_platform_focus_delay_prevents_injection() {
        // The user exits after confirmation while the native writer is settling
        // focus. Validation must recheck cancellation immediately before typing.
        struct SettlingTyper<'a> {
            gate: &'a FinalizationGate,
            calls: std::sync::atomic::AtomicUsize,
        }
        impl TextTyper for SettlingTyper<'_> {
            fn type_text(&self, _: &str) -> Result<()> {
                self.calls
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            fn type_text_checked(
                &self,
                text: &str,
                validate: &dyn Fn() -> Result<()>,
            ) -> Result<()> {
                self.gate.cancel(); // Mock the exit received during the native focus delay.
                validate()?;
                self.type_text(text)
            }
        }
        let gate = FinalizationGate::new();
        let typer = SettlingTyper {
            gate: &gate,
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut saved = None;
        assert!(
            confirm_delivery(
                &typer,
                &Target(Cell::new(true)),
                "text",
                None,
                &mut saved,
                &gate,
                |_| {}
            )
            .is_err()
        );
        assert_eq!(typer.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn worker_requires_confirmation_and_cancel_does_not_commit() {
        // The actual command loop must wait after publishing a preview, even when
        // recognition is ready; cancelling that preview must leave history untouched.
        use std::sync::{Arc, Mutex};
        struct CountingTyper(Mutex<Vec<String>>);
        impl TextTyper for CountingTyper {
            fn type_text(&self, text: &str) -> Result<()> {
                self.0.lock().unwrap().push(text.to_owned());
                Ok(())
            }
        }
        // A corrected bubble must deliver and save the edit, never the original STT text.
        for command in [
            PreviewCommand::Cancel,
            PreviewCommand::Confirm("修改后的文字 🎙️".into()),
            PreviewCommand::Confirm(" \n".into()),
        ] {
            let directory = tempdir().unwrap();
            let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
            let worker_history = history.clone();
            let typer = Arc::new(CountingTyper(Mutex::new(Vec::new())));
            let worker_typer = Arc::clone(&typer);
            let (notify, events) = mpsc::channel();
            let (confirmation, feedback) = Confirmation::channel();
            let worker = std::thread::spawn(move || {
                wait_for_confirmation(
                    SessionId(8),
                    PreviewTranscription {
                        text: "candidate".into(),
                        warning: None,
                        confirmation: Some(confirmation),
                    },
                    worker_typer.as_ref(),
                    Some(&worker_history),
                    &FinalizationGate::new(),
                    || Ok(Target(Cell::new(true))),
                    &|event| notify.send(event).is_ok(),
                );
            });
            let PreviewUpdate::Ready { commands, .. } = events
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap()
            else {
                panic!("expected preview");
            };
            assert!(typer.0.lock().unwrap().is_empty());
            assert!(history.load_recent().unwrap().is_empty());
            let expected = match &command {
                PreviewCommand::Confirm(text) if !text.trim().is_empty() => vec![text.clone()],
                _ => Vec::new(),
            };
            commands.send(command).unwrap();
            let learned = feedback.blocking_recv().ok().map(|feedback| {
                assert_eq!(
                    typer.0.lock().unwrap().as_slice(),
                    std::slice::from_ref(&feedback.text)
                );
                feedback.acknowledged.send(Ok(())).unwrap();
                feedback.text
            });
            worker.join().unwrap();
            assert_eq!(*typer.0.lock().unwrap(), expected);
            assert_eq!(history.load_recent().unwrap(), expected);
            assert_eq!(learned.into_iter().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn permission_revocation_in_target_operations_requests_guidance() {
        use crate::platform::NativeTarget;
        struct RevokedTarget {
            during_restore: bool,
        }
        impl DeliveryTarget for RevokedTarget {
            fn name(&self) -> String {
                "Test editor".into()
            }
            fn restore(&self) -> Result<()> {
                if self.during_restore {
                    return Err(NativeTarget::permission_denied_error_for_test()
                        .context("target restoration"));
                }
                Ok(())
            }
            fn validate(&self) -> Result<()> {
                Err(NativeTarget::permission_denied_error_for_test().context("target validation"))
            }
        }
        struct MustNotType;
        impl TextTyper for MustNotType {
            fn type_text(&self, _: &str) -> Result<()> {
                panic!("permission denial must prevent injection");
            }
        }
        // AX permission may disappear before preview capture or while the user edits
        // the bubble. Exercise the production worker with the native typed denial.
        for (during_capture, during_restore) in [(true, false), (false, true), (false, false)] {
            let directory = tempdir().unwrap();
            let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
            let guidance = Cell::new(0);
            let failures = Cell::new(0);
            wait_for_confirmation(
                SessionId(9),
                ("candidate".into(), None).into(),
                &MustNotType,
                Some(&history),
                &FinalizationGate::new(),
                || {
                    if during_capture {
                        Err(NativeTarget::permission_denied_error_for_test())
                    } else {
                        Ok(RevokedTarget { during_restore })
                    }
                },
                &|event| {
                    match event {
                        PreviewUpdate::Ready { commands, .. } => {
                            commands
                                .send(PreviewCommand::Confirm("candidate".into()))
                                .unwrap();
                        }
                        PreviewUpdate::PermissionRequired => guidance.set(guidance.get() + 1),
                        PreviewUpdate::DeliveryFailed { .. } => {
                            failures.set(failures.get() + 1);
                            return false;
                        }
                        _ => panic!("unexpected event"),
                    }
                    true
                },
            );
            assert_eq!(guidance.get(), 1);
            assert_eq!(failures.get(), 1);
            assert!(history.load_recent().unwrap().is_empty());
        }
    }

    #[test]
    fn focus_changed_during_platform_delay_prevents_injection() {
        // Switching applications during the writer's focus delay must be caught
        // by its final validation, even if the destination was valid when confirmed.
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };
        struct FocusTarget(Arc<AtomicBool>);
        impl DeliveryTarget for FocusTarget {
            fn name(&self) -> String {
                "editor".into()
            }
            fn restore(&self) -> Result<()> {
                Ok(())
            }
            fn validate(&self) -> Result<()> {
                anyhow::ensure!(self.0.load(Ordering::Relaxed), "focus changed");
                Ok(())
            }
        }
        struct SwitchingTyper {
            focused: Arc<AtomicBool>,
            calls: AtomicUsize,
        }
        impl TextTyper for SwitchingTyper {
            fn type_text(&self, _: &str) -> Result<()> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            fn type_text_checked(
                &self,
                text: &str,
                validate: &dyn Fn() -> Result<()>,
            ) -> Result<()> {
                self.focused.store(false, Ordering::Relaxed);
                validate()?;
                self.type_text(text)
            }
        }
        let focused = Arc::new(AtomicBool::new(true));
        let target = FocusTarget(Arc::clone(&focused));
        let typer = SwitchingTyper {
            focused,
            calls: AtomicUsize::new(0),
        };
        assert!(
            confirm_delivery(
                &typer,
                &target,
                "text",
                None,
                &mut None,
                &FinalizationGate::new(),
                |_| {}
            )
            .is_err()
        );
        assert_eq!(typer.calls.load(Ordering::Relaxed), 0);
    }
}
