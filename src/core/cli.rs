use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// ViberWhisper - voice-to-text typing utility
#[derive(Parser, Debug)]
#[command(
    name = "viberwhisper",
    version,
    about = "语音转文字输入工具，按住热键录音，释放后自动输入识别文字"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run the first-run setup wizard
    Setup,
    /// Manage configuration (list, read, and update settings)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Transcribe an audio file
    Convert {
        /// Path to the input WAV file
        input: String,
        /// Optional output path (prints to stdout by default)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Capture, correct, and evaluate STT prompt datasets
    PromptLab {
        #[command(subcommand)]
        action: PromptLabCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabCommand {
    /// Record test samples using the existing tray and hotkeys
    Record {
        /// Dataset root (only one process may access it at a time)
        #[arg(long)]
        dataset: PathBuf,
    },
    /// Inspect or correct recorded samples
    Sample {
        #[command(subcommand)]
        action: PromptLabSampleCommand,
    },
    /// Run dataset-wide operations
    Dataset {
        #[command(subcommand)]
        action: PromptLabDatasetCommand,
    },
    /// Evaluate all ready samples with a candidate STT prompt
    Evaluate {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long, conflicts_with = "no_prompt")]
        prompt_file: Option<PathBuf>,
        #[arg(long, conflicts_with = "prompt_file")]
        no_prompt: bool,
        #[arg(long)]
        max_wer_percent: f64,
        #[arg(long)]
        min_llm_score: f64,
        #[arg(long)]
        min_proper_noun_percent: f64,
        #[arg(long)]
        compare_to: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Write per-sample coding-agent semantic reviews to a canonical JSON report
    Report {
        #[command(subcommand)]
        action: PromptLabReportCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabReportCommand {
    /// Validate and apply a complete agent-review JSON document
    ApplyReview {
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        review: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PromptLabSampleStatus {
    Pending,
    Ready,
}

#[derive(Subcommand, Debug)]
pub enum PromptLabSampleCommand {
    /// List samples
    List {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long)]
        status: Option<PromptLabSampleStatus>,
    },
    /// Show a sample as JSON
    Show {
        #[arg(long)]
        dataset: PathBuf,
        id: String,
    },
    /// Write the human reference and proper-noun annotations
    Correct {
        #[arg(long)]
        dataset: PathBuf,
        id: String,
        #[arg(
            long,
            conflicts_with = "reference_file",
            required_unless_present = "reference_file"
        )]
        reference: Option<String>,
        #[arg(
            long,
            conflicts_with = "reference",
            required_unless_present = "reference"
        )]
        reference_file: Option<PathBuf>,
        /// Path to a JSON array of proper-noun annotations; omit when there are none
        #[arg(long)]
        proper_nouns_file: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabDatasetCommand {
    /// Validate the manifest, sidecars, WAV files, and summary
    Validate {
        #[arg(long)]
        dataset: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Show the configuration file path for the current platform
    Path,
    /// Validate the current profile's runtime configuration
    Check,
    /// List all configuration keys and their current values
    List,
    /// Read a configuration value
    Get {
        /// Canonical dotted key (for example, input.hold_hotkey)
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Canonical dotted key
        key: String,
        /// New value
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_no_subcommand() {
        let cli = Cli::try_parse_from(["viberwhisper"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_cli_setup() {
        let cli = Cli::try_parse_from(["viberwhisper", "setup"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Setup)));
    }

    #[test]
    fn test_cli_config_set() {
        let cli = Cli::try_parse_from(["viberwhisper", "config", "set", "input.hold_hotkey", "F9"])
            .unwrap();
        if let Some(Commands::Config {
            action: ConfigAction::Set { key, value },
        }) = cli.command
        {
            assert_eq!(key, "input.hold_hotkey");
            assert_eq!(value, "F9");
        } else {
            panic!("Expected config set command");
        }
    }

    #[test]
    fn test_cli_convert_optional_output() {
        let cases: &[(&[&str], Option<&str>)] = &[
            (&["viberwhisper", "convert", "test.wav"], None),
            (
                &["viberwhisper", "convert", "test.wav", "--output", "out.txt"],
                Some("out.txt"),
            ),
        ];

        for &(args, expected_output) in cases {
            let cli = Cli::try_parse_from(args.iter().copied()).unwrap();
            let Some(Commands::Convert { input, output }) = cli.command else {
                panic!("Expected convert command for {args:?}");
            };
            assert_eq!(input, "test.wav");
            assert_eq!(output.as_deref(), expected_output, "args: {args:?}");
        }
    }

    #[test]
    fn rejects_removed_local_command() {
        assert!(Cli::try_parse_from(["viberwhisper", "local", "start"]).is_err());
    }

    #[test]
    fn parses_prompt_lab_sample_correction_inputs() {
        let cli = Cli::try_parse_from([
            "viberwhisper",
            "prompt-lab",
            "sample",
            "correct",
            "--dataset",
            "/tmp/lab",
            "sample-1-1",
            "--reference-file",
            "expected.txt",
            "--proper-nouns-file",
            "terms.json",
        ])
        .unwrap();

        let Some(Commands::PromptLab {
            action:
                PromptLabCommand::Sample {
                    action:
                        PromptLabSampleCommand::Correct {
                            dataset,
                            id,
                            reference,
                            reference_file,
                            proper_nouns_file,
                        },
                },
        }) = cli.command
        else {
            panic!("expected prompt-lab sample correct");
        };
        assert_eq!(dataset, std::path::PathBuf::from("/tmp/lab"));
        assert_eq!(id, "sample-1-1");
        assert!(reference.is_none());
        assert_eq!(
            reference_file.as_deref(),
            Some(std::path::Path::new("expected.txt"))
        );
        assert_eq!(
            proper_nouns_file.as_deref(),
            Some(std::path::Path::new("terms.json"))
        );
    }

    #[test]
    fn correction_rejects_two_reference_sources() {
        let error = Cli::try_parse_from([
            "viberwhisper",
            "prompt-lab",
            "sample",
            "correct",
            "--dataset",
            "/tmp/lab",
            "sample-1-1",
            "--reference",
            "inline",
            "--reference-file",
            "expected.txt",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_prompt_lab_evaluation_thresholds_and_paths() {
        let cli = Cli::try_parse_from([
            "viberwhisper",
            "prompt-lab",
            "evaluate",
            "--dataset",
            "/tmp/lab",
            "--prompt-file",
            "candidate.txt",
            "--max-wer-percent",
            "8",
            "--min-llm-score",
            "95",
            "--min-proper-noun-percent",
            "98",
            "--compare-to",
            "baseline.json",
            "--output",
            "candidate.json",
        ])
        .unwrap();

        let Some(Commands::PromptLab {
            action:
                PromptLabCommand::Evaluate {
                    dataset,
                    prompt_file,
                    no_prompt,
                    max_wer_percent,
                    min_llm_score,
                    min_proper_noun_percent,
                    compare_to,
                    output,
                },
        }) = cli.command
        else {
            panic!("expected prompt-lab evaluate");
        };
        assert_eq!(dataset, PathBuf::from("/tmp/lab"));
        assert_eq!(prompt_file, Some(PathBuf::from("candidate.txt")));
        assert!(!no_prompt);
        assert_eq!(max_wer_percent, 8.0);
        assert_eq!(min_llm_score, 95.0);
        assert_eq!(min_proper_noun_percent, 98.0);
        assert_eq!(compare_to, Some(PathBuf::from("baseline.json")));
        assert_eq!(output, Some(PathBuf::from("candidate.json")));
    }

    #[test]
    fn evaluation_rejects_prompt_file_with_no_prompt() {
        let error = Cli::try_parse_from([
            "viberwhisper",
            "prompt-lab",
            "evaluate",
            "--dataset",
            "/tmp/lab",
            "--prompt-file",
            "candidate.txt",
            "--no-prompt",
            "--max-wer-percent",
            "8",
            "--min-llm-score",
            "95",
            "--min-proper-noun-percent",
            "98",
        ])
        .unwrap_err();

        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parses_prompt_lab_agent_review_application() {
        let cli = Cli::try_parse_from([
            "viberwhisper",
            "prompt-lab",
            "report",
            "apply-review",
            "--report",
            "run.json",
            "--review",
            "review.json",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Some(Commands::PromptLab {
                action: PromptLabCommand::Report {
                    action: PromptLabReportCommand::ApplyReview { .. }
                }
            })
        ));
    }
}
