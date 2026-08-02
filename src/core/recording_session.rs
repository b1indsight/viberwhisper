use crate::audio::WavChunk;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlSource {
    HoldHotkey,
    ToggleHotkey,
    Tray,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    Start(SessionMode),
    Stop,
    Toggle(SessionMode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlEvent {
    pub source: ControlSource,
    pub action: ControlAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPhase {
    Recorder,
    Orchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopPhase {
    Recorder,
    Orchestrator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Starting {
        session_id: SessionId,
        mode: SessionMode,
        source: ControlSource,
        phase: StartPhase,
    },
    Recording {
        session_id: SessionId,
        mode: SessionMode,
        source: ControlSource,
    },
    Stopping {
        session_id: SessionId,
        mode: SessionMode,
        source: ControlSource,
        phase: StopPhase,
    },
    ShuttingDown {
        session_id: Option<SessionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Control(ControlEvent),
    RecorderStarted {
        session_id: SessionId,
    },
    RecorderStartFailed {
        session_id: SessionId,
        error: String,
    },
    RecorderAlreadyRecording {
        requested_session_id: SessionId,
        active_session_id: SessionId,
    },
    OrchestratorStarted {
        session_id: SessionId,
    },
    OrchestratorStartFailed {
        requested_session_id: SessionId,
        active_session_id: Option<SessionId>,
        error: String,
    },
    ChunkReady {
        session_id: SessionId,
        chunk: WavChunk,
    },
    RecorderStopped {
        session_id: SessionId,
        chunks: Vec<WavChunk>,
        warning: Option<String>,
    },
    RecorderStillRecording {
        session_id: SessionId,
        error: String,
    },
    RecorderNotRecording {
        session_id: SessionId,
    },
    OrchestratorFinished {
        session_id: SessionId,
    },
    ShutdownRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    StartRecorder {
        session_id: SessionId,
    },
    StartOrchestrator {
        session_id: SessionId,
        mode: SessionMode,
    },
    StopRecorder {
        session_id: SessionId,
    },
    SubmitChunk {
        session_id: SessionId,
        chunk: WavChunk,
    },
    FinishOrchestrator {
        session_id: SessionId,
    },
    CancelRecorder {
        session_id: SessionId,
    },
    AbortOrchestrator {
        session_id: SessionId,
    },
    SetTrayRecording(bool),
    ReadyToExit,
}

impl SessionEvent {
    fn summary(&self) -> (&'static str, Option<SessionId>) {
        match self {
            Self::Control(ControlEvent { action, .. }) => {
                let name = match action {
                    ControlAction::Start(_) => "control_start",
                    ControlAction::Stop => "control_stop",
                    ControlAction::Toggle(_) => "control_toggle",
                };
                (name, None)
            }
            Self::RecorderStarted { session_id } => ("recorder_started", Some(*session_id)),
            Self::RecorderStartFailed { session_id, .. } => {
                ("recorder_start_failed", Some(*session_id))
            }
            Self::RecorderAlreadyRecording {
                requested_session_id,
                ..
            } => ("recorder_already_recording", Some(*requested_session_id)),
            Self::OrchestratorStarted { session_id } => ("orchestrator_started", Some(*session_id)),
            Self::OrchestratorStartFailed {
                requested_session_id,
                ..
            } => ("orchestrator_start_failed", Some(*requested_session_id)),
            Self::ChunkReady { session_id, .. } => ("chunk_ready", Some(*session_id)),
            Self::RecorderStopped { session_id, .. } => ("recorder_stopped", Some(*session_id)),
            Self::RecorderStillRecording { session_id, .. } => {
                ("recorder_still_recording", Some(*session_id))
            }
            Self::RecorderNotRecording { session_id } => {
                ("recorder_not_recording", Some(*session_id))
            }
            Self::OrchestratorFinished { session_id } => {
                ("orchestrator_finished", Some(*session_id))
            }
            Self::ShutdownRequested => ("shutdown_requested", None),
        }
    }
}

struct Transition {
    next: RecordingState,
    effects: Vec<SessionEffect>,
}

pub struct RecordingSessionMachine {
    state: RecordingState,
    next_session_id: u64,
}

impl Default for RecordingSessionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingSessionMachine {
    pub fn new() -> Self {
        Self {
            state: RecordingState::Idle,
            next_session_id: 1,
        }
    }

    pub fn state(&self) -> &RecordingState {
        &self.state
    }

    /// Apply one external or lower-layer event to the recording lifecycle.
    ///
    /// This is the machine's only state-writing entry point. Events outside the
    /// explicit transition table are logged and leave the current state unchanged.
    pub fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        let (event_name, routing_session_id) = event.summary();
        let session_mismatch = matches!(
            (active_session_id(self.state), routing_session_id),
            (Some(active), Some(routed)) if active != routed
        );
        let transition = if session_mismatch {
            None
        } else {
            transition(self.state, event, &mut self.next_session_id)
        };
        let Some(transition) = transition else {
            debug!(
                state = ?self.state,
                event = event_name,
                session_id = ?routing_session_id.map(|id| id.0),
                "Recording session event rejected"
            );
            return Vec::new();
        };

        self.state = transition.next;
        transition.effects
    }
}

/// Enumerate every event that may change or act on the recording lifecycle.
/// The caller validates session routing first; events not represented by a
/// state/phase match arm are then rejected without mutating the current state.
fn transition(
    state: RecordingState,
    event: SessionEvent,
    next_session_id: &mut u64,
) -> Option<Transition> {
    match (state, event) {
        (RecordingState::ShuttingDown { .. }, _) => None,
        (state, SessionEvent::ShutdownRequested) => Some(shutdown_transition(state)),
        (
            RecordingState::Idle,
            SessionEvent::Control(ControlEvent {
                source,
                action: ControlAction::Start(mode) | ControlAction::Toggle(mode),
            }),
        ) => {
            let session_id = SessionId(*next_session_id);
            *next_session_id += 1;
            Some(Transition {
                next: RecordingState::Starting {
                    session_id,
                    mode,
                    source,
                    phase: StartPhase::Recorder,
                },
                effects: vec![SessionEffect::StartRecorder { session_id }],
            })
        }
        (
            RecordingState::Starting {
                session_id,
                mode,
                source,
                phase: StartPhase::Recorder,
            },
            SessionEvent::RecorderStarted { .. },
        ) => Some(Transition {
            next: RecordingState::Starting {
                session_id,
                mode,
                source,
                phase: StartPhase::Orchestrator,
            },
            effects: vec![SessionEffect::StartOrchestrator { session_id, mode }],
        }),
        (
            RecordingState::Starting {
                phase: StartPhase::Recorder,
                ..
            },
            SessionEvent::RecorderStartFailed { .. },
        ) => Some(Transition {
            next: RecordingState::Idle,
            effects: vec![SessionEffect::SetTrayRecording(false)],
        }),
        (
            RecordingState::Starting {
                phase: StartPhase::Recorder,
                ..
            },
            SessionEvent::RecorderAlreadyRecording {
                active_session_id, ..
            },
        ) => Some(Transition {
            next: RecordingState::Idle,
            effects: vec![
                SessionEffect::CancelRecorder {
                    session_id: active_session_id,
                },
                SessionEffect::AbortOrchestrator {
                    session_id: active_session_id,
                },
                SessionEffect::SetTrayRecording(false),
            ],
        }),
        (
            RecordingState::Starting {
                session_id,
                mode,
                source,
                phase: StartPhase::Orchestrator,
            },
            SessionEvent::OrchestratorStarted { .. },
        ) => Some(Transition {
            next: RecordingState::Recording {
                session_id,
                mode,
                source,
            },
            effects: vec![SessionEffect::SetTrayRecording(true)],
        }),
        (
            RecordingState::Starting {
                session_id,
                phase: StartPhase::Orchestrator,
                ..
            },
            SessionEvent::OrchestratorStartFailed {
                active_session_id, ..
            },
        ) => Some(Transition {
            next: RecordingState::Idle,
            effects: vec![
                SessionEffect::CancelRecorder { session_id },
                SessionEffect::AbortOrchestrator {
                    session_id: active_session_id.unwrap_or(session_id),
                },
                SessionEffect::SetTrayRecording(false),
            ],
        }),
        (
            state @ RecordingState::Recording { session_id, .. },
            SessionEvent::ChunkReady { chunk, .. },
        ) => Some(Transition {
            next: state,
            effects: vec![SessionEffect::SubmitChunk { session_id, chunk }],
        }),
        (
            RecordingState::Recording {
                session_id,
                mode,
                source,
            },
            SessionEvent::Control(ControlEvent {
                action: ControlAction::Toggle(_),
                ..
            }),
        ) => Some(stop_transition(session_id, mode, source)),
        (
            RecordingState::Recording {
                session_id,
                mode: SessionMode::Hold,
                source,
            },
            SessionEvent::Control(ControlEvent {
                source: ControlSource::HoldHotkey,
                action: ControlAction::Stop,
            }),
        ) => Some(stop_transition(session_id, SessionMode::Hold, source)),
        (
            RecordingState::Stopping {
                session_id,
                mode,
                source,
                phase: StopPhase::Recorder,
            },
            SessionEvent::RecorderStopped { chunks, .. },
        ) => Some(recorder_stopped_transition(
            session_id, mode, source, chunks,
        )),
        (
            RecordingState::Stopping {
                session_id,
                mode,
                source,
                phase: StopPhase::Recorder,
            },
            SessionEvent::RecorderNotRecording { .. },
        ) => Some(recorder_stopped_transition(
            session_id,
            mode,
            source,
            Vec::new(),
        )),
        (
            RecordingState::Stopping {
                session_id,
                mode,
                source,
                phase: StopPhase::Recorder,
            },
            SessionEvent::RecorderStillRecording { .. },
        ) => Some(Transition {
            next: RecordingState::Recording {
                session_id,
                mode,
                source,
            },
            effects: vec![SessionEffect::SetTrayRecording(true)],
        }),
        (
            RecordingState::Stopping {
                phase: StopPhase::Orchestrator,
                ..
            },
            SessionEvent::OrchestratorFinished { .. },
        ) => Some(Transition {
            next: RecordingState::Idle,
            effects: Vec::new(),
        }),
        _ => None,
    }
}

fn stop_transition(session_id: SessionId, mode: SessionMode, source: ControlSource) -> Transition {
    Transition {
        next: RecordingState::Stopping {
            session_id,
            mode,
            source,
            phase: StopPhase::Recorder,
        },
        effects: vec![SessionEffect::StopRecorder { session_id }],
    }
}

fn recorder_stopped_transition(
    session_id: SessionId,
    mode: SessionMode,
    source: ControlSource,
    chunks: Vec<WavChunk>,
) -> Transition {
    let mut effects = Vec::with_capacity(chunks.len() + 2);
    effects.push(SessionEffect::SetTrayRecording(false));
    effects.extend(
        chunks
            .into_iter()
            .map(|chunk| SessionEffect::SubmitChunk { session_id, chunk }),
    );
    effects.push(SessionEffect::FinishOrchestrator { session_id });

    Transition {
        next: RecordingState::Stopping {
            session_id,
            mode,
            source,
            phase: StopPhase::Orchestrator,
        },
        effects,
    }
}

fn shutdown_transition(state: RecordingState) -> Transition {
    let session_id = active_session_id(state);
    let mut effects = Vec::new();
    if let Some(session_id) = session_id {
        effects.push(SessionEffect::CancelRecorder { session_id });
        effects.push(SessionEffect::AbortOrchestrator { session_id });
    }
    effects.push(SessionEffect::SetTrayRecording(false));
    effects.push(SessionEffect::ReadyToExit);

    Transition {
        next: RecordingState::ShuttingDown { session_id },
        effects,
    }
}

fn active_session_id(state: RecordingState) -> Option<SessionId> {
    match state {
        RecordingState::Starting { session_id, .. }
        | RecordingState::Recording { session_id, .. }
        | RecordingState::Stopping { session_id, .. } => Some(session_id),
        RecordingState::ShuttingDown { session_id } => session_id,
        RecordingState::Idle => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_chunk() -> WavChunk {
        WavChunk::from_encoded_bytes(b"test wav chunk".to_vec())
    }

    fn toggle(source: ControlSource) -> SessionEvent {
        SessionEvent::Control(ControlEvent {
            source,
            action: ControlAction::Toggle(SessionMode::Toggle),
        })
    }

    #[test]
    fn explicit_transition_table_rejects_unlisted_paths() {
        let mut next_session_id = 2;

        let result = transition(
            RecordingState::Starting {
                session_id: SessionId(1),
                mode: SessionMode::Toggle,
                source: ControlSource::Tray,
                phase: StartPhase::Orchestrator,
            },
            SessionEvent::RecorderStarted {
                session_id: SessionId(1),
            },
            &mut next_session_id,
        );

        assert!(result.is_none());
        assert_eq!(next_session_id, 2);
    }

    #[test]
    fn explicit_transition_table_accepts_applied_zero_effect_path() {
        // Finishing convergence changes lifecycle state even though the runtime has
        // no follow-up command to execute, so it must not hit the rejection fallback.
        let mut next_session_id = 2;

        let result = transition(
            RecordingState::Stopping {
                session_id: SessionId(1),
                mode: SessionMode::Toggle,
                source: ControlSource::Tray,
                phase: StopPhase::Orchestrator,
            },
            SessionEvent::OrchestratorFinished {
                session_id: SessionId(1),
            },
            &mut next_session_id,
        )
        .expect("listed transition should be accepted");

        assert_eq!(result.next, RecordingState::Idle);
        assert!(result.effects.is_empty());
        assert_eq!(next_session_id, 2);
    }

    #[test]
    fn orphan_recorder_routes_by_requested_session_id() {
        // The requested ID routes the event to this machine; the different active
        // ID identifies the orphan lower-layer resources that need cleanup.
        let mut machine = RecordingSessionMachine::new();
        machine.handle(toggle(ControlSource::Tray));

        let effects = machine.handle(SessionEvent::RecorderAlreadyRecording {
            requested_session_id: SessionId(1),
            active_session_id: SessionId(7),
        });

        assert_eq!(machine.state(), &RecordingState::Idle);
        assert_eq!(
            effects,
            vec![
                SessionEffect::CancelRecorder {
                    session_id: SessionId(7),
                },
                SessionEffect::AbortOrchestrator {
                    session_id: SessionId(7),
                },
                SessionEffect::SetTrayRecording(false),
            ]
        );
    }

    #[test]
    fn toggle_session_runs_one_start_and_stop_chain() {
        let mut machine = RecordingSessionMachine::new();
        let chunk = test_chunk();

        assert_eq!(
            machine.handle(toggle(ControlSource::Tray)),
            vec![SessionEffect::StartRecorder {
                session_id: SessionId(1)
            }]
        );
        assert_eq!(
            machine.handle(SessionEvent::RecorderStarted {
                session_id: SessionId(1)
            }),
            vec![SessionEffect::StartOrchestrator {
                session_id: SessionId(1),
                mode: SessionMode::Toggle,
            }]
        );
        assert_eq!(
            machine.handle(SessionEvent::OrchestratorStarted {
                session_id: SessionId(1)
            }),
            vec![SessionEffect::SetTrayRecording(true)]
        );
        assert!(matches!(machine.state(), RecordingState::Recording { .. }));

        assert_eq!(
            machine.handle(toggle(ControlSource::ToggleHotkey)),
            vec![SessionEffect::StopRecorder {
                session_id: SessionId(1)
            }]
        );
        assert_eq!(
            machine.handle(SessionEvent::RecorderStopped {
                session_id: SessionId(1),
                chunks: vec![chunk.clone()],
                warning: None,
            }),
            vec![
                SessionEffect::SetTrayRecording(false),
                SessionEffect::SubmitChunk {
                    session_id: SessionId(1),
                    chunk,
                },
                SessionEffect::FinishOrchestrator {
                    session_id: SessionId(1),
                },
            ]
        );
        assert_eq!(
            machine.handle(SessionEvent::OrchestratorFinished {
                session_id: SessionId(1)
            }),
            Vec::<SessionEffect>::new()
        );
        assert_eq!(machine.state(), &RecordingState::Idle);
    }

    #[test]
    fn hold_release_after_tray_stop_is_a_noop() {
        let mut machine = RecordingSessionMachine::new();
        let hold_start = SessionEvent::Control(ControlEvent {
            source: ControlSource::HoldHotkey,
            action: ControlAction::Start(SessionMode::Hold),
        });
        machine.handle(hold_start);
        machine.handle(SessionEvent::RecorderStarted {
            session_id: SessionId(1),
        });
        machine.handle(SessionEvent::OrchestratorStarted {
            session_id: SessionId(1),
        });
        machine.handle(toggle(ControlSource::Tray));
        machine.handle(SessionEvent::RecorderStopped {
            session_id: SessionId(1),
            chunks: vec![],
            warning: None,
        });
        machine.handle(SessionEvent::OrchestratorFinished {
            session_id: SessionId(1),
        });

        let release = SessionEvent::Control(ControlEvent {
            source: ControlSource::HoldHotkey,
            action: ControlAction::Stop,
        });
        assert!(machine.handle(release).is_empty());
        assert_eq!(machine.state(), &RecordingState::Idle);
    }

    #[test]
    fn stale_chunks_and_completions_cannot_mutate_active_session() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(toggle(ControlSource::Tray));
        machine.handle(SessionEvent::RecorderStarted {
            session_id: SessionId(1),
        });
        machine.handle(SessionEvent::OrchestratorStarted {
            session_id: SessionId(1),
        });

        assert_eq!(
            machine.handle(SessionEvent::ChunkReady {
                session_id: SessionId(99),
                chunk: test_chunk(),
            }),
            Vec::<SessionEffect>::new()
        );
        assert!(
            machine
                .handle(SessionEvent::OrchestratorFinished {
                    session_id: SessionId(99)
                })
                .is_empty()
        );
        assert!(matches!(machine.state(), RecordingState::Recording { .. }));
    }

    #[test]
    fn shutdown_cancels_active_session_and_is_idempotent() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(toggle(ControlSource::Tray));
        machine.handle(SessionEvent::RecorderStarted {
            session_id: SessionId(1),
        });
        machine.handle(SessionEvent::OrchestratorStarted {
            session_id: SessionId(1),
        });

        assert_eq!(
            machine.handle(SessionEvent::ShutdownRequested),
            vec![
                SessionEffect::CancelRecorder {
                    session_id: SessionId(1)
                },
                SessionEffect::AbortOrchestrator {
                    session_id: SessionId(1)
                },
                SessionEffect::SetTrayRecording(false),
                SessionEffect::ReadyToExit,
            ]
        );
        assert!(matches!(
            machine.state(),
            RecordingState::ShuttingDown { .. }
        ));
        assert!(machine.handle(SessionEvent::ShutdownRequested).is_empty());
        assert!(machine.handle(toggle(ControlSource::Tray)).is_empty());
    }

    #[test]
    fn start_failure_and_orchestrator_failure_return_to_idle() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(toggle(ControlSource::Tray));
        assert_eq!(
            machine.handle(SessionEvent::RecorderStartFailed {
                session_id: SessionId(1),
                error: "device unavailable".into(),
            }),
            vec![SessionEffect::SetTrayRecording(false)]
        );
        assert_eq!(machine.state(), &RecordingState::Idle);

        machine.handle(toggle(ControlSource::Tray));
        machine.handle(SessionEvent::RecorderStarted {
            session_id: SessionId(2),
        });
        assert_eq!(
            machine.handle(SessionEvent::OrchestratorStartFailed {
                requested_session_id: SessionId(2),
                active_session_id: Some(SessionId(99)),
                error: "active session".into(),
            }),
            vec![
                SessionEffect::CancelRecorder {
                    session_id: SessionId(2)
                },
                SessionEffect::AbortOrchestrator {
                    session_id: SessionId(99)
                },
                SessionEffect::SetTrayRecording(false),
            ]
        );
        assert_eq!(machine.state(), &RecordingState::Idle);
    }

    #[test]
    fn recorder_outcome_controls_stop_recovery() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(toggle(ControlSource::Tray));
        machine.handle(SessionEvent::RecorderStarted {
            session_id: SessionId(1),
        });
        machine.handle(SessionEvent::OrchestratorStarted {
            session_id: SessionId(1),
        });
        machine.handle(toggle(ControlSource::Tray));

        assert_eq!(
            machine.handle(SessionEvent::RecorderStillRecording {
                session_id: SessionId(1),
                error: "backend busy".into(),
            }),
            vec![SessionEffect::SetTrayRecording(true)]
        );
        assert!(matches!(machine.state(), RecordingState::Recording { .. }));

        machine.handle(toggle(ControlSource::Tray));
        assert_eq!(
            machine.handle(SessionEvent::RecorderNotRecording {
                session_id: SessionId(1)
            }),
            vec![
                SessionEffect::SetTrayRecording(false),
                SessionEffect::FinishOrchestrator {
                    session_id: SessionId(1)
                }
            ]
        );
    }

    #[test]
    fn shutdown_from_idle_emits_ready_once() {
        let mut machine = RecordingSessionMachine::new();
        assert_eq!(
            machine.handle(SessionEvent::ShutdownRequested),
            vec![
                SessionEffect::SetTrayRecording(false),
                SessionEffect::ReadyToExit
            ]
        );
        assert!(machine.handle(SessionEvent::ShutdownRequested).is_empty());
    }
}
