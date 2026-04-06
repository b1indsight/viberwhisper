use clap::{Parser, Subcommand};

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
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 列出所有配置项及当前值
    List,
    /// 读取指定配置项的值
    Get {
        /// 配置项名称（如 hotkey, model, language）
        key: String,
    },
    /// 设置指定配置项的值
    Set {
        /// 配置项名称
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
    fn test_cli_config_list() {
        let cli = Cli::try_parse_from(["viberwhisper", "config", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Config {
                action: ConfigAction::List
            })
        ));
    }

    #[test]
    fn test_cli_config_get() {
        let cli = Cli::try_parse_from(["viberwhisper", "config", "get", "hotkey"]).unwrap();
        if let Some(Commands::Config {
            action: ConfigAction::Get { key },
        }) = cli.command
        {
            assert_eq!(key, "hotkey");
        } else {
            panic!("Expected config get command");
        }
    }

    #[test]
    fn test_cli_config_set() {
        let cli = Cli::try_parse_from(["viberwhisper", "config", "set", "hotkey", "F9"]).unwrap();
        if let Some(Commands::Config {
            action: ConfigAction::Set { key, value },
        }) = cli.command
        {
            assert_eq!(key, "hotkey");
            assert_eq!(value, "F9");
        } else {
            panic!("Expected config set command");
        }
    }

    #[test]
    fn test_cli_convert_basic() {
        let cli = Cli::try_parse_from(["viberwhisper", "convert", "test.wav"]).unwrap();
        if let Some(Commands::Convert { input, output }) = cli.command {
            assert_eq!(input, "test.wav");
            assert_eq!(output, None);
        } else {
            panic!("Expected convert command");
        }
    }

    #[test]
    fn test_cli_convert_with_output() {
        let cli =
            Cli::try_parse_from(["viberwhisper", "convert", "test.wav", "--output", "out.txt"])
                .unwrap();
        if let Some(Commands::Convert { input, output }) = cli.command {
            assert_eq!(input, "test.wav");
            assert_eq!(output, Some("out.txt".to_string()));
        } else {
            panic!("Expected convert command");
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
    fn test_cli_local_status() {
        let cli = Cli::try_parse_from(["viberwhisper", "local", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Local {
                action: LocalCommand::Status
            })
        ));
    }
}
