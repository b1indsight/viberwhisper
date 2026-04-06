from __future__ import annotations

import argparse
import asyncio
import io
import logging
import os
import tempfile
import threading
import time
from contextlib import asynccontextmanager
from typing import Annotated, Any

from fastapi import FastAPI, File, Form, HTTPException, UploadFile
from fastapi.responses import JSONResponse
from pydantic import BaseModel

LOGGER = logging.getLogger("viberwhisper.local_server")
MODEL_NAME = "gemma-4-E4B-it"
DEFAULT_MAX_NEW_TOKENS = 512


class ChatMessage(BaseModel):
    role: str
    content: Any


class ChatCompletionRequest(BaseModel):
    model: str | None = None
    messages: list[ChatMessage]
    temperature: float | None = 0.0
    stream: bool | None = False
    max_tokens: int | None = None


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
                LOGGER.info(
                    "Loading Gemma runtime from %s (%s)",
                    self.model_dir,
                    self.quantization,
                )
                load_kwargs: dict[str, Any] = {
                    "dtype": "auto",
                    "device_map": "auto",
                    "low_cpu_mem_usage": True,
                }

                quanto_dtype = None
                if self.quantization in ("int8", "int4"):
                    quanto_dtype = self._try_quanto_quantization(
                        self.quantization, load_kwargs,
                    )

                from transformers import AutoModelForCausalLM, AutoProcessor

                processor = AutoProcessor.from_pretrained(self.model_dir)
                model = AutoModelForCausalLM.from_pretrained(
                    self.model_dir, **load_kwargs,
                )

                if quanto_dtype is not None:
                    self._apply_quanto(model, quanto_dtype)

                model.eval()

                self.processor = processor
                self.model = model
                self.ready = True
                LOGGER.info("Gemma runtime is ready")
            except Exception as exc:
                LOGGER.exception("Failed to load Gemma runtime")
                self.error = str(exc)

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

    def transcribe_audio(
        self,
        wav_bytes: bytes,
        language: str | None,
        prompt: str | None,
    ) -> dict[str, Any]:
        self.ensure_ready()

        import soundfile as sf

        samples, sample_rate = sf.read(io.BytesIO(wav_bytes), dtype="float32")
        if len(samples.shape) > 1:
            samples = samples.mean(axis=1)
        duration = float(len(samples) / sample_rate) if sample_rate else 0.0
        if duration > 30.0:
            raise HTTPException(
                status_code=400, detail="audio duration exceeds 30 seconds",
            )

        instruction = self._build_transcription_prompt(language, prompt)

        tmp_path: str | None = None
        try:
            with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
                sf.write(tmp.name, samples, sample_rate)
                tmp_path = tmp.name
            text = self._generate_from_audio(instruction, tmp_path)
        finally:
            if tmp_path and os.path.exists(tmp_path):
                os.unlink(tmp_path)

        return {
            "text": text.strip(),
            "language": language or "auto",
            "duration": round(duration, 3),
        }

    def chat_complete(self, request: ChatCompletionRequest) -> dict[str, Any]:
        self.ensure_ready()

        if request.stream:
            raise HTTPException(
                status_code=400, detail="stream=true is not supported",
            )

        prompt_text = self._render_chat_prompt(request.messages)
        content = self._generate_from_text(
            prompt_text,
            temperature=request.temperature or 0.0,
            max_new_tokens=request.max_tokens or DEFAULT_MAX_NEW_TOKENS,
        ).strip()
        created = int(time.time())

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

    @staticmethod
    def _try_quanto_quantization(
        mode: str, load_kwargs: dict[str, Any],
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

    def _build_transcription_prompt(
        self, language: str | None, prompt: str | None,
    ) -> str:
        parts = [
            "Transcribe the provided audio accurately.",
            "Return only the transcription text.",
        ]
        if language:
            parts.append(f"Expected language: {language}.")
        if prompt:
            parts.append(f"Additional context: {prompt}")
        return " ".join(parts)

    def _render_chat_prompt(self, messages: list[ChatMessage]) -> str:
        assert self.processor is not None
        normalized = [
            {
                "role": msg.role,
                "content": [
                    {"type": "text", "text": self._flatten_content(msg.content)},
                ],
            }
            for msg in messages
        ]
        return self.processor.apply_chat_template(
            normalized,
            tokenize=False,
            add_generation_prompt=True,
            enable_thinking=False,
        )

    def _flatten_content(self, content: Any) -> str:
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

    def _generate_from_text(
        self,
        prompt_text: str,
        *,
        temperature: float,
        max_new_tokens: int,
    ) -> str:
        assert self.processor is not None
        assert self.model is not None

        inputs = self.processor(text=prompt_text, return_tensors="pt").to(
            self.model.device,
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
        return self.processor.parse_response(response)

    def _generate_from_audio(self, prompt_text: str, audio_path: str) -> str:
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
        ).to(self.model.device)
        input_len = inputs["input_ids"].shape[-1]
        outputs = self.model.generate(
            **inputs,
            max_new_tokens=DEFAULT_MAX_NEW_TOKENS,
            do_sample=False,
        )
        response = self.processor.decode(
            outputs[0][input_len:], skip_special_tokens=False,
        )
        return self.processor.parse_response(response)


def create_app(runtime: LocalModelRuntime) -> FastAPI:
    @asynccontextmanager
    async def lifespan(_: FastAPI):
        thread = threading.Thread(target=runtime.load, daemon=True)
        thread.start()
        yield

    app = FastAPI(
        title="ViberWhisper Local Gemma Service",
        lifespan=lifespan,
    )

    @app.get("/health")
    def health() -> JSONResponse:
        status_code, payload = runtime.health_payload()
        return JSONResponse(status_code=status_code, content=payload)

    @app.post("/v1/audio/transcriptions")
    async def audio_transcriptions(
        file: Annotated[UploadFile, File()],
        language: Annotated[str | None, Form()] = None,
        prompt: Annotated[str | None, Form()] = None,
        model: Annotated[str | None, Form()] = None,
        temperature: Annotated[str | None, Form()] = None,
        response_format: Annotated[str | None, Form()] = None,
    ) -> dict[str, Any]:
        del model, temperature, response_format
        wav_bytes = await file.read()
        return await asyncio.to_thread(
            runtime.transcribe_audio, wav_bytes, language, prompt,
        )

    @app.post("/v1/chat/completions")
    async def chat_completions(request: ChatCompletionRequest) -> dict[str, Any]:
        return await asyncio.to_thread(runtime.chat_complete, request)

    return app


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--port", type=int, default=17265)
    parser.add_argument(
        "--quantization", choices=["int4", "int8", "bf16"], default="bf16",
    )
    return parser.parse_args()


def main() -> None:
    import uvicorn

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    args = parse_args()
    runtime = LocalModelRuntime(args.model_dir, args.quantization)
    app = create_app(runtime)
    uvicorn.run(app, host="127.0.0.1", port=args.port, log_level="info")


if __name__ == "__main__":
    main()
