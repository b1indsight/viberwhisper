use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// ViberWhisper - 语音转文字输入工具
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
    /// 配置管理（查看、读取、修改配置项）
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// 本地 Gemma 推理服务管理
    Local {
        #[command(subcommand)]
        action: LocalCommand,
    },
    /// 转换音频文件为文字
    Convert {
        /// 输入的 WAV 文件路径
        input: String,
        /// 可选：输出文件路径（默认打印到 stdout）
        #[arg(short, long)]
        output: Option<String>,
    },
    /// STT prompt 数据集采集、校对与回归
    PromptLab {
        #[command(subcommand)]
        action: PromptLabCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabCommand {
    /// 使用现有托盘和热键录制测试样本
    Record {
        /// 数据集根目录（同一时间只能由一个进程使用）
        #[arg(long)]
        dataset: PathBuf,
    },
    /// 查看或修正录音样本
    Sample {
        #[command(subcommand)]
        action: PromptLabSampleCommand,
    },
    /// 数据集级操作
    Dataset {
        #[command(subcommand)]
        action: PromptLabDatasetCommand,
    },
    /// 使用候选 STT prompt 对全部 ready 样本执行回归
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
    /// 将编码代理的逐样本语义复核写入规范 JSON 报告
    Report {
        #[command(subcommand)]
        action: PromptLabReportCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabReportCommand {
    /// 验证并应用完整的代理复核 JSON
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
    /// 列出样本
    List {
        #[arg(long)]
        dataset: PathBuf,
        #[arg(long)]
        status: Option<PromptLabSampleStatus>,
    },
    /// 以 JSON 显示一个样本
    Show {
        #[arg(long)]
        dataset: PathBuf,
        id: String,
    },
    /// 写入人工标准文本和专有名词标注
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
        /// JSON 格式的专有名词标注数组；省略表示没有专有名词
        #[arg(long)]
        proper_nouns_file: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PromptLabDatasetCommand {
    /// 校验 manifest、sidecar、WAV 和摘要
    Validate {
        #[arg(long)]
        dataset: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 显示当前平台使用的配置文件路径
    Path,
    /// 检查当前 profile 的运行配置
    Check,
    /// 列出所有配置项及当前值
    List,
    /// 读取指定配置项的值
    Get {
        /// Canonical dotted key（如 input.hold_hotkey）
        key: String,
    },
    /// 设置指定配置项的值
    Set {
        /// Canonical dotted key
        key: String,
        /// 新值
        value: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum LocalCommand {
    /// 安装本地推理服务依赖与模型
    Install,
    /// 启动本地推理服务并运行主监听循环
    Start,
    /// 停止本地推理服务
    Stop,
    /// 查看本地推理服务状态
    Status,
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
    fn test_cli_local_start() {
        let cli = Cli::try_parse_from(["viberwhisper", "local", "start"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Local {
                action: LocalCommand::Start
            })
        ));
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
