use std::path::{Path, PathBuf};
use std::process::Command;

const DEPENDENCY_CHECK_SCRIPT: &str = "import accelerate, fastapi, huggingface_hub, librosa, multipart, PIL, soundfile, torch, torchvision, transformers, uvicorn";
const MIN_PYTHON_MAJOR: u32 = 3;
const MIN_PYTHON_MINOR: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonRuntime {
    pub python: PathBuf,
    pub version: (u32, u32),
    pub uv: Option<PathBuf>,
}

/// Creates a Python virtual environment for the local service.
pub fn setup_venv(venv_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let python = venv_python_path(venv_dir);
    if python.exists() {
        return Ok(());
    }

    if let Some(parent) = venv_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let python = find_python()?;
    if let Some(uv) = find_uv() {
        return run_command(
            uv,
            [
                "venv",
                "--python",
                &python.to_string_lossy(),
                &venv_dir.to_string_lossy(),
            ],
            None,
        );
    }

    run_command(python, ["-m", "venv", &venv_dir.to_string_lossy()], None)
}

/// Installs Python requirements into the virtual environment.
pub fn install_requirements(
    venv_dir: &Path,
    reqs: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !reqs.exists() {
        return Err(format!("requirements file not found: {}", reqs.display()).into());
    }

    let python = venv_python_path(venv_dir);
    if !python.exists() {
        return Err(format!("virtualenv python not found: {}", python.display()).into());
    }

    if let Some(uv) = find_uv() {
        return run_command(
            uv,
            [
                "pip",
                "install",
                "--python",
                &python.to_string_lossy(),
                "-r",
                &reqs.to_string_lossy(),
            ],
            None,
        );
    }

    run_command(
        python,
        ["-m", "pip", "install", "-r", &reqs.to_string_lossy()],
        None,
    )
}

/// Downloads the local Gemma model into the model directory.
pub fn download_model(
    model_dir: &Path,
    hf_endpoint: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if model_weights_present(model_dir) {
        return Ok(());
    }

    std::fs::create_dir_all(model_dir)?;

    let venv_dir = model_dir
        .parent()
        .ok_or("model directory must have a parent directory for sibling venv lookup")?
        .join("venv");
    let python = venv_python_path(&venv_dir);
    if !python.exists() {
        return Err(format!(
            "virtualenv python not found for model download: {}",
            python.display()
        )
        .into());
    }

    let hf = venv_bin_path(&venv_dir, "hf");
    if hf.exists() {
        return run_command(
            hf,
            [
                "download",
                "google/gemma-4-E2B-it",
                "--local-dir",
                &model_dir.to_string_lossy(),
            ],
            Some(("HF_ENDPOINT", hf_endpoint)),
        );
    }

    run_command(
        python,
        [
            "-c",
            "from huggingface_hub import snapshot_download; snapshot_download(repo_id='google/gemma-4-E2B-it', local_dir=r'''__MODEL_DIR__''')"
                .replace("__MODEL_DIR__", &model_dir.to_string_lossy())
                .as_str(),
        ],
        Some(("HF_ENDPOINT", hf_endpoint)),
    )
}

/// Verifies the presence of the virtual environment and model files.
pub fn verify_install(venv_dir: &Path, model_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let python = venv_python_path(venv_dir);
    if !python.exists() {
        return Err(format!("virtualenv python not found: {}", python.display()).into());
    }

    if !model_dir.is_dir() {
        return Err(format!("model directory not found: {}", model_dir.display()).into());
    }

    let has_model_files = std::fs::read_dir(model_dir)?
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_type()
                .map(|file_type| file_type.is_file())
                .unwrap_or(false)
        });

    if !has_model_files {
        return Err(format!("model directory is empty: {}", model_dir.display()).into());
    }

    Ok(())
}

/// Returns true when the key Python packages required by the server are importable.
pub fn dependencies_installed(venv_dir: &Path) -> bool {
    let python = venv_python_path(venv_dir);
    if !python.exists() {
        return false;
    }
    Command::new(python)
        .args(["-c", DEPENDENCY_CHECK_SCRIPT])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Returns true when model config and at least one weight shard are present.
pub fn model_weights_present(model_dir: &Path) -> bool {
    if !model_dir.join("config.json").exists() {
        return false;
    }
    std::fs::read_dir(model_dir)
        .ok()
        .map(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".safetensors") || name.ends_with(".bin")
            })
        })
        .unwrap_or(false)
}

pub(crate) fn venv_python_path(venv_dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv_dir.join("Scripts").join("python.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv_dir.join("bin").join("python")
    }
}

fn venv_bin_path(venv_dir: &Path, executable: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv_dir.join("Scripts").join(format!("{executable}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv_dir.join("bin").join(executable)
    }
}

pub(crate) fn find_uv() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("UV")
        && !path.is_empty()
    {
        let uv = PathBuf::from(path);
        if command_succeeds(&uv, ["--version"]) {
            return Some(uv);
        }
    }

    let uv = PathBuf::from("uv");
    command_succeeds(&uv, ["--version"]).then_some(uv)
}

pub fn detect_python_runtime() -> Result<PythonRuntime, Box<dyn std::error::Error>> {
    let python = find_python()?;
    let version = python_version(&python)?;
    Ok(PythonRuntime {
        python,
        version,
        uv: find_uv(),
    })
}

fn find_python() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = std::env::var("PYTHON")
        && !path.is_empty()
    {
        let python = PathBuf::from(path);
        ensure_supported_python(&python)?;
        return Ok(python);
    }

    #[cfg(target_os = "windows")]
    let candidates = ["python", "py"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["python3", "python"];

    for candidate in candidates {
        let python = PathBuf::from(candidate);
        if ensure_supported_python(&python).is_ok() {
            return Ok(python);
        }
    }

    Err(format!(
        "python executable not found, or no supported version >= {}.{} is available",
        MIN_PYTHON_MAJOR, MIN_PYTHON_MINOR
    )
    .into())
}

fn ensure_supported_python(python: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (major, minor) = python_version(python)?;
    if is_supported_python_version(major, minor) {
        Ok(())
    } else {
        Err(format!(
            "Python {}.{} is not supported; require >= {}.{}",
            major, minor, MIN_PYTHON_MAJOR, MIN_PYTHON_MINOR
        )
        .into())
    }
}

fn python_version(python: &Path) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let output = Command::new(python)
        .args([
            "-c",
            "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
        ])
        .output()?;

    if !output.status.success() {
        return Err(format!("failed to query python version from {}", python.display()).into());
    }

    parse_python_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_python_version(version: &str) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    let version = version.trim();
    let mut parts = version.split('.');
    let major = parts
        .next()
        .ok_or("missing python major version")?
        .parse::<u32>()?;
    let minor = parts
        .next()
        .ok_or("missing python minor version")?
        .parse::<u32>()?;
    Ok((major, minor))
}

fn is_supported_python_version(major: u32, minor: u32) -> bool {
    major > MIN_PYTHON_MAJOR || (major == MIN_PYTHON_MAJOR && minor >= MIN_PYTHON_MINOR)
}

fn command_succeeds<I, S>(program: &Path, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run_command<I, S>(
    program: PathBuf,
    args: I,
    env: Option<(&str, &str)>,
) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    if let Some((key, value)) = env {
        command.env(key, value);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed with status {status}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_install_requires_existing_paths() {
        let result = verify_install(
            Path::new("/definitely/missing/venv"),
            Path::new("/definitely/missing/model"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_venv_python_path_uses_platform_layout() {
        #[cfg(target_os = "windows")]
        assert_eq!(
            venv_python_path(Path::new("venv")),
            PathBuf::from("venv/Scripts/python.exe")
        );

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            venv_python_path(Path::new("venv")),
            PathBuf::from("venv/bin/python")
        );
    }

    #[test]
    fn test_dependency_check_script_covers_required_runtime_packages() {
        for package in [
            "accelerate",
            "fastapi",
            "huggingface_hub",
            "librosa",
            "multipart",
            "PIL",
            "soundfile",
            "torch",
            "torchvision",
            "transformers",
            "uvicorn",
        ] {
            assert!(
                DEPENDENCY_CHECK_SCRIPT.contains(package),
                "missing package in dependency check: {package}"
            );
        }
    }

    #[test]
    fn test_parse_python_version() {
        assert_eq!(parse_python_version("3.10\n").unwrap(), (3, 10));
        assert_eq!(parse_python_version("3.12").unwrap(), (3, 12));
    }

    #[test]
    fn test_parse_python_version_rejects_invalid_input() {
        assert!(parse_python_version("3").is_err());
        assert!(parse_python_version("").is_err());
    }

    #[test]
    fn test_python_version_support_floor() {
        assert!(!is_supported_python_version(3, 9));
        assert!(is_supported_python_version(3, 10));
        assert!(is_supported_python_version(3, 11));
        assert!(is_supported_python_version(4, 0));
    }

    #[test]
    fn test_detect_python_runtime_reports_supported_version() {
        let runtime = detect_python_runtime().unwrap();
        assert!(is_supported_python_version(
            runtime.version.0,
            runtime.version.1
        ));
    }

    #[cfg(feature = "integration")]
    #[test]
    fn test_setup_venv_and_verify_install_integration() {
        let temp_dir = std::env::temp_dir().join(format!(
            "viberwhisper-local-installer-{}",
            std::process::id()
        ));
        let venv_dir = temp_dir.join("venv");
        let model_dir = temp_dir.join("model");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::write(model_dir.join("config.json"), "{}").unwrap();

        setup_venv(&venv_dir).unwrap();
        verify_install(&venv_dir, &model_dir).unwrap();

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
