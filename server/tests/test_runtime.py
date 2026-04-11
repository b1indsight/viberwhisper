from __future__ import annotations

from server.runtime import LocalModelRuntime


def test_health_payload_states() -> None:
    runtime = LocalModelRuntime("/tmp/model", "int8")
    assert runtime.health_payload() == (503, {"status": "loading", "model": "gemma-4-E2B-it"})

    runtime.ready = True
    assert runtime.health_payload() == (200, {"status": "ok", "model": "gemma-4-E2B-it"})

    runtime.ready = False
    runtime.error = "boom"
    assert runtime.health_payload() == (
        500,
        {"status": "error", "model": "gemma-4-E2B-it", "error": "boom"},
    )


def test_base_load_kwargs_for_mps_avoids_device_map() -> None:
    runtime = LocalModelRuntime("/tmp/model", "int8")
    load_kwargs = runtime._base_load_kwargs("mps")

    assert load_kwargs["low_cpu_mem_usage"] is True
    assert load_kwargs["device_map"] == {"": "mps"}


def test_base_load_kwargs_for_cuda_uses_auto_dispatch() -> None:
    runtime = LocalModelRuntime("/tmp/model", "int8")
    load_kwargs = runtime._base_load_kwargs("cuda")

    assert load_kwargs["device_map"] == "auto"
    assert load_kwargs["low_cpu_mem_usage"] is True


def test_extract_text_response_accepts_string() -> None:
    assert LocalModelRuntime.extract_text_response("hello") == "hello"


def test_extract_text_response_extracts_text_from_dict() -> None:
    assert LocalModelRuntime.extract_text_response({"text": "hello"}) == "hello"
    assert LocalModelRuntime.extract_text_response({"content": "world"}) == "world"


def test_extract_text_response_rejects_dict_without_text() -> None:
    try:
        LocalModelRuntime.extract_text_response({"foo": "bar"})
    except ValueError as error:
        assert "text field" in str(error)
    else:
        raise AssertionError("expected ValueError")
