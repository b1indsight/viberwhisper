from __future__ import annotations

import argparse
import asyncio
import logging
import threading
import time
import uuid
from contextlib import asynccontextmanager
from typing import Annotated, Any

from fastapi import FastAPI, File, Form, HTTPException, Request, UploadFile
from fastapi.responses import JSONResponse

from server.chat import chat_complete
from server.models import ChatCompletionRequest
from server.runtime import LocalModelRuntime, MODEL_NAME
from server.transcription import transcribe_audio

LOGGER = logging.getLogger("viberwhisper.local_server")


def create_app(runtime: LocalModelRuntime) -> FastAPI:
    @asynccontextmanager
    async def lifespan(_: FastAPI):
        LOGGER.info("Starting background model load thread")
        thread = threading.Thread(target=runtime.load, daemon=True)
        thread.start()
        yield
        LOGGER.info("Shutting down local Gemma service")

    app = FastAPI(
        title="ViberWhisper Local Gemma Service",
        lifespan=lifespan,
    )

    @app.middleware("http")
    async def log_requests(request: Request, call_next):
        request_id = uuid.uuid4().hex[:8]
        started_at = time.perf_counter()
        LOGGER.info(
            "Request started id=%s method=%s path=%s client=%s",
            request_id,
            request.method,
            request.url.path,
            request.client.host if request.client else "unknown",
        )
        try:
            response = await call_next(request)
        except Exception:
            elapsed = time.perf_counter() - started_at
            LOGGER.exception(
                "Request failed id=%s method=%s path=%s duration=%.3fs",
                request_id,
                request.method,
                request.url.path,
                elapsed,
            )
            raise

        elapsed = time.perf_counter() - started_at
        LOGGER.info(
            "Request completed id=%s method=%s path=%s status=%s duration=%.3fs",
            request_id,
            request.method,
            request.url.path,
            response.status_code,
            elapsed,
        )
        return response

    @app.exception_handler(HTTPException)
    async def http_exception_handler(_: Request, exc: HTTPException) -> JSONResponse:
        LOGGER.warning(
            "HTTP exception status=%s detail=%s",
            exc.status_code,
            exc.detail,
        )
        return JSONResponse(status_code=exc.status_code, content={"detail": exc.detail})

    @app.exception_handler(Exception)
    async def unhandled_exception_handler(_: Request, exc: Exception) -> JSONResponse:
        LOGGER.exception("Unhandled server exception")
        return JSONResponse(
            status_code=500,
            content={"detail": str(exc)},
        )

    @app.get("/health")
    def health() -> JSONResponse:
        status_code, payload = runtime.health_payload()
        LOGGER.info("Health check status=%s payload=%s", status_code, payload)
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
        LOGGER.info(
            "Audio transcription request filename=%s content_type=%s bytes=%s language=%s",
            file.filename,
            file.content_type,
            len(wav_bytes),
            language or "auto",
        )
        return await asyncio.to_thread(
            transcribe_audio, runtime, wav_bytes, language, prompt,
        )

    @app.post("/v1/chat/completions")
    async def chat_completions(request: ChatCompletionRequest) -> dict[str, Any]:
        LOGGER.info(
            "Chat completion request model=%s messages=%s stream=%s",
            request.model or MODEL_NAME,
            len(request.messages),
            request.stream,
        )
        return await asyncio.to_thread(chat_complete, runtime, request)

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
    LOGGER.info(
        "Launching local Gemma service model_dir=%s port=%s quantization=%s",
        args.model_dir,
        args.port,
        args.quantization,
    )
    runtime = LocalModelRuntime(args.model_dir, args.quantization)
    app = create_app(runtime)
    uvicorn.run(app, host="127.0.0.1", port=args.port, log_level="info")


if __name__ == "__main__":
    main()
