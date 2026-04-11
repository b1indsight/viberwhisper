from __future__ import annotations

import io
import logging
import os
import tempfile
import time
from typing import Any

from fastapi import HTTPException

LOGGER = logging.getLogger("viberwhisper.local_server")


def build_transcription_prompt(language: str | None, prompt: str | None) -> str:
    parts = [
        "Transcribe the following speech segment in its original language.",
        "Preserve punctuation and formatting when they are clear from the audio.",
        "Return only the transcription text.",
    ]
    if language:
        parts.append(f"Expected language: {language}.")
    if prompt:
        parts.append(f"Additional context: {prompt}")
    return " ".join(parts)


def transcribe_audio(
    runtime: Any,
    wav_bytes: bytes,
    language: str | None,
    prompt: str | None,
) -> dict[str, Any]:
    runtime.ensure_ready()
    started_at = time.perf_counter()

    import soundfile as sf

    samples, sample_rate = sf.read(io.BytesIO(wav_bytes), dtype="float32")
    if len(samples.shape) > 1:
        samples = samples.mean(axis=1)
    duration = float(len(samples) / sample_rate) if sample_rate else 0.0
    if duration > 30.0:
        raise HTTPException(
            status_code=400, detail="audio duration exceeds 30 seconds",
        )

    instruction = build_transcription_prompt(language, prompt)
    LOGGER.info(
        "Starting audio transcription bytes=%s sample_rate=%s duration=%.3fs language=%s prompt_chars=%s",
        len(wav_bytes),
        sample_rate,
        duration,
        language or "auto",
        len(prompt) if prompt else 0,
    )

    tmp_path: str | None = None
    try:
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            sf.write(tmp.name, samples, sample_rate)
            tmp_path = tmp.name
        text = runtime.generate_from_audio(instruction, tmp_path)
    finally:
        if tmp_path and os.path.exists(tmp_path):
            os.unlink(tmp_path)

    elapsed = time.perf_counter() - started_at
    LOGGER.info(
        "Completed audio transcription duration=%.3fs output_chars=%s",
        elapsed,
        len(text.strip()),
    )

    return {
        "text": text.strip(),
        "language": language or "auto",
        "duration": round(duration, 3),
    }
