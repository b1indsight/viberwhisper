use std::fs;
use std::path::PathBuf;

use super::{config_context, load_config};
use crate::core::cli::{
    PromptLabCommand, PromptLabDatasetCommand, PromptLabSampleCommand, PromptLabSampleStatus,
};
use crate::core::config::EnvironmentSecretSource;
use crate::prompt_lab::{DatasetStore, ProperNounAnnotation, ReferenceStatus};
use crate::runtime_config::{self, ProfileSelection};

pub(super) fn handle(action: PromptLabCommand) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        PromptLabCommand::Record { dataset } => record(dataset),
        PromptLabCommand::Sample { action } => sample(action),
        PromptLabCommand::Dataset { action } => dataset(action),
    }
}

fn record(root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let dataset = DatasetStore::open_or_create(root)?;
    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    let config = runtime_config::resolve_listener(
        &document,
        &EnvironmentSecretSource,
        ProfileSelection::Configured,
        &config_dir,
        &home_dir,
    )?;
    super::listener::run_capture(config, dataset)
}

fn sample(action: PromptLabSampleCommand) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        PromptLabSampleCommand::List { dataset, status } => {
            let store = DatasetStore::open_or_create(dataset)?;
            let status = status.map(|status| match status {
                PromptLabSampleStatus::Pending => ReferenceStatus::Pending,
                PromptLabSampleStatus::Ready => ReferenceStatus::Ready,
            });
            for sample in store.list_samples(status)? {
                let status = match sample.reference.status {
                    ReferenceStatus::Pending => "pending",
                    ReferenceStatus::Ready => "ready",
                };
                println!("{}\t{}\t{}", sample.id, status, sample.audio.path);
            }
            Ok(())
        }
        PromptLabSampleCommand::Show { dataset, id } => {
            let store = DatasetStore::open_or_create(dataset)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.load_sample(&id)?)?
            );
            Ok(())
        }
        PromptLabSampleCommand::Correct {
            dataset,
            id,
            reference,
            reference_file,
            proper_nouns_file,
        } => {
            let reference = match (reference, reference_file) {
                (Some(reference), None) => reference,
                (None, Some(path)) => fs::read_to_string(path)?,
                _ => unreachable!("clap enforces exactly one reference source"),
            };
            let proper_nouns = match proper_nouns_file {
                Some(path) => {
                    serde_json::from_slice::<Vec<ProperNounAnnotation>>(&fs::read(path)?)?
                }
                None => Vec::new(),
            };
            let store = DatasetStore::open_or_create(dataset)?;
            let sample = store.correct_sample(&id, &reference, proper_nouns)?;
            println!("{}", serde_json::to_string_pretty(&sample)?);
            Ok(())
        }
    }
}

fn dataset(action: PromptLabDatasetCommand) -> Result<(), Box<dyn std::error::Error>> {
    match action {
        PromptLabDatasetCommand::Validate { dataset } => {
            let store = DatasetStore::open_or_create(dataset)?;
            let report = store.validate()?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.issues.is_empty() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "dataset validation found {} issue(s)",
                    report.issues.len()
                ))
                .into())
            }
        }
    }
}
