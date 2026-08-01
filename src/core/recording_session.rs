use crate::audio::WavChunk;

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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    Recovering {
        session_id: Option<SessionId>,
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

    pub fn handle(&mut self, event: SessionEvent) -> Vec<SessionEffect> {
        if matches!(self.state, RecordingState::ShuttingDown { .. }) {
            return Vec::new();
        }

        if matches!(event, SessionEvent::ShutdownRequested) {
            return self.shutdown();
        }

        match event {
            SessionEvent::Control(control) => self.handle_control(control),
            SessionEvent::RecorderStarted { session_id } => {
                let RecordingState::Starting {
                    session_id: active_id,
                    mode,
                    phase: StartPhase::Recorder,
                    ..
                } = self.state
                else {
                    return Vec::new();
                };
                if session_id != active_id {
                    return Vec::new();
                }
                if let RecordingState::Starting { phase, .. } = &mut self.state {
                    *phase = StartPhase::Orchestrator;
                }
                vec![SessionEffect::StartOrchestrator { session_id, mode }]
            }
            SessionEvent::RecorderStartFailed { session_id, .. } => {
                if self.matches_start(session_id) {
                    self.state = RecordingState::Idle;
                    vec![SessionEffect::SetTrayRecording(false)]
                } else {
                    Vec::new()
                }
            }
            SessionEvent::RecorderAlreadyRecording {
                requested_session_id,
                active_session_id,
            } => {
                if !self.matches_start(requested_session_id) {
                    return Vec::new();
                }
                self.state = RecordingState::Recovering {
                    session_id: Some(active_session_id),
                };
                let effects = vec![
                    SessionEffect::CancelRecorder {
                        session_id: active_session_id,
                    },
                    SessionEffect::AbortOrchestrator {
                        session_id: active_session_id,
                    },
                    SessionEffect::SetTrayRecording(false),
                ];
                self.state = RecordingState::Idle;
                effects
            }
            SessionEvent::OrchestratorStarted { session_id } => {
                let RecordingState::Starting {
                    session_id: active_id,
                    mode,
                    source,
                    phase: StartPhase::Orchestrator,
                } = self.state
                else {
                    return Vec::new();
                };
                if session_id != active_id {
                    return Vec::new();
                }
                self.state = RecordingState::Recording {
                    session_id,
                    mode,
                    source,
                };
                vec![SessionEffect::SetTrayRecording(true)]
            }
            SessionEvent::OrchestratorStartFailed {
                requested_session_id,
                active_session_id,
                ..
            } => {
                if !self.matches_start(requested_session_id) {
                    return Vec::new();
                }
                self.state = RecordingState::Idle;
                vec![
                    SessionEffect::CancelRecorder {
                        session_id: requested_session_id,
                    },
                    SessionEffect::AbortOrchestrator {
                        session_id: active_session_id.unwrap_or(requested_session_id),
                    },
                    SessionEffect::SetTrayRecording(false),
                ]
            }
            SessionEvent::ChunkReady { session_id, chunk } => {
                if self.active_session_id() == Some(session_id)
                    && matches!(self.state, RecordingState::Recording { .. })
                {
                    vec![SessionEffect::SubmitChunk { session_id, chunk }]
                } else {
                    Vec::new()
                }
            }
            SessionEvent::RecorderStopped {
                session_id, chunks, ..
            } => self.recorder_stopped(session_id, chunks),
            SessionEvent::RecorderNotRecording { session_id } => {
                self.recorder_stopped(session_id, Vec::new())
            }
            SessionEvent::RecorderStillRecording { session_id, .. } => {
                let RecordingState::Stopping {
                    session_id: active_id,
                    mode,
                    source,
                    phase: StopPhase::Recorder,
                } = self.state
                else {
                    return Vec::new();
                };
                if session_id != active_id {
                    return Vec::new();
                }
                self.state = RecordingState::Recording {
                    session_id,
                    mode,
                    source,
                };
                vec![SessionEffect::SetTrayRecording(true)]
            }
            SessionEvent::OrchestratorFinished { session_id } => {
                let RecordingState::Stopping {
                    session_id: active_id,
                    phase: StopPhase::Orchestrator,
                    ..
                } = self.state
                else {
                    return Vec::new();
                };
                if session_id != active_id {
                    return Vec::new();
                }
                self.state = RecordingState::Idle;
                Vec::new()
            }
            SessionEvent::ShutdownRequested => unreachable!(),
        }
    }

    fn handle_control(&mut self, control: ControlEvent) -> Vec<SessionEffect> {
        match self.state {
            RecordingState::Idle => match control.action {
                ControlAction::Start(mode) | ControlAction::Toggle(mode) => {
                    let session_id = SessionId(self.next_session_id);
                    self.next_session_id += 1;
                    self.state = RecordingState::Starting {
                        session_id,
                        mode,
                        source: control.source,
                        phase: StartPhase::Recorder,
                    };
                    vec![SessionEffect::StartRecorder { session_id }]
                }
                ControlAction::Stop => Vec::new(),
            },
            RecordingState::Recording {
                session_id,
                mode,
                source,
            } => {
                let should_stop = matches!(control.action, ControlAction::Toggle(_))
                    || (matches!(control.action, ControlAction::Stop)
                        && control.source == ControlSource::HoldHotkey
                        && mode == SessionMode::Hold);
                if !should_stop {
                    return Vec::new();
                }
                self.state = RecordingState::Stopping {
                    session_id,
                    mode,
                    source,
                    phase: StopPhase::Recorder,
                };
                vec![SessionEffect::StopRecorder { session_id }]
            }
            _ => Vec::new(),
        }
    }

    fn recorder_stopped(
        &mut self,
        session_id: SessionId,
        chunks: Vec<WavChunk>,
    ) -> Vec<SessionEffect> {
        let RecordingState::Stopping {
            session_id: active_id,
            phase: StopPhase::Recorder,
            ..
        } = self.state
        else {
            return Vec::new();
        };
        if session_id != active_id {
            return Vec::new();
        }
        if let RecordingState::Stopping { phase, .. } = &mut self.state {
            *phase = StopPhase::Orchestrator;
        }
        let mut effects = Vec::with_capacity(chunks.len() + 2);
        effects.push(SessionEffect::SetTrayRecording(false));
        effects.extend(
            chunks
                .into_iter()
                .map(|chunk| SessionEffect::SubmitChunk { session_id, chunk }),
        );
        effects.push(SessionEffect::FinishOrchestrator { session_id });
        effects
    }

    fn shutdown(&mut self) -> Vec<SessionEffect> {
        let session_id = self.active_session_id();
        self.state = RecordingState::ShuttingDown { session_id };
        let mut effects = Vec::new();
        if let Some(session_id) = session_id {
            effects.push(SessionEffect::CancelRecorder { session_id });
            effects.push(SessionEffect::AbortOrchestrator { session_id });
        }
        effects.push(SessionEffect::SetTrayRecording(false));
        effects.push(SessionEffect::ReadyToExit);
        effects
    }

    fn active_session_id(&self) -> Option<SessionId> {
        match self.state {
            RecordingState::Starting { session_id, .. }
            | RecordingState::Recording { session_id, .. }
            | RecordingState::Stopping { session_id, .. } => Some(session_id),
            RecordingState::Recovering { session_id }
            | RecordingState::ShuttingDown { session_id } => session_id,
            RecordingState::Idle => None,
        }
    }

    fn matches_start(&self, session_id: SessionId) -> bool {
        matches!(
            self.state,
            RecordingState::Starting {
                session_id: active_id,
                ..
            } if active_id == session_id
        )
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
