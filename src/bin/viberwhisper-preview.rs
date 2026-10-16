fn main() -> anyhow::Result<()> {
    viberwhisper::run_preview_demo(std::env::args().any(|arg| arg == "--smoke"))
}
