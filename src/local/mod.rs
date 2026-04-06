pub mod installer;
pub mod service;

pub use installer::{
    dependencies_installed, download_model, install_requirements, model_weights_present,
    setup_venv, verify_install,
};
pub use service::LocalServiceManager;
