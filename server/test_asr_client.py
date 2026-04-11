from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Send a WAV file to the local Gemma ASR endpoint and print the result.",
    )
    parser.add_argument("wav_path", help="Path to a WAV file")
    parser.add_argument(
        "--url",
        default="http://127.0.0.1:17265/v1/audio/transcriptions",
        help="ASR endpoint URL",
    )
    parser.add_argument("--language", help="Optional language hint")
    parser.add_argument("--prompt", help="Optional extra transcription prompt")
    parser.add_argument(
        "--expect",
        help="Optional substring expected to appear in the transcription output",
    )
    return parser.parse_args()


def build_multipart_body(
    wav_path: Path,
    *,
    language: str | None,
    prompt: str | None,
) -> tuple[bytes, str]:
    boundary = "----ViberWhisperASRBoundary"
    crlf = b"\r\n"
    parts: list[bytes] = []

    def add_field(name: str, value: str) -> None:
        parts.extend(
            [
                f"--{boundary}".encode(),
                f'Content-Disposition: form-data; name="{name}"'.encode(),
                b"",
                value.encode("utf-8"),
            ]
        )

    add_field("model", "gemma-4-E2B-it")
    if language:
        add_field("language", language)
    if prompt:
        add_field("prompt", prompt)

    parts.extend(
        [
            f"--{boundary}".encode(),
            (
                f'Content-Disposition: form-data; name="file"; filename="{wav_path.name}"'
            ).encode(),
            b"Content-Type: audio/wav",
            b"",
            wav_path.read_bytes(),
            f"--{boundary}--".encode(),
            b"",
        ]
    )

    return crlf.join(parts), boundary


def main() -> int:
    args = parse_args()
    wav_path = Path(args.wav_path)
    if not wav_path.is_file():
        print(f"missing wav file: {wav_path}", file=sys.stderr)
        return 2

    body, boundary = build_multipart_body(
        wav_path,
        language=args.language,
        prompt=args.prompt,
    )
    request = urllib.request.Request(
        args.url,
        data=body,
        method="POST",
        headers={
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Accept": "application/json",
        },
    )

    try:
        with urllib.request.urlopen(request) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        print(f"HTTP {error.code}: {body}", file=sys.stderr)
        return 1
    except urllib.error.URLError as error:
        print(f"request failed: {error}", file=sys.stderr)
        return 1

    text = payload.get("text", "")
    print(json.dumps(payload, ensure_ascii=False, indent=2))

    if args.expect and args.expect not in text:
        print(
            f"expected substring not found: {args.expect!r} not in transcription",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
