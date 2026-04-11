from __future__ import annotations

import json
import sys
from pathlib import Path

from server.runtime import LocalModelRuntime
from server.tests.integration_helpers import DEFAULT_MODEL_DIR, DEFAULT_TEST_WAV
from server.transcription import transcribe_audio


def probe_load() -> dict:
    runtime = LocalModelRuntime(str(DEFAULT_MODEL_DIR), "int8")
    runtime.load()
    return {
        "ready": runtime.ready,
        "error": runtime.error,
        "quantization": runtime.quantization_state(),
    }


def probe_inference() -> dict:
    runtime = LocalModelRuntime(str(DEFAULT_MODEL_DIR), "int8")
    runtime.load()
    if not runtime.ready:
        return {
            "ready": runtime.ready,
            "error": runtime.error,
            "quantization": runtime.quantization_state(),
        }

    response = transcribe_audio(
        runtime,
        Path(DEFAULT_TEST_WAV).read_bytes(),
        "zh",
        "以下是一段简体中文的普通话句子，去掉首尾的语气词",
    )
    return {
        "ready": runtime.ready,
        "error": runtime.error,
        "quantization": runtime.quantization_state(),
        "response": response,
    }


def main() -> int:
    command = sys.argv[1]
    if command == "load":
        result = probe_load()
    elif command == "inference":
        result = probe_inference()
    else:
        raise SystemExit(f"unknown command: {command}")

    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
