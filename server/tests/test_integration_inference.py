from __future__ import annotations

from server.tests.integration_helpers import (
    normalize_text,
    require_real_model_test_environment,
    run_probe,
)


def test_real_audio_transcription_matches_expected_keywords() -> None:
    require_real_model_test_environment()
    result = run_probe("inference")

    assert result["ready"] is True
    assert result["error"] is None

    response = result["response"]
    normalized = normalize_text(response["text"])

    assert response["language"] == "zh"
    assert response["duration"] > 0
    assert normalized
    assert "超市" in normalized
    assert "水果" in normalized
