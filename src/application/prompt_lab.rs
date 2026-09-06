use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use super::{config_context, load_config};
use crate::core::cli::{
    PromptLabCommand, PromptLabDatasetCommand, PromptLabReportCommand, PromptLabSampleCommand,
    PromptLabSampleStatus,
};
use crate::core::config::EnvironmentSecretSource;
use crate::prompt_lab::{
    DatasetStore, EvaluationReport, EvaluationRequest, ProperNounAnnotation, ReferenceStatus,
    RunStatus, SttSnapshot, Thresholds, apply_review, evaluate,
};
use crate::runtime_config::{self, ProfileSelection};

pub(super) fn handle(action: PromptLabCommand) -> Result<()> {
    match action {
        PromptLabCommand::Record { dataset } => record(dataset),
        PromptLabCommand::Sample { action } => sample(action),
        PromptLabCommand::Dataset { action } => dataset(action),
        PromptLabCommand::Evaluate {
            dataset,
            prompt_file,
            no_prompt,
            max_wer_percent,
            min_llm_score,
            min_proper_noun_percent,
            compare_to,
            output,
        } => evaluate_command(EvaluateCommand {
            dataset,
            prompt_file,
            no_prompt,
            thresholds: Thresholds {
                max_wer_percent,
                min_llm_score,
                min_proper_noun_percent,
            },
            compare_to,
            output,
        }),
        PromptLabCommand::Report { action } => report(action),
    }
}

struct EvaluateCommand {
    dataset: PathBuf,
    prompt_file: Option<PathBuf>,
    no_prompt: bool,
    thresholds: Thresholds,
    compare_to: Option<PathBuf>,
    output: Option<PathBuf>,
}

fn evaluate_command(command: EvaluateCommand) -> Result<()> {
    use crate::transcriber::ApiTranscriber;

    let candidate_from_file = command
        .prompt_file
        .as_ref()
        .map(fs::read_to_string)
        .transpose()?;
    if candidate_from_file
        .as_deref()
        .is_some_and(|prompt| prompt.trim().is_empty())
    {
        return Err(std::io::Error::other("candidate prompt file must not be empty").into());
    }
    let dataset = DatasetStore::open_or_create(command.dataset)?;
    let prior = command
        .compare_to
        .as_deref()
        .map(EvaluationReport::read)
        .transpose()?;
    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    let mut config = runtime_config::resolve_convert(
        &document,
        &EnvironmentSecretSource,
        &config_dir,
        &home_dir,
    )?;
    let prompt = if command.no_prompt {
        None
    } else if let Some(prompt) = candidate_from_file {
        Some(prompt)
    } else {
        config.backend.transcriber.metadata().prompt
    };
    config.backend.transcriber = config.backend.transcriber.with_prompt(prompt);
    let stt = SttSnapshot::from(config.backend.transcriber.metadata());
    let local_manager = super::start_local_backend(&mut config.backend)?;
    let _local_manager = super::LocalServiceGuard::new(local_manager);
    let transcriber = ApiTranscriber::new(config.backend.transcriber)?;
    let outcome = evaluate(
        &dataset,
        &transcriber,
        EvaluationRequest {
            stt,
            language: config.language,
            max_chunk_duration_secs: config.max_chunk_duration_secs,
            max_chunk_size_bytes: config.max_chunk_size_bytes,
            thresholds: command.thresholds,
            output: command.output,
            compare_to: prior,
        },
    )?;
    println!("report: {}", outcome.path.display());
    println!("status: {}", outcome.report.status.as_str());
    println!("wer_percent: {}", outcome.report.aggregates.wer.wer_percent);
    println!(
        "proper_noun_percent: {}",
        outcome.report.aggregates.proper_nouns.accuracy_percent
    );
    if outcome.report.status == RunStatus::Incomplete {
        Err(std::io::Error::other(format!(
            "evaluation completed with {} sample failure(s)",
            outcome.report.failures.len()
        ))
        .into())
    } else {
        Ok(())
    }
}

fn report(action: PromptLabReportCommand) -> Result<()> {
    match action {
        PromptLabReportCommand::ApplyReview { report, review } => {
            let report = apply_review(&report, &review)?;
            let llm = report
                .aggregates
                .llm
                .as_ref()
                .expect("applied review creates LLM aggregate");
            println!("status: {}", report.status.as_str());
            println!("llm_score: {}", llm.mean_score);
            println!(
                "meets_targets: {}",
                report
                    .meets_targets
                    .expect("complete report has gate result")
            );
            Ok(())
        }
    }
}

fn record(root: PathBuf) -> Result<()> {
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

fn sample(action: PromptLabSampleCommand) -> Result<()> {
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

fn dataset(action: PromptLabDatasetCommand) -> Result<()> {
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
