pub mod installer;
pub mod service;

pub use installer::{
    PythonRuntime, dependencies_installed, detect_python_runtime, download_model,
    install_requirements, model_weights_present, setup_venv, verify_install,
};
pub use service::LocalServiceManager;
