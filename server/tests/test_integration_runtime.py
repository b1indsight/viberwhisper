from __future__ import annotations

from server.tests.integration_helpers import (
    require_real_model_test_environment,
    run_probe,
)


def test_runtime_loads_real_model_and_applies_quantization() -> None:
    require_real_model_test_environment()
    result = run_probe("load")

    assert result["ready"] is True
    assert result["error"] is None

    quantization = result["quantization"]
    assert quantization["requested"] == "int8"
    assert isinstance(quantization["device"], str)
    assert quantization["device"].startswith(("cpu", "cuda", "mps"))
    assert quantization["applied"] is True
    assert quantization["backend"] in {"optimum-quanto", "bitsandbytes"}
    assert quantization["mode"] == "int8"
