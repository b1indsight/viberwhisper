from __future__ import annotations

from typing import Any

from pydantic import BaseModel


class ChatMessage(BaseModel):
    role: str
    content: Any


class ChatCompletionRequest(BaseModel):
    model: str | None = None
    messages: list[ChatMessage]
    temperature: float | None = 0.0
    stream: bool | None = False
    max_tokens: int | None = None
