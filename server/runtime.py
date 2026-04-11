from __future__ import annotations

import logging
import threading
from typing import Any

from fastapi import HTTPException

LOGGER = logging.getLogger("viberwhisper.local_server")
MODEL_NAME = "gemma-4-E2B-it"
DEFAULT_MAX_NEW_TOKENS = 512


class LocalModelRuntime:
    def __init__(self, model_dir: str, quantization: str) -> None:
        self.model_dir = model_dir
        self.quantization = quantization
        self.processor: Any | None = None
        self.model: Any | None = None
        self.ready = False
        self.error: str | None = None
        self._lock = threading.Lock()

    def load(self) -> None:
        with self._lock:
            if self.ready or self.error:
                return
            try:
                target_device = self._target_device()
                import torch

                LOGGER.info(
                    "Loading Gemma runtime from %s (%s, device=%s)",
                    self.model_dir,
                    self.quantization,
                    target_device,
                )
                load_kwargs = self._base_load_kwargs(
                    target_device,
                    mps_dtype=torch.float16,
                )

                quanto_dtype = None
                if self.quantization in ("int8", "int4"):
                    quanto_dtype = self._try_quanto_quantization(
                        self.quantization, load_kwargs, target_device,
                    )

                from transformers import AutoModelForMultimodalLM, AutoProcessor

                LOGGER.info("Loading AutoProcessor from %s", self.model_dir)
                processor = AutoProcessor.from_pretrained(self.model_dir)
                LOGGER.info("AutoProcessor loaded")

                LOGGER.info("Loading AutoModelForMultimodalLM with kwargs=%s", load_kwargs)
                model = AutoModelForMultimodalLM.from_pretrained(
                    self.model_dir, **load_kwargs,
                )
                LOGGER.info("AutoModelForMultimodalLM loaded")

                if quanto_dtype is not None:
                    LOGGER.info("Applying quanto quantization")
                    self._apply_quanto(model, quanto_dtype)
                    LOGGER.info("Quanto quantization applied")

                LOGGER.info("Switching model to eval mode")
                model.eval()
                LOGGER.info("Model is in eval mode")

                self.processor = processor
                self.model = model
                self.ready = True
                LOGGER.info("Gemma runtime is ready")
            except Exception as exc:
                LOGGER.exception("Failed to load Gemma runtime")
                self.error = str(exc)

    @staticmethod
    def _target_device() -> str:
        import torch

        if torch.backends.mps.is_available():
            return "mps"
        if torch.cuda.is_available():
            return "cuda"
        return "cpu"

    @staticmethod
    def _base_load_kwargs(
        target_device: str,
        *,
        mps_dtype: Any | None = None,
    ) -> dict[str, Any]:
        if target_device == "mps":
            return {
                "torch_dtype": mps_dtype if mps_dtype is not None else "float16",
                "device_map": {"": "mps"},
                "low_cpu_mem_usage": True,
            }
        return {
            "torch_dtype": "auto",
            "device_map": "auto",
            "low_cpu_mem_usage": True,
        }

    def health_payload(self) -> tuple[int, dict[str, Any]]:
        if self.ready:
            return 200, {"status": "ok", "model": MODEL_NAME}
        if self.error:
            return 500, {"status": "error", "model": MODEL_NAME, "error": self.error}
        return 503, {"status": "loading", "model": MODEL_NAME}

    def ensure_ready(self) -> None:
        if self.error:
            raise HTTPException(status_code=500, detail=self.error)
        if not self.ready or self.model is None or self.processor is None:
            raise HTTPException(status_code=503, detail="model is still loading")

    @staticmethod
    def _try_quanto_quantization(
        mode: str, load_kwargs: dict[str, Any], target_device: str,
    ) -> Any | None:
        """Try to set up optimum-quanto quantization; fall back to bitsandbytes on CUDA."""
        try:
            from optimum.quanto import qint4, qint8  # noqa: F811

            LOGGER.info("Using optimum-quanto for %s quantization", mode)
            return qint8 if mode == "int8" else qint4
        except ImportError:
            LOGGER.info(
                "optimum-quanto not available, falling back to bitsandbytes (%s)",
                mode,
            )

        if target_device != "cuda":
            LOGGER.warning(
                "bitsandbytes quantization is only supported on CUDA; "
                "loading model without backend quantization on %s",
                target_device,
            )
            return None

        try:
            import bitsandbytes as _bnb  # noqa: F401

            if mode == "int8":
                load_kwargs["load_in_8bit"] = True
            else:
                load_kwargs["load_in_4bit"] = True
            LOGGER.info("Using bitsandbytes for %s quantization (CUDA only)", mode)
        except ImportError:
            LOGGER.warning(
                "Neither optimum-quanto nor bitsandbytes available; "
                "loading model without quantization",
            )
        return None

    @staticmethod
    def _apply_quanto(model: Any, quanto_dtype: Any) -> None:
        from optimum.quanto import freeze, quantize

        quantize(model, weights=quanto_dtype)
        freeze(model)

    def generate_from_text(
        self,
        prompt_text: str,
        *,
        temperature: float,
        max_new_tokens: int,
    ) -> str:
        self.ensure_ready()
        assert self.processor is not None
        assert self.model is not None

        inputs = self.processor(text=prompt_text, return_tensors="pt").to(
            self._model_device(),
        )
        input_len = inputs["input_ids"].shape[-1]
        outputs = self.model.generate(
            **inputs,
            max_new_tokens=max_new_tokens,
            do_sample=temperature > 0,
            temperature=max(temperature, 1e-5),
        )
        response = self.processor.decode(
            outputs[0][input_len:], skip_special_tokens=False,
        )
        return self.extract_text_response(self.processor.parse_response(response))

    def generate_from_audio(self, prompt_text: str, audio_path: str) -> str:
        self.ensure_ready()
        assert self.processor is not None
        assert self.model is not None

        messages = [
            {
                "role": "user",
                "content": [
                    {"type": "audio", "audio": audio_path},
                    {"type": "text", "text": prompt_text},
                ],
            },
        ]
        inputs = self.processor.apply_chat_template(
            messages,
            tokenize=True,
            return_dict=True,
            return_tensors="pt",
            add_generation_prompt=True,
        ).to(self._model_device())
        input_len = inputs["input_ids"].shape[-1]
        outputs = self.model.generate(
            **inputs,
            max_new_tokens=DEFAULT_MAX_NEW_TOKENS,
            do_sample=False,
        )
        response = self.processor.decode(
            outputs[0][input_len:], skip_special_tokens=False,
        )
        return self.extract_text_response(self.processor.parse_response(response))

    def quantization_state(self) -> dict[str, Any]:
        model = self.model
        if model is None:
            return {
                "requested": self.quantization,
                "device": None,
                "backend": "unloaded",
                "applied": False,
            }

        backend = "none"
        mode = None

        if getattr(model, "is_loaded_in_8bit", False):
            backend = "bitsandbytes"
            mode = "int8"
        elif getattr(model, "is_loaded_in_4bit", False):
            backend = "bitsandbytes"
            mode = "int4"

        quantization_config = getattr(model, "quantization_config", None)
        if backend == "none" and quantization_config is not None:
            backend_name = type(quantization_config).__name__.lower()
            backend = "bitsandbytes" if "bits" in backend_name else backend_name
            if getattr(quantization_config, "load_in_8bit", False):
                mode = "int8"
            elif getattr(quantization_config, "load_in_4bit", False):
                mode = "int4"

        if backend == "none":
            for module in model.modules():
                module_name = type(module).__name__.lower()
                module_mod = type(module).__module__.lower()
                if "quanto" in module_name or "quanto" in module_mod:
                    backend = "optimum-quanto"
                    mode = self.quantization if self.quantization in {"int4", "int8"} else None
                    break

        return {
            "requested": self.quantization,
            "device": self._model_device(),
            "backend": backend,
            "mode": mode,
            "applied": backend not in {"none", "unloaded"},
        }

    @staticmethod
    def extract_text_response(parsed: Any) -> str:
        if isinstance(parsed, str):
            return parsed
        if isinstance(parsed, dict):
            for key in ("text", "content", "response", "output"):
                value = parsed.get(key)
                if isinstance(value, str):
                    return value
            LOGGER.error("Parsed response dict did not contain a text field: %s", parsed)
            raise ValueError("parsed response did not contain a text field")
        if parsed is None:
            LOGGER.error("Parsed response was None")
            raise ValueError("parsed response was None")

        LOGGER.error("Parsed response had unsupported type %s: %r", type(parsed).__name__, parsed)
        raise ValueError(f"unsupported parsed response type: {type(parsed).__name__}")

    def _model_device(self) -> str:
        assert self.model is not None
        device = getattr(self.model, "device", None)
        if device is not None:
            return str(device)
        return self._target_device()
