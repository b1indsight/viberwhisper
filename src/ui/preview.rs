//! Shared preview protocol, confirmation state and worker-owned text delivery.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use anyhow::Result;
use tracing::{debug, error};

pub(super) use crate::core::recognition::FinalizedTranscription as PreviewTranscription;
use crate::history::HistoryStore;
use crate::input::typer::TextTyper;
use crate::platform::{DeliveryTarget, PreviewRect};
use crate::session::SessionId;

#[derive(Debug, Clone)]
pub(super) enum PreviewUpdate {
    Destination {
        session_id: SessionId,
        destination: String,
        anchor: Option<PreviewRect>,
        warning: Option<String>,
        stopped: bool,
    },
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

#[derive(Debug)]
pub(super) enum PreviewCommand {
    Stop,
    SelectAt((f64, f64)),
    Transcribed(Option<PreviewTranscription>),
    Confirm(String),
    Cancel,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum CaptureStage {
    Start,
    Stop,
    Click((f64, f64)),
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

/// Runs the same three capture stages for HTTP and Realtime, independently of recognition.
/// Only the owning worker sees native targets; the UI receives destination metadata.
pub(super) fn run_preview_session<T: DeliveryTarget>(
    session_id: SessionId,
    typer: &dyn TextTyper,
    history: Option<&HistoryStore>,
    gate: &FinalizationGate,
    (commands, receiver): (mpsc::Sender<PreviewCommand>, mpsc::Receiver<PreviewCommand>),
    mut capture: impl FnMut(CaptureStage) -> Result<T>,
    notify: &impl Fn(PreviewUpdate) -> bool,
) {
    let mut session = PreviewSession::new(
        session_id,
        typer,
        history,
        gate,
        capture(CaptureStage::Start),
    );
    if gate.is_cancelled() || !session.publish_destination(false, notify) {
        return;
    }
    let mut stopped = false;
    while let Ok(command) = receiver.recv() {
        if gate.is_cancelled() {
            return;
        }
        match command {
            PreviewCommand::Stop if !stopped => {
                if session.can_select() {
                    let candidate = capture(CaptureStage::Stop);
                    if gate.is_cancelled() {
                        return;
                    }
                    session.consider(candidate);
                }
                stopped = true;
                // This acknowledgement must precede preview activation, even if neither
                // capture found geometry. Centering does not close the click stage.
                if !session.publish_destination(true, notify) {
                    return;
                }
            }
            PreviewCommand::SelectAt(point) if stopped && session.can_select() => {
                let candidate = capture(CaptureStage::Click(point));
                if gate.is_cancelled() {
                    return;
                }
                // An invalid click must not replace even a geometry-less legal target.
                if candidate
                    .as_ref()
                    .is_ok_and(|target| target.preview_anchor().is_some())
                {
                    session.consider(candidate);
                    if !session.publish_destination(true, notify) {
                        return;
                    }
                }
            }
            PreviewCommand::Transcribed(Some(transcription))
                if stopped && session.transcription.is_none() =>
            {
                if !session.ready(transcription, commands.clone(), notify) {
                    return;
                }
            }
            PreviewCommand::Transcribed(None) => return,
            command => {
                if session.handle_confirmation(command, notify).is_break() {
                    return;
                }
            }
        }
    }
}

/// The static native demo uses the same confirmation transaction without recording stages.
/// Always acquires its saved target, even if cancellation won the Ready handoff.
#[cfg(any(test, feature = "integration"))]
pub(super) fn wait_for_confirmation<T: DeliveryTarget>(
    session_id: SessionId,
    transcription: PreviewTranscription,
    typer: &dyn TextTyper,
    history: Option<&HistoryStore>,
    gate: &FinalizationGate,
    capture: impl FnOnce() -> Result<T>,
    notify: &impl Fn(PreviewUpdate) -> bool,
) {
    let mut session = PreviewSession::new(session_id, typer, history, gate, capture());
    let (commands, receiver) = mpsc::channel();
    if session.ready(transcription, commands, notify) {
        while let Ok(command) = receiver.recv() {
            if session.handle_confirmation(command, notify).is_break() {
                break;
            }
        }
    }
}

/// Owns the chosen native destination, confirmation capability and retry transaction.
/// Unsuccessful exits restore the saved destination before publishing session completion.
struct PreviewSession<'a, T: DeliveryTarget> {
    session_id: SessionId,
    typer: &'a dyn TextTyper,
    history: Option<&'a HistoryStore>,
    gate: &'a FinalizationGate,
    target: Result<T>,
    transcription: Option<PreviewTranscription>,
    saved: Option<String>,
    selection_open: bool,
    retry_allowed: bool,
    delivered: bool,
}

impl<'a, T: DeliveryTarget> PreviewSession<'a, T> {
    fn new(
        session_id: SessionId,
        typer: &'a dyn TextTyper,
        history: Option<&'a HistoryStore>,
        gate: &'a FinalizationGate,
        target: Result<T>,
    ) -> Self {
        Self {
            session_id,
            typer,
            history,
            gate,
            target,
            transcription: None,
            saved: None,
            selection_open: true,
            retry_allowed: true,
            delivered: false,
        }
    }

    fn anchor(&self) -> Option<PreviewRect> {
        self.target
            .as_ref()
            .ok()
            .and_then(DeliveryTarget::preview_anchor)
    }

    fn destination(&self) -> String {
        self.target
            .as_ref()
            .map_or_else(|_| "无法确定输入目标".into(), DeliveryTarget::name)
    }

    fn can_confirm(&self) -> bool {
        self.transcription
            .as_ref()
            .is_some_and(|text| !text.text.trim().is_empty())
    }

    fn can_select(&self) -> bool {
        self.selection_open
            && self.anchor().is_none()
            && (self.transcription.is_none() || self.can_confirm())
    }

    fn consider(&mut self, candidate: Result<T>) {
        if self.target.is_err()
            || candidate
                .as_ref()
                .is_ok_and(|target| target.preview_anchor().is_some())
        {
            self.target = candidate;
        }
    }

    fn warning(&self) -> Option<String> {
        let warning = self
            .transcription
            .as_ref()
            .and_then(|text| text.warning.clone());
        if self.can_confirm() && self.target.is_err() {
            let unavailable = "无法获取输入目标；可复制文字后手动粘贴。";
            Some(warning.map_or_else(
                || unavailable.into(),
                |warning| format!("{warning}\n{unavailable}"),
            ))
        } else {
            warning
        }
    }

    fn publish_destination(&self, stopped: bool, notify: &impl Fn(PreviewUpdate) -> bool) -> bool {
        notify(PreviewUpdate::Destination {
            session_id: self.session_id,
            destination: self.destination(),
            anchor: self.anchor(),
            warning: self.warning(),
            stopped,
        })
    }

    fn ready(
        &mut self,
        transcription: PreviewTranscription,
        commands: mpsc::Sender<PreviewCommand>,
        notify: &impl Fn(PreviewUpdate) -> bool,
    ) -> bool {
        self.transcription = Some(transcription);
        !self.gate.is_cancelled()
            && notify(PreviewUpdate::Ready {
                session_id: self.session_id,
                text: self
                    .transcription
                    .as_ref()
                    .expect("just prepared")
                    .text
                    .clone(),
                destination: self.destination(),
                anchor: self.anchor(),
                warning: self.warning(),
                commands,
            })
    }

    fn handle_confirmation(
        &mut self,
        command: PreviewCommand,
        notify: &impl Fn(PreviewUpdate) -> bool,
    ) -> std::ops::ControlFlow<()> {
        use std::ops::ControlFlow::{Break, Continue};
        if self.gate.is_cancelled() {
            return Break(());
        }
        match command {
            PreviewCommand::Cancel => Break(()),
            // An error-only preview cannot acquire delivery capability from a
            // queued native callback carrying arbitrary nonempty text.
            PreviewCommand::Confirm(_) if !self.can_confirm() || !self.retry_allowed => {
                Continue(())
            }
            PreviewCommand::Confirm(text) => {
                self.selection_open = false;
                if text.trim().is_empty() {
                    return Break(());
                }
                let result = match &self.target {
                    Ok(target) => confirm_delivery(
                        self.typer,
                        target,
                        &text,
                        self.history,
                        &mut self.saved,
                        self.gate,
                        |text| {
                            notify(PreviewUpdate::HistorySaved(text));
                        },
                    ),
                    Err(error) => Err(anyhow::anyhow!("无法恢复输入目标：{error}")),
                };
                match result {
                    Ok(delivered) => {
                        self.delivered = delivered;
                        if delivered
                            && let Some(confirmation) = self
                                .transcription
                                .as_mut()
                                .and_then(|text| text.confirmation.take())
                            && let Err(error) = confirmation.confirm(text)
                        {
                            error!(%error, "Confirmed text was delivered but dictation learning failed");
                        }
                        Break(())
                    }
                    Err(error) => {
                        if self.gate.is_cancelled() {
                            return Break(());
                        }
                        error!(%error, "Confirmed text delivery failed");
                        if self
                            .target
                            .as_ref()
                            .err()
                            .is_some_and(crate::platform::is_text_delivery_permission_error)
                            || crate::platform::is_text_delivery_permission_error(&error)
                        {
                            notify(PreviewUpdate::PermissionRequired);
                        }
                        self.retry_allowed =
                            !error.is::<crate::input::typer::PartialTextDelivery>();
                        if !notify(PreviewUpdate::DeliveryFailed {
                            session_id: self.session_id,
                            error: error.to_string(),
                            retry_allowed: self.retry_allowed,
                        }) {
                            return Break(());
                        }
                        Continue(())
                    }
                }
            }
            _ => Continue(()),
        }
    }
}

impl<T: DeliveryTarget> Drop for PreviewSession<'_, T> {
    fn drop(&mut self) {
        if !self.delivered
            && let Ok(target) = &self.target
            && let Err(error) = target.restore()
        {
            debug!(%error, "Could not restore abandoned preview destination");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use anyhow::{Result, bail};
    use tempfile::tempdir;

    use super::*;
    use crate::dictation::Confirmation;
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
    fn recording_stages_keep_the_first_position_and_deliver_to_its_target() {
        use std::rc::Rc;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        struct Positioned {
            name: &'static str,
            anchor: Option<PreviewRect>,
            focus: Arc<Mutex<Option<&'static str>>>,
            _owner: Rc<()>,
        }
        impl DeliveryTarget for Positioned {
            fn name(&self) -> String {
                self.name.into()
            }
            fn preview_anchor(&self) -> Option<PreviewRect> {
                self.anchor
            }
            fn restore(&self) -> Result<()> {
                *self.focus.lock().unwrap() = Some(self.name);
                Ok(())
            }
            fn validate(&self) -> Result<()> {
                anyhow::ensure!(*self.focus.lock().unwrap() == Some(self.name));
                Ok(())
            }
        }
        struct RecordingTyper {
            focus: Arc<Mutex<Option<&'static str>>>,
            delivered: Arc<Mutex<Vec<(&'static str, String)>>>,
        }
        impl TextTyper for RecordingTyper {
            fn type_text(&self, text: &str) -> Result<()> {
                self.delivered
                    .lock()
                    .unwrap()
                    .push((self.focus.lock().unwrap().unwrap(), text.into()));
                Ok(())
            }
        }

        let a = PreviewRect::new(125.0, 241.0, 1.0, 19.0).unwrap();
        let b = PreviewRect::new(-913.0, 57.0, 2.0, 23.0).unwrap();
        let c = PreviewRect::new(713.0, -216.0, 0.0, 17.0).unwrap();
        // A tray session can have a legal window but no caret at either boundary.
        // Clicking an input must still work before or after recognition is ready;
        // once A/B/C wins, a later click on D must not redirect the edited result.
        for (start, stop, click_before_ready, expected_name, expected_anchor) in [
            (Some(Some(a)), Some(Some(b)), true, "A", a),
            (Some(Some(a)), Some(Some(b)), false, "A", a),
            (None, Some(Some(b)), false, "B", b),
            (Some(None), Some(Some(b)), true, "B", b),
            (Some(None), Some(None), true, "C", c),
            (Some(None), Some(None), false, "C", c),
            (None, None, true, "C", c),
            (None, None, false, "C", c),
        ] {
            let directory = tempdir().unwrap();
            let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
            let worker_history = history.clone();
            let delivered = Arc::new(Mutex::new(Vec::new()));
            let output = Arc::clone(&delivered);
            let (commands, receiver) = mpsc::channel();
            let worker_commands = commands.clone();
            let (notify, events) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let focus = Arc::new(Mutex::new(None));
                let typer = RecordingTyper {
                    focus: Arc::clone(&focus),
                    delivered: output,
                };
                run_preview_session(
                    SessionId(27),
                    &typer,
                    Some(&worker_history),
                    &FinalizationGate::new(),
                    (worker_commands, receiver),
                    |stage| {
                        let (name, anchor) = match stage {
                            CaptureStage::Start => ("A", start),
                            CaptureStage::Stop => ("B", stop),
                            CaptureStage::Click((1.0, 2.0)) => ("C", Some(Some(c))),
                            CaptureStage::Click((3.0, 4.0)) => ("D", Some(Some(b))),
                            CaptureStage::Click(_) => bail!("not an input click"),
                        };
                        Ok(Positioned {
                            name,
                            anchor: anchor.ok_or_else(|| anyhow::anyhow!("no target"))?,
                            focus: Arc::clone(&focus),
                            _owner: Rc::new(()),
                        })
                    },
                    &|event| notify.send(event).is_ok(),
                );
            });
            assert!(matches!(
                events.recv_timeout(Duration::from_secs(1)).unwrap(),
                PreviewUpdate::Destination { stopped: false, .. }
            ));
            commands.send(PreviewCommand::SelectAt((1.0, 2.0))).unwrap();
            commands.send(PreviewCommand::Stop).unwrap();
            let PreviewUpdate::Destination {
                stopped, anchor, ..
            } = events.recv_timeout(Duration::from_secs(1)).unwrap()
            else {
                panic!("stop capture must finish before preview activation");
            };
            assert!(stopped);
            assert_eq!(anchor, start.flatten().or(stop.flatten()));

            let clicks = || {
                commands
                    .send(PreviewCommand::SelectAt((-1.0, -1.0)))
                    .unwrap();
                commands.send(PreviewCommand::SelectAt((1.0, 2.0))).unwrap();
                commands.send(PreviewCommand::SelectAt((3.0, 4.0))).unwrap();
            };
            if click_before_ready {
                clicks();
            }
            commands
                .send(PreviewCommand::Transcribed(Some(
                    (
                        "recognition draft".into(),
                        Some("recognition warning".into()),
                    )
                        .into(),
                )))
                .unwrap();
            loop {
                if let PreviewUpdate::Ready {
                    destination,
                    anchor,
                    warning,
                    text,
                    ..
                } = events.recv_timeout(Duration::from_secs(1)).unwrap()
                {
                    assert_eq!(text, "recognition draft");
                    assert!(warning.unwrap().contains("recognition warning"));
                    if click_before_ready || expected_name != "C" {
                        assert_eq!(destination, expected_name);
                        assert_eq!(anchor, Some(expected_anchor));
                    } else {
                        assert_eq!(anchor, None, "Ready must not be a fourth capture");
                    }
                    break;
                }
            }
            if !click_before_ready {
                clicks();
            }
            commands
                .send(PreviewCommand::Confirm("edited final 🎙️".into()))
                .unwrap();
            worker.join().unwrap();
            if !click_before_ready && expected_name == "C" {
                let update = events.try_iter().find_map(|event| match event {
                    PreviewUpdate::Destination {
                        destination,
                        anchor,
                        warning,
                        ..
                    } => Some((destination, anchor, warning)),
                    _ => None,
                });
                assert_eq!(
                    update,
                    Some(("C".into(), Some(c), Some("recognition warning".into())))
                );
            }
            assert_eq!(
                *delivered.lock().unwrap(),
                [(expected_name, "edited final 🎙️".into())]
            );
            assert_eq!(history.load_recent().unwrap(), ["edited final 🎙️"]);
        }
    }

    #[test]
    fn late_selection_cannot_revive_empty_cancelled_or_confirmed_previews() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Clone, Copy, Debug)]
        enum Case {
            Empty,
            Cancelled,
            CancelledDuringClick,
            ClosedTarget,
            ConfirmationFailed,
            PartialDelivery,
        }
        struct Destination<'a> {
            name: &'static str,
            anchor: Option<PreviewRect>,
            closed: bool,
            focused: &'a Cell<&'static str>,
        }
        impl DeliveryTarget for Destination<'_> {
            fn name(&self) -> String {
                self.name.into()
            }
            fn preview_anchor(&self) -> Option<PreviewRect> {
                self.anchor
            }
            fn restore(&self) -> Result<()> {
                self.focused.set(self.name);
                anyhow::ensure!(!self.closed, "original target closed");
                Ok(())
            }
            fn validate(&self) -> Result<()> {
                anyhow::ensure!(!self.closed);
                Ok(())
            }
        }
        struct PartialTyper(AtomicUsize);
        impl TextTyper for PartialTyper {
            fn type_text(&self, _: &str) -> Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Err(crate::input::typer::PartialTextDelivery.into())
            }
        }
        // A later valid input click cannot turn an error/cancelled preview into a
        // deliverable one, redirect a failed confirmation, or repeat a partial paste.
        for case in [
            Case::Empty,
            Case::Cancelled,
            Case::CancelledDuringClick,
            Case::ClosedTarget,
            Case::ConfirmationFailed,
            Case::PartialDelivery,
        ] {
            let gate = FinalizationGate::new();
            let focused = Cell::new("none");
            let typer = PartialTyper(AtomicUsize::new(0));
            let (commands, receiver) = mpsc::channel();
            commands.send(PreviewCommand::Stop).unwrap();
            commands
                .send(PreviewCommand::Transcribed(Some(
                    (
                        if matches!(case, Case::Empty) {
                            ""
                        } else {
                            "draft"
                        }
                        .into(),
                        None,
                    )
                        .into(),
                )))
                .unwrap();
            if matches!(
                case,
                Case::ClosedTarget | Case::ConfirmationFailed | Case::PartialDelivery
            ) {
                commands
                    .send(PreviewCommand::Confirm("first confirmation".into()))
                    .unwrap();
            }
            commands.send(PreviewCommand::SelectAt((1.0, 2.0))).unwrap();
            commands
                .send(PreviewCommand::Confirm("stale confirmation".into()))
                .unwrap();
            commands.send(PreviewCommand::Cancel).unwrap();
            run_preview_session(
                SessionId(31),
                &typer,
                None,
                &gate,
                (commands, receiver),
                |stage| {
                    let clicked = matches!(stage, CaptureStage::Click(_));
                    if clicked && matches!(case, Case::CancelledDuringClick) {
                        gate.cancel();
                    }
                    Ok(Destination {
                        name: if clicked {
                            "late input"
                        } else {
                            "original input"
                        },
                        anchor: (clicked || matches!(case, Case::ClosedTarget))
                            .then(|| PreviewRect::new(113.0, 217.0, 1.0, 19.0).unwrap()),
                        closed: matches!(case, Case::ClosedTarget | Case::ConfirmationFailed),
                        focused: &focused,
                    })
                },
                &|event| {
                    if matches!(case, Case::Cancelled)
                        && matches!(event, PreviewUpdate::Ready { .. })
                    {
                        gate.cancel();
                    }
                    true
                },
            );
            assert_eq!(focused.get(), "original input", "{case:?}");
            assert_eq!(
                typer.0.load(Ordering::Relaxed),
                usize::from(matches!(case, Case::PartialDelivery)),
                "{case:?}"
            );
        }
    }

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
        assert_eq!(feedback, "最终修改 🎙️\nViberWhisper");
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
        // An error-only bubble has no deliverable text, even if a stale or forged
        // callback supplies a nonempty edit. Only cancellation may finish it.
        for (candidate, command) in [
            ("candidate", PreviewCommand::Cancel),
            (
                "candidate",
                PreviewCommand::Confirm("修改后的文字 🎙️".into()),
            ),
            ("candidate", PreviewCommand::Confirm(" \n".into())),
            ("", PreviewCommand::Cancel),
            ("", PreviewCommand::Confirm("不能投递的提示".into())),
            (" \n", PreviewCommand::Confirm("不能投递的提示".into())),
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
                        text: candidate.into(),
                        warning: candidate.trim().is_empty().then(|| "按 Esc 关闭。".into()),
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
                PreviewCommand::Confirm(text)
                    if !candidate.trim().is_empty() && !text.trim().is_empty() =>
                {
                    vec![text.clone()]
                }
                _ => Vec::new(),
            };
            commands.send(command).unwrap();
            if candidate.trim().is_empty() {
                let _ = commands.send(PreviewCommand::Cancel);
            }
            let learned = feedback.blocking_recv().ok().inspect(|feedback| {
                assert_eq!(
                    typer.0.lock().unwrap().as_slice(),
                    std::slice::from_ref(feedback)
                );
            });
            worker.join().unwrap();
            assert_eq!(*typer.0.lock().unwrap(), expected);
            assert_eq!(history.load_recent().unwrap(), expected);
            assert_eq!(learned.into_iter().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn unrelated_errors_and_canceled_confirmation_do_not_request_permission_guidance() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct FailingTyper(AtomicUsize);
        impl TextTyper for FailingTyper {
            fn type_text(&self, _: &str) -> Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                bail!("Accessibility permission is required")
            }
        }
        // An unrelated failure can mention Accessibility. Only a typed denial warrants
        // guidance; a confirmation queued just before cancellation must not type at all.
        for (capture_fails, cancelled) in [(false, false), (true, false), (false, true)] {
            let typer = FailingTyper(AtomicUsize::new(0));
            let gate = FinalizationGate::new();
            let guidance = Cell::new(0);
            let failures = Cell::new(0);
            wait_for_confirmation(
                SessionId(9),
                ("candidate".into(), None).into(),
                &typer,
                None,
                &gate,
                || {
                    if capture_fails {
                        bail!("Accessibility permission is required");
                    }
                    Ok(Target(Cell::new(true)))
                },
                &|event| {
                    match event {
                        PreviewUpdate::Ready { commands, .. } => {
                            commands
                                .send(PreviewCommand::Confirm("candidate".into()))
                                .unwrap();
                            if cancelled {
                                gate.cancel();
                            }
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
            assert_eq!(guidance.get(), 0);
            assert_eq!(failures.get(), usize::from(!cancelled));
            assert_eq!(
                typer.0.load(Ordering::Relaxed),
                usize::from(!capture_fails && !cancelled)
            );
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
