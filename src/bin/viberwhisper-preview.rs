fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut smoke = false;
    let mut artifacts = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke" => smoke = true,
            "--artifacts" => {
                artifacts =
                    Some(std::path::PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--artifacts requires a directory")
                    })?))
            }
            _ => anyhow::bail!("unknown preview argument: {arg}"),
        }
    }
    anyhow::ensure!(
        !smoke || artifacts.is_none(),
        "Use --smoke or --artifacts separately"
    );
    viberwhisper::run_preview_demo(smoke, artifacts)
}
