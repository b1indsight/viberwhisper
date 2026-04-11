from __future__ import annotations

from fastapi import HTTPException

from server.chat import chat_complete, flatten_content, render_chat_prompt
from server.models import ChatCompletionRequest


class StubProcessor:
    def __init__(self) -> None:
        self.last_messages = None

    def apply_chat_template(self, messages, **_: object) -> str:
        self.last_messages = messages
        return "rendered prompt"


class StubRuntime:
    def __init__(self) -> None:
        self.processor = StubProcessor()
        self.generate_calls = []

    def ensure_ready(self) -> None:
        return None

    def generate_from_text(
        self,
        prompt_text: str,
        *,
        temperature: float,
        max_new_tokens: int,
    ) -> str:
        self.generate_calls.append((prompt_text, temperature, max_new_tokens))
        return " stub response "


def test_flatten_content_handles_supported_shapes() -> None:
    assert flatten_content("hello") == "hello"
    assert flatten_content(
        ["a", {"type": "text", "text": "b"}, {"type": "image", "url": "ignored"}],
    ) == "ab"
    assert flatten_content(None) == ""


def test_render_chat_prompt_normalizes_message_content() -> None:
    processor = StubProcessor()
    request = ChatCompletionRequest(
        messages=[
            {"role": "system", "content": "clean"},
            {"role": "user", "content": ["你", {"type": "text", "text": "好"}]},
        ],
    )

    result = render_chat_prompt(processor, request.messages)

    assert result == "rendered prompt"
    assert processor.last_messages == [
        {"role": "system", "content": [{"type": "text", "text": "clean"}]},
        {"role": "user", "content": [{"type": "text", "text": "你好"}]},
    ]


def test_chat_complete_rejects_streaming() -> None:
    runtime = StubRuntime()
    request = ChatCompletionRequest(messages=[{"role": "user", "content": "hi"}], stream=True)

    try:
        chat_complete(runtime, request)
    except HTTPException as error:
        assert error.status_code == 400
        assert error.detail == "stream=true is not supported"
    else:
        raise AssertionError("expected HTTPException")


def test_chat_complete_uses_rendered_prompt_and_runtime_generation() -> None:
    runtime = StubRuntime()
    request = ChatCompletionRequest(
        messages=[{"role": "user", "content": "你好"}],
        temperature=0.3,
        max_tokens=64,
    )

    response = chat_complete(runtime, request)

    assert runtime.generate_calls == [("rendered prompt", 0.3, 64)]
    assert response["choices"][0]["message"]["content"] == "stub response"
    assert response["model"] == "gemma-4-E2B-it"
