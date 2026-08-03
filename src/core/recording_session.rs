use crate::audio::WavChunk;
use crate::session::SessionId;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Starting { session_id: SessionId },
    Recording { session_id: SessionId },
    Stopping { session_id: SessionId },
    ShuttingDown { session_id: Option<SessionId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    StartRequested,
    StopRequested,
    SessionStarted {
        session_id: SessionId,
    },
    SessionStartFailed {
        session_id: SessionId,
        error: String,
    },
    ChunkReady {
        session_id: SessionId,
        chunk: WavChunk,
    },
    SessionStopped {
        session_id: SessionId,
    },
    SessionStopFailed {
        session_id: SessionId,
        error: String,
    },
    ShutdownRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEffect {
    StartSession {
        session_id: SessionId,
    },
    StopSession {
        session_id: SessionId,
    },
    SubmitChunk {
        session_id: SessionId,
        chunk: WavChunk,
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
            Self::StartRequested => ("start_requested", None),
            Self::StopRequested => ("stop_requested", None),
            Self::SessionStarted { session_id } => ("session_started", Some(*session_id)),
            Self::SessionStartFailed { session_id, .. } => {
                ("session_start_failed", Some(*session_id))
            }
            Self::ChunkReady { session_id, .. } => ("chunk_ready", Some(*session_id)),
            Self::SessionStopped { session_id } => ("session_stopped", Some(*session_id)),
            Self::SessionStopFailed { session_id, .. } => {
                ("session_stop_failed", Some(*session_id))
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
/// state/event match arm are then rejected without mutating the current state.
fn transition(
    state: RecordingState,
    event: SessionEvent,
    next_session_id: &mut u64,
) -> Option<Transition> {
    match (state, event) {
        (RecordingState::ShuttingDown { .. }, _) => None,
        (state, SessionEvent::ShutdownRequested) => Some(shutdown_transition(state)),
        (RecordingState::Idle, SessionEvent::StartRequested) => {
            let session_id = SessionId(*next_session_id);
            *next_session_id += 1;
            Some(Transition {
                next: RecordingState::Starting { session_id },
                effects: vec![SessionEffect::StartSession { session_id }],
            })
        }
        (RecordingState::Starting { session_id }, SessionEvent::SessionStarted { .. }) => {
            Some(Transition {
                next: RecordingState::Recording { session_id },
                effects: vec![SessionEffect::SetTrayRecording(true)],
            })
        }
        (RecordingState::Starting { .. }, SessionEvent::SessionStartFailed { .. }) => {
            Some(Transition {
                next: RecordingState::Idle,
                effects: vec![SessionEffect::SetTrayRecording(false)],
            })
        }
        (
            state @ RecordingState::Recording { session_id, .. },
            SessionEvent::ChunkReady { chunk, .. },
        ) => Some(Transition {
            next: state,
            effects: vec![SessionEffect::SubmitChunk { session_id, chunk }],
        }),
        (RecordingState::Recording { session_id }, SessionEvent::StopRequested) => {
            Some(Transition {
                next: RecordingState::Stopping { session_id },
                effects: vec![
                    SessionEffect::SetTrayRecording(false),
                    SessionEffect::StopSession { session_id },
                ],
            })
        }
        (RecordingState::Stopping { .. }, SessionEvent::SessionStopped { .. }) => {
            Some(Transition {
                next: RecordingState::Idle,
                effects: Vec::new(),
            })
        }
        (RecordingState::Stopping { session_id }, SessionEvent::SessionStopFailed { .. }) => {
            Some(Transition {
                next: RecordingState::Recording { session_id },
                effects: vec![SessionEffect::SetTrayRecording(true)],
            })
        }
        _ => None,
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

    #[test]
    fn explicit_transition_table_rejects_unlisted_paths() {
        let mut next_session_id = 2;

        let result = transition(
            RecordingState::Starting {
                session_id: SessionId(1),
            },
            SessionEvent::StopRequested,
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
            },
            SessionEvent::SessionStopped {
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
    fn session_runs_one_start_and_stop_chain() {
        let mut machine = RecordingSessionMachine::new();
        let chunk = test_chunk();

        assert_eq!(
            machine.handle(SessionEvent::StartRequested),
            vec![SessionEffect::StartSession {
                session_id: SessionId(1)
            }]
        );
        assert_eq!(
            machine.state(),
            &RecordingState::Starting {
                session_id: SessionId(1)
            }
        );
        assert_eq!(
            machine.handle(SessionEvent::SessionStarted {
                session_id: SessionId(1)
            }),
            vec![SessionEffect::SetTrayRecording(true)]
        );
        assert_eq!(
            machine.state(),
            &RecordingState::Recording {
                session_id: SessionId(1)
            }
        );

        assert_eq!(
            machine.handle(SessionEvent::ChunkReady {
                session_id: SessionId(1),
                chunk: chunk.clone(),
            }),
            vec![SessionEffect::SubmitChunk {
                session_id: SessionId(1),
                chunk,
            }]
        );
        assert_eq!(
            machine.handle(SessionEvent::StopRequested),
            vec![
                SessionEffect::SetTrayRecording(false),
                SessionEffect::StopSession {
                    session_id: SessionId(1)
                },
            ]
        );
        assert_eq!(
            machine.state(),
            &RecordingState::Stopping {
                session_id: SessionId(1)
            }
        );
        assert!(
            machine
                .handle(SessionEvent::SessionStopped {
                    session_id: SessionId(1)
                })
                .is_empty()
        );
        assert_eq!(machine.state(), &RecordingState::Idle);

        // A source-free release or toggle normalized after completion is rejected.
        assert!(machine.handle(SessionEvent::StopRequested).is_empty());
    }

    #[test]
    fn stale_chunks_and_session_results_cannot_mutate_active_session() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(SessionEvent::StartRequested);
        machine.handle(SessionEvent::SessionStarted {
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
                .handle(SessionEvent::SessionStopFailed {
                    session_id: SessionId(99),
                    error: "stale".into(),
                })
                .is_empty()
        );
        assert!(matches!(machine.state(), RecordingState::Recording { .. }));
    }

    #[test]
    fn shutdown_cancels_active_session_and_is_idempotent() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(SessionEvent::StartRequested);
        machine.handle(SessionEvent::SessionStarted {
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
        assert!(machine.handle(SessionEvent::StartRequested).is_empty());
    }

    #[test]
    fn session_start_failure_returns_to_idle_and_next_start_gets_a_new_id() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(SessionEvent::StartRequested);
        assert_eq!(
            machine.handle(SessionEvent::SessionStartFailed {
                session_id: SessionId(1),
                error: "device unavailable".into(),
            }),
            vec![SessionEffect::SetTrayRecording(false)]
        );
        assert_eq!(machine.state(), &RecordingState::Idle);

        assert_eq!(
            machine.handle(SessionEvent::StartRequested),
            vec![SessionEffect::StartSession {
                session_id: SessionId(2)
            }]
        );
    }

    #[test]
    fn session_stop_failure_restores_recording() {
        let mut machine = RecordingSessionMachine::new();
        machine.handle(SessionEvent::StartRequested);
        machine.handle(SessionEvent::SessionStarted {
            session_id: SessionId(1),
        });
        machine.handle(SessionEvent::StopRequested);

        assert_eq!(
            machine.handle(SessionEvent::SessionStopFailed {
                session_id: SessionId(1),
                error: "backend busy".into(),
            }),
            vec![SessionEffect::SetTrayRecording(true)]
        );
        assert_eq!(
            machine.state(),
            &RecordingState::Recording {
                session_id: SessionId(1)
            }
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
