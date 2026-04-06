use std::path::{Path, PathBuf};
use std::process::Command;

const DEPENDENCY_CHECK_SCRIPT: &str =
    "import accelerate, fastapi, huggingface_hub, soundfile, torch, transformers, uvicorn";

/// Creates a Python virtual environment for the local service.
pub fn setup_venv(venv_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let python = venv_python_path(venv_dir);
    if python.exists() {
        return Ok(());
    }

    if let Some(parent) = venv_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    run_command(
        find_python()?,
        ["-m", "venv", &venv_dir.to_string_lossy()],
        None,
    )
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

    run_command(
        python,
        [
            "-m",
            "huggingface_hub.commands.huggingface_cli",
            "download",
            "google/gemma-4-E4B-it",
            "--local-dir",
            &model_dir.to_string_lossy(),
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

fn find_python() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = std::env::var("PYTHON")
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }

    #[cfg(target_os = "windows")]
    let candidates = ["python", "py"];
    #[cfg(not(target_os = "windows"))]
    let candidates = ["python3", "python"];

    for candidate in candidates {
        let status = Command::new(candidate)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return Ok(PathBuf::from(candidate));
        }
    }

    Err("python executable not found".into())
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
            "soundfile",
            "torch",
            "transformers",
            "uvicorn",
        ] {
            assert!(
                DEPENDENCY_CHECK_SCRIPT.contains(package),
                "missing package in dependency check: {package}"
            );
        }
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
