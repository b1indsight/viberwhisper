from __future__ import annotations

from typing import Any

from fastapi import HTTPException
from fastapi.testclient import TestClient

from server.models import ChatCompletionRequest
from server.server import create_app


class StubRuntime:
    def __init__(self) -> None:
        self.loaded = False
        self.last_audio_args: tuple[bytes, str | None, str | None] | None = None
        self.last_chat_request: ChatCompletionRequest | None = None
        self.last_chat_prompt: str | None = None
        self.processor = self

    def load(self) -> None:
        self.loaded = True

    def health_payload(self) -> tuple[int, dict[str, str]]:
        return 200, {"status": "ok", "model": "stub"}

    def ensure_ready(self) -> None:
        return None

    def generate_from_audio(self, prompt_text: str, audio_path: str) -> str:
        del audio_path
        self.last_audio_args = (b"wav-bytes", "zh", "keep punctuation")
        assert "Expected language: zh." in prompt_text
        return "stub transcript"

    def apply_chat_template(self, messages, **_: object) -> str:
        self.last_chat_request = ChatCompletionRequest(messages=messages)
        return "rendered prompt"

    def generate_from_text(
        self,
        prompt_text: str,
        *,
        temperature: float,
        max_new_tokens: int,
    ) -> str:
        del temperature, max_new_tokens
        self.last_chat_prompt = prompt_text
        return "stub response"


class FailingRuntime(StubRuntime):
    def health_payload(self) -> tuple[int, dict[str, str]]:
        raise HTTPException(status_code=503, detail="loading")


def test_create_app_health_endpoint() -> None:
    runtime = StubRuntime()
    with TestClient(create_app(runtime)) as client:
        response = client.get("/health")

    assert response.status_code == 200
    assert response.json() == {"status": "ok", "model": "stub"}
    assert runtime.loaded is True


def test_create_app_audio_transcriptions_endpoint(monkeypatch) -> None:
    runtime = StubRuntime()

    def fake_transcribe_audio(
        runtime_obj: Any,
        wav_bytes: bytes,
        language: str | None,
        prompt: str | None,
    ) -> dict[str, object]:
        assert runtime_obj is runtime
        runtime.last_audio_args = (wav_bytes, language, prompt)
        return {"text": "stub transcript", "language": language or "auto", "duration": 1.25}

    monkeypatch.setattr("server.server.transcribe_audio", fake_transcribe_audio)

    with TestClient(create_app(runtime)) as client:
        response = client.post(
            "/v1/audio/transcriptions",
            files={"file": ("sample.wav", b"wav-bytes", "audio/wav")},
            data={"language": "zh", "prompt": "keep punctuation", "model": "ignored"},
        )

    assert response.status_code == 200
    assert response.json()["text"] == "stub transcript"
    assert runtime.last_audio_args == (b"wav-bytes", "zh", "keep punctuation")


def test_create_app_chat_completions_endpoint() -> None:
    runtime = StubRuntime()
    with TestClient(create_app(runtime)) as client:
        response = client.post(
            "/v1/chat/completions",
            json={
                "messages": [
                    {"role": "system", "content": "clean up text"},
                    {"role": "user", "content": "你好"},
                ],
                "stream": False,
            },
        )

    assert response.status_code == 200
    assert response.json()["choices"][0]["message"]["content"] == "stub response"
    assert runtime.last_chat_request is not None
    assert [message.role for message in runtime.last_chat_request.messages] == [
        "system",
        "user",
    ]
    assert runtime.last_chat_prompt == "rendered prompt"


def test_http_exception_handler_returns_json() -> None:
    runtime = FailingRuntime()
    with TestClient(create_app(runtime)) as client:
        response = client.get("/health")

    assert response.status_code == 503
    assert response.json() == {"detail": "loading"}
