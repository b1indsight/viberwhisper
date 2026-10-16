mod audio;
mod core;
mod dictation;
mod history;
mod input;
mod platform;
mod postprocess;
mod prompt_lab;
mod session;
mod text;
mod transcriber;
mod ui;

pub use core::{run, run_desktop};

#[cfg(feature = "integration")]
pub use ui::preview_demo::run_preview_demo;
