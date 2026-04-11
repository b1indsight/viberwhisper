from __future__ import annotations

import sys

from fastapi import HTTPException

from server.transcription import build_transcription_prompt, transcribe_audio


class StubRuntime:
    def __init__(self) -> None:
        self.calls = []

    def ensure_ready(self) -> None:
        return None

    def generate_from_audio(self, prompt_text: str, audio_path: str) -> str:
        self.calls.append((prompt_text, audio_path))
        return " stub transcript "


class MonoSamples:
    def __init__(self, length: int) -> None:
        self.shape = (length,)
        self._length = length

    def __len__(self) -> int:
        return self._length


class StereoSamples:
    def __init__(self, length: int) -> None:
        self.shape = (length, 2)
        self._length = length

    def __len__(self) -> int:
        return self._length

    def mean(self, axis: int) -> MonoSamples:
        assert axis == 1
        return MonoSamples(self._length)


class FakeSoundFile:
    def __init__(self, samples, sample_rate: int) -> None:
        self.samples = samples
        self.sample_rate = sample_rate
        self.last_write = None

    def read(self, _source, dtype: str):
        assert dtype == "float32"
        return self.samples, self.sample_rate

    def write(self, path: str, samples, sample_rate: int) -> None:
        self.last_write = (path, samples, sample_rate)


def install_fake_soundfile(monkeypatch, *, samples, sample_rate: int = 16000) -> FakeSoundFile:
    fake_sf = FakeSoundFile(samples, sample_rate)
    monkeypatch.setitem(sys.modules, "soundfile", fake_sf)
    return fake_sf


def test_build_transcription_prompt_includes_optional_context() -> None:
    prompt = build_transcription_prompt("zh", "keep punctuation")

    assert "Expected language: zh." in prompt
    assert "Additional context: keep punctuation" in prompt


def test_transcribe_audio_returns_text_language_and_duration(monkeypatch) -> None:
    runtime = StubRuntime()
    fake_sf = install_fake_soundfile(monkeypatch, samples=MonoSamples(16000))

    response = transcribe_audio(runtime, b"wav-bytes", "zh", "keep punctuation")

    assert response == {
        "text": "stub transcript",
        "language": "zh",
        "duration": 1.0,
    }
    assert len(runtime.calls) == 1
    assert "Expected language: zh." in runtime.calls[0][0]
    assert fake_sf.last_write is not None


def test_transcribe_audio_converts_multichannel_audio_to_mono(monkeypatch) -> None:
    runtime = StubRuntime()
    stereo = StereoSamples(8000)
    fake_sf = install_fake_soundfile(monkeypatch, samples=stereo, sample_rate=8000)

    response = transcribe_audio(runtime, b"wav-bytes", None, None)

    assert response["language"] == "auto"
    assert response["duration"] == 1.0
    assert len(runtime.calls) == 1
    assert isinstance(fake_sf.last_write[1], MonoSamples)


def test_transcribe_audio_rejects_long_audio(monkeypatch) -> None:
    runtime = StubRuntime()
    install_fake_soundfile(monkeypatch, samples=MonoSamples(31 * 16000))

    try:
        transcribe_audio(runtime, b"wav-bytes", None, None)
    except HTTPException as error:
        assert error.status_code == 400
        assert error.detail == "audio duration exceeds 30 seconds"
    else:
        raise AssertionError("expected HTTPException")
