# Server Test Guide

This directory contains the local Gemma server implementation and its test suite.

## Test Layout

- `tests/test_server.py`
  FastAPI request/response tests.
- `tests/test_runtime.py`
  Runtime utility tests that do not load a real model.
- `tests/test_chat.py`
  Chat flow tests with stub runtime objects.
- `tests/test_transcription.py`
  Audio transcription flow tests with fake audio dependencies.
- `tests/test_integration_runtime.py`
  Real-model load and quantization checks.
- `tests/test_integration_inference.py`
  Real-model inference check using the fixed fixture WAV.

## Default Paths

- Default model directory: `~/.viberwhisper/model`
- Default test audio: `server/tests/fixtures/test_audio.wav`
- Real-model probe Python: the current test interpreter (`sys.executable`)

The integration tests no longer take model-path or wav-path parameters. They always use the default model directory and the committed fixture audio.

## Environment Setup

Use a Python environment that contains the server runtime dependencies.

Example:

```bash
python3 -m venv .venv
. .venv/bin/activate
pip install -r server/requirements.txt
pip install pytest
```

If you already have a working local runtime environment, run the tests from that environment instead.

## Running Tests

Fast tests:

```bash
pytest -q server/tests
```

Real-model tests are opt-in:

```bash
VIBERWHISPER_RUN_REAL_MODEL_TESTS=1 pytest -q \
  server/tests/test_integration_runtime.py \
  server/tests/test_integration_inference.py
```

If you want to run everything in one go:

```bash
VIBERWHISPER_RUN_REAL_MODEL_TESTS=1 pytest -q server/tests
```

## Real-Model Test Requirements

To run the integration tests successfully, all of the following must be true:

- `~/.viberwhisper/model` exists and contains the downloaded model files.
- `server/tests/fixtures/test_audio.wav` exists.
- The current Python environment has `torch`, `transformers`, and the rest of `server/requirements.txt`.

If any of these prerequisites are missing, the integration tests will skip instead of failing.

## What The Real-Model Tests Verify

`test_integration_runtime.py` checks:

- the model can be loaded from `~/.viberwhisper/model`
- the runtime reports `ready=True`
- no load error is reported
- the requested quantization mode is `int8`
- quantization appears to be applied through the detected backend

`test_integration_inference.py` checks:

- the real model can transcribe the committed fixture WAV
- the transcription result is non-empty
- the returned language is `zh`
- the output contains expected keywords from the fixture audio

These tests intentionally validate stable expectations instead of exact full-string equality, because real model output can vary slightly across environments and library versions.

## Common Failures

`ModuleNotFoundError: No module named 'torch'`

- You are running tests in a Python environment that does not have the runtime dependencies installed.
- Fix: activate the environment you use for the local server, or install `server/requirements.txt` into the current environment.

`default model dir missing: ~/.viberwhisper/model`

- The local model has not been installed yet.
- Fix: install or download the model into the default local runtime directory first.

`default test wav missing: server/tests/fixtures/test_audio.wav`

- The fixture audio is missing from the working tree.
- Fix: restore the fixture file before running integration tests.

Real-model test is skipped unexpectedly

- Check that `VIBERWHISPER_RUN_REAL_MODEL_TESTS=1` is set in the same command invocation.
- Check that the current interpreter is the one you expect by running:

```bash
python -c "import sys; print(sys.executable)"
```

Quantization test fails but load succeeds

- This usually means the model loaded, but the expected quantization backend was not actually applied or could not be detected.
- Check the runtime logs and verify whether the current machine supports the requested backend.
- On non-CUDA environments, backend behavior may differ from CUDA-only paths.

Inference test fails on keyword match

- The model may have loaded correctly but produced a slightly different transcription.
- Check the actual returned text first before tightening or changing the expected keywords.
- If the fixture audio changes, update the expectations together with the fixture.

## Maintenance Notes

- Keep fixture audio small and stable. Do not replace it casually, because it anchors the real-model regression tests.
- If you change the default local model location in the product, update the defaults in `server/tests/integration_helpers.py`.
- If you change quantization behavior, update both the runtime probe and the integration assertions together.
