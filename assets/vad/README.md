# Bundled voice activity detection

`silero-v6.2.onnx` is the unmodified MIT-licensed Silero VAD v6.2 model:
https://github.com/snakers4/silero-vad/blob/v6.2/src/silero_vad/data/silero_vad.onnx

SHA-256: `1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3`.

The model is embedded with `include_bytes!`. CPU-only ONNX Runtime 1.20.0 is
statically linked by pinned `ort` / `ort-sys` 2.0.0-rc.9. On macOS their build script
obtains platform archives and verifies their hashes. Windows development archives
use a dynamic CRT; Windows releases instead run `scripts/build-windows-vad-runtime.ps1`
to build CPU-only ONNX Runtime at commit `c4fb724e810bb496165b9015c77f402727392933`
with `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`, preserving the static CRT contract.
No runtime
download, Python installation, model directory, DLL, or dylib is needed.

Keep both Rust bindings pinned together. rc.10's macOS archives require macOS 13.3;
rc.9's archives retain this application's macOS 11 deployment target. Updates must
check both macOS architectures, the Windows static CRT, and real model inference.

`THIRD-PARTY-NOTICES.txt` contains the Silero, ONNX Runtime 1.20.0, ort and rubato
licenses plus ONNX Runtime's third-party notices. It ships in the macOS app,
Windows MSI and portable archive.
