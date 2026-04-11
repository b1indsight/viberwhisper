from __future__ import annotations

import logging
import time
from typing import Any

from fastapi import HTTPException

from server.models import ChatCompletionRequest, ChatMessage
from server.runtime import DEFAULT_MAX_NEW_TOKENS, MODEL_NAME

LOGGER = logging.getLogger("viberwhisper.local_server")


def flatten_content(content: Any) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for item in content:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict) and item.get("type") == "text":
                parts.append(str(item.get("text", "")))
        return "".join(parts)
    if content is None:
        return ""
    return str(content)


def render_chat_prompt(processor: Any, messages: list[ChatMessage]) -> str:
    normalized = [
        {
            "role": msg.role,
            "content": [
                {"type": "text", "text": flatten_content(msg.content)},
            ],
        }
        for msg in messages
    ]
    return processor.apply_chat_template(
        normalized,
        tokenize=False,
        add_generation_prompt=True,
        enable_thinking=False,
    )


def chat_complete(runtime: Any, request: ChatCompletionRequest) -> dict[str, Any]:
    runtime.ensure_ready()
    started_at = time.perf_counter()

    if request.stream:
        raise HTTPException(
            status_code=400, detail="stream=true is not supported",
        )

    LOGGER.info(
        "Starting chat completion messages=%s temperature=%s max_tokens=%s",
        len(request.messages),
        request.temperature,
        request.max_tokens or DEFAULT_MAX_NEW_TOKENS,
    )

    prompt_text = render_chat_prompt(runtime.processor, request.messages)
    content = runtime.generate_from_text(
        prompt_text,
        temperature=request.temperature or 0.0,
        max_new_tokens=request.max_tokens or DEFAULT_MAX_NEW_TOKENS,
    ).strip()
    created = int(time.time())
    elapsed = time.perf_counter() - started_at
    LOGGER.info(
        "Completed chat completion duration=%.3fs output_chars=%s",
        elapsed,
        len(content),
    )

    return {
        "id": f"chatcmpl-local-{created}",
        "object": "chat.completion",
        "created": created,
        "model": MODEL_NAME,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop",
            },
        ],
        "usage": {
            "prompt_tokens": None,
            "completion_tokens": None,
            "total_tokens": None,
        },
    }
