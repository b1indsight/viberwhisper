from __future__ import annotations

from fastapi.testclient import TestClient

from server.server import ChatCompletionRequest, LocalModelRuntime, create_app


class StubRuntime:
    def __init__(self) -> None:
        self.loaded = False
        self.last_audio_args: tuple[bytes, str | None, str | None] | None = None
        self.last_chat_request: ChatCompletionRequest | None = None

    def load(self) -> None:
        self.loaded = True

    def health_payload(self) -> tuple[int, dict[str, str]]:
        return 200, {"status": "ok", "model": "stub"}

    def transcribe_audio(
        self,
        wav_bytes: bytes,
        language: str | None,
        prompt: str | None,
    ) -> dict[str, object]:
        self.last_audio_args = (wav_bytes, language, prompt)
        return {"text": "stub transcript", "language": language or "auto", "duration": 1.25}

    def chat_complete(self, request: ChatCompletionRequest) -> dict[str, object]:
        self.last_chat_request = request
        return {
            "id": "chatcmpl-local-test",
            "object": "chat.completion",
            "created": 0,
            "model": "stub",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "stub response"},
                    "finish_reason": "stop",
                },
            ],
            "usage": {
                "prompt_tokens": None,
                "completion_tokens": None,
                "total_tokens": None,
            },
        }


def test_health_payload_states() -> None:
    runtime = LocalModelRuntime("/tmp/model", "int8")
    assert runtime.health_payload() == (503, {"status": "loading", "model": "gemma-4-E4B-it"})

    runtime.ready = True
    assert runtime.health_payload() == (200, {"status": "ok", "model": "gemma-4-E4B-it"})

    runtime.ready = False
    runtime.error = "boom"
    assert runtime.health_payload() == (
        500,
        {"status": "error", "model": "gemma-4-E4B-it", "error": "boom"},
    )


def test_flatten_content_handles_supported_shapes() -> None:
    runtime = LocalModelRuntime("/tmp/model", "int8")

    assert runtime._flatten_content("hello") == "hello"
    assert runtime._flatten_content(
        ["a", {"type": "text", "text": "b"}, {"type": "image", "url": "ignored"}],
    ) == "ab"
    assert runtime._flatten_content(None) == ""


def test_create_app_health_endpoint() -> None:
    runtime = StubRuntime()
    with TestClient(create_app(runtime)) as client:
        response = client.get("/health")

    assert response.status_code == 200
    assert response.json() == {"status": "ok", "model": "stub"}
    assert runtime.loaded is True


def test_create_app_audio_transcriptions_endpoint() -> None:
    runtime = StubRuntime()
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
