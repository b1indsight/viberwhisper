from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

DEFAULT_MODEL_DIR = Path.home() / ".viberwhisper" / "model"
DEFAULT_TEST_WAV = Path(__file__).resolve().parent / "fixtures" / "test_audio.wav"
DEFAULT_RUNTIME_PYTHON = Path(sys.executable)
REPO_ROOT = Path(__file__).resolve().parents[2]
RUN_REAL_TESTS_ENV = "VIBERWHISPER_RUN_REAL_MODEL_TESTS"


def require_real_model_test_environment() -> tuple[Path, Path, Path]:
    import pytest

    if os.environ.get(RUN_REAL_TESTS_ENV) != "1":
        pytest.skip(f"set {RUN_REAL_TESTS_ENV}=1 to run real model tests")
    if not DEFAULT_MODEL_DIR.is_dir():
        pytest.skip(f"default model dir missing: {DEFAULT_MODEL_DIR}")
    if not DEFAULT_TEST_WAV.is_file():
        pytest.skip(f"default test wav missing: {DEFAULT_TEST_WAV}")
    if not DEFAULT_RUNTIME_PYTHON.is_file():
        pytest.skip(f"default runtime python missing: {DEFAULT_RUNTIME_PYTHON}")
    try:
        import torch  # noqa: F401
        import transformers  # noqa: F401
    except ModuleNotFoundError as error:
        pytest.skip(
            "real model tests require torch and transformers in the current Python "
            f"environment ({DEFAULT_RUNTIME_PYTHON}): {error}"
        )
    return DEFAULT_MODEL_DIR, DEFAULT_TEST_WAV, DEFAULT_RUNTIME_PYTHON


def normalize_text(text: str) -> str:
    punctuation = " ，。！？、,.!?"
    return "".join(ch for ch in text.strip() if ch not in punctuation)


def run_probe(command: str) -> dict:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(REPO_ROOT)
    process = subprocess.run(
        [str(DEFAULT_RUNTIME_PYTHON), str(Path(__file__).resolve().parent / "real_runtime_probe.py"), command],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if process.returncode != 0:
        raise AssertionError(process.stderr.strip() or process.stdout.strip())

    import json

    return json.loads(process.stdout)
