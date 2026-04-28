use clap::{Parser, Subcommand};

/// git-sc のサブコマンド
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// ~/.config/git-sc/config.toml に設定ファイルを生成
    Init {
        /// 確認なしで既存の設定ファイルを上書き
        #[arg(long)]
        force: bool,
    },
}

/// AI コーディングエージェントによるスマートコミットメッセージ生成ツール (Gemini CLI, Codex CLI, Claude Code)
#[derive(Parser, Debug)]
#[command(name = "git-sc")]
#[command(
    about = "AI-powered smart commit message generator using coding agents (Gemini CLI, Codex CLI, or Claude Code)"
)]
#[command(version)]
pub struct Cli {
    /// サブコマンド (例: init)
    #[command(subcommand)]
    pub command: Option<Command>,

    /// 確認プロンプトをスキップして直接コミット
    #[arg(short = 'y', long = "yes")]
    pub auto_confirm: bool,

    /// コミットせずに生成されたメッセージのみ表示
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// 未ステージの変更を含む全変更をステージしてコミット
    #[arg(short = 'a', long = "all")]
    pub stage_all: bool,

    /// 直前のコミットメッセージをAI生成で書き換え
    #[arg(long = "amend", conflicts_with_all = ["squash", "reword", "generate_for"])]
    pub amend: bool,

    /// ブランチ内の全コミットを1つにsquash（ベースブランチを指定）
    #[arg(long = "squash", value_name = "BASE", conflicts_with_all = ["amend", "reword", "generate_for"])]
    pub squash: Option<String>,

    /// 指定コミットハッシュのメッセージを再生成（git rebase使用）
    #[arg(long = "reword", value_name = "HASH", conflicts_with_all = ["amend", "squash", "generate_for"])]
    pub reword: Option<String>,

    /// 指定コミットハッシュのdiffからメッセージ生成（出力のみ、複数指定可）
    #[arg(short = 'g', long = "generate-for", value_name = "HASH", num_args = 1.., conflicts_with_all = ["amend", "squash", "reword"])]
    pub generate_for: Option<Vec<String>>,

    /// 本文（body）付きのコミットメッセージを生成
    #[arg(short = 'b', long = "body")]
    pub with_body: bool,

    /// コミットメッセージの言語（設定ファイルより優先）
    #[arg(short = 'l', long = "lang")]
    pub language: Option<String>,

    /// デバッグモード（AIに送信するプロンプトを表示）
    #[arg(short = 'd', long = "debug")]
    pub debug: bool,

    /// 進捗メッセージを抑制（スクリプト/hooks向け）
    #[arg(short = 'q', long = "quiet")]
    pub quiet: bool,

    /// 使用するAIプロバイダーを指定 (例: gemini, codex, claude, opencode, apple-intelligence)
    #[arg(short = 'p', long = "provider")]
    pub provider: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // CLI 引数パースのテスト
    // ============================================================

    #[test]
    fn test_cli_default_values() {
        let cli = Cli::parse_from(["git-sc"]);
        assert!(!cli.auto_confirm);
        assert!(!cli.dry_run);
        assert!(!cli.stage_all);
        assert!(!cli.amend);
        assert!(cli.squash.is_none());
        assert!(cli.reword.is_none());
        assert!(cli.generate_for.is_none());
        assert!(!cli.with_body);
        assert!(cli.language.is_none());
        assert!(!cli.debug);
        assert!(!cli.quiet);
        assert!(cli.provider.is_none());
    }

    #[test]
    fn test_cli_auto_confirm_short() {
        let cli = Cli::parse_from(["git-sc", "-y"]);
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_auto_confirm_long() {
        let cli = Cli::parse_from(["git-sc", "--yes"]);
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_dry_run_short() {
        let cli = Cli::parse_from(["git-sc", "-n"]);
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_dry_run_long() {
        let cli = Cli::parse_from(["git-sc", "--dry-run"]);
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_stage_all_short() {
        let cli = Cli::parse_from(["git-sc", "-a"]);
        assert!(cli.stage_all);
    }

    #[test]
    fn test_cli_stage_all_long() {
        let cli = Cli::parse_from(["git-sc", "--all"]);
        assert!(cli.stage_all);
    }

    #[test]
    fn test_cli_amend() {
        let cli = Cli::parse_from(["git-sc", "--amend"]);
        assert!(cli.amend);
    }

    #[test]
    fn test_cli_squash_with_base() {
        let cli = Cli::parse_from(["git-sc", "--squash", "origin/main"]);
        assert_eq!(cli.squash, Some("origin/main".to_string()));
    }

    #[test]
    fn test_cli_squash_with_feature_branch() {
        let cli = Cli::parse_from(["git-sc", "--squash", "origin/feature/test"]);
        assert_eq!(cli.squash, Some("origin/feature/test".to_string()));
    }

    #[test]
    fn test_cli_language_short() {
        let cli = Cli::parse_from(["git-sc", "-l", "English"]);
        assert_eq!(cli.language, Some("English".to_string()));
    }

    #[test]
    fn test_cli_language_long() {
        let cli = Cli::parse_from(["git-sc", "--lang", "Japanese"]);
        assert_eq!(cli.language, Some("Japanese".to_string()));
    }

    #[test]
    fn test_cli_combined_options() {
        let cli = Cli::parse_from(["git-sc", "-a", "-y", "-l", "English"]);
        assert!(cli.auto_confirm);
        assert!(cli.stage_all);
        assert_eq!(cli.language, Some("English".to_string()));
    }

    #[test]
    fn test_cli_squash_with_confirm() {
        let cli = Cli::parse_from(["git-sc", "--squash", "main", "-y"]);
        assert_eq!(cli.squash, Some("main".to_string()));
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_squash_with_dry_run() {
        let cli = Cli::parse_from(["git-sc", "--squash", "develop", "-n"]);
        assert_eq!(cli.squash, Some("develop".to_string()));
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_amend_with_options() {
        let cli = Cli::parse_from(["git-sc", "--amend", "-y", "-l", "English"]);
        assert!(cli.amend);
        assert!(cli.auto_confirm);
        assert_eq!(cli.language, Some("English".to_string()));
    }

    #[test]
    fn test_cli_debug_short() {
        let cli = Cli::parse_from(["git-sc", "-d"]);
        assert!(cli.debug);
    }

    #[test]
    fn test_cli_debug_long() {
        let cli = Cli::parse_from(["git-sc", "--debug"]);
        assert!(cli.debug);
    }

    #[test]
    fn test_cli_debug_with_dry_run() {
        let cli = Cli::parse_from(["git-sc", "-d", "-n"]);
        assert!(cli.debug);
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_reword() {
        let cli = Cli::parse_from(["git-sc", "--reword", "abc1234"]);
        assert_eq!(cli.reword, Some("abc1234".to_string()));
    }

    #[test]
    fn test_cli_reword_with_confirm() {
        let cli = Cli::parse_from(["git-sc", "--reword", "abc1234", "-y"]);
        assert_eq!(cli.reword, Some("abc1234".to_string()));
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_reword_with_dry_run() {
        let cli = Cli::parse_from(["git-sc", "--reword", "abc1234", "-n"]);
        assert_eq!(cli.reword, Some("abc1234".to_string()));
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_reword_with_full_hash() {
        let cli = Cli::parse_from([
            "git-sc",
            "--reword",
            "1234567890abcdef1234567890abcdef12345678",
        ]);
        assert_eq!(
            cli.reword,
            Some("1234567890abcdef1234567890abcdef12345678".to_string())
        );
    }

    #[test]
    fn test_cli_body_short() {
        let cli = Cli::parse_from(["git-sc", "-b"]);
        assert!(cli.with_body);
    }

    #[test]
    fn test_cli_body_long() {
        let cli = Cli::parse_from(["git-sc", "--body"]);
        assert!(cli.with_body);
    }

    #[test]
    fn test_cli_body_with_stage_all() {
        let cli = Cli::parse_from(["git-sc", "-a", "-b", "-y"]);
        assert!(cli.stage_all);
        assert!(cli.with_body);
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_generate_for_short() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234"]);
        assert_eq!(cli.generate_for, Some(vec!["abc1234".to_string()]));
    }

    #[test]
    fn test_cli_generate_for_long() {
        let cli = Cli::parse_from(["git-sc", "--generate-for", "abc1234def5678"]);
        assert_eq!(cli.generate_for, Some(vec!["abc1234def5678".to_string()]));
    }

    #[test]
    fn test_cli_generate_for_multiple() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234", "def5678", "ghi9012"]);
        assert_eq!(
            cli.generate_for,
            Some(vec![
                "abc1234".to_string(),
                "def5678".to_string(),
                "ghi9012".to_string()
            ])
        );
    }

    #[test]
    fn test_cli_generate_for_with_body() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234", "-b"]);
        assert_eq!(cli.generate_for, Some(vec!["abc1234".to_string()]));
        assert!(cli.with_body);
    }

    #[test]
    fn test_cli_generate_for_multiple_with_body() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234", "def5678", "-b"]);
        assert_eq!(
            cli.generate_for,
            Some(vec!["abc1234".to_string(), "def5678".to_string()])
        );
        assert!(cli.with_body);
    }

    #[test]
    fn test_cli_generate_for_with_language() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234", "-l", "English"]);
        assert_eq!(cli.generate_for, Some(vec!["abc1234".to_string()]));
        assert_eq!(cli.language, Some("English".to_string()));
    }

    #[test]
    fn test_cli_generate_for_multiple_with_language() {
        let cli = Cli::parse_from(["git-sc", "-g", "abc1234", "def5678", "-l", "English"]);
        assert_eq!(
            cli.generate_for,
            Some(vec!["abc1234".to_string(), "def5678".to_string()])
        );
        assert_eq!(cli.language, Some("English".to_string()));
    }

    #[test]
    fn test_cli_quiet_short() {
        let cli = Cli::parse_from(["git-sc", "-q"]);
        assert!(cli.quiet);
    }

    #[test]
    fn test_cli_quiet_long() {
        let cli = Cli::parse_from(["git-sc", "--quiet"]);
        assert!(cli.quiet);
    }

    #[test]
    fn test_cli_quiet_with_all_and_yes() {
        let cli = Cli::parse_from(["git-sc", "-a", "-y", "-q"]);
        assert!(cli.stage_all);
        assert!(cli.auto_confirm);
        assert!(cli.quiet);
    }

    #[test]
    fn test_cli_provider_short() {
        let cli = Cli::parse_from(["git-sc", "-p", "gemini"]);
        assert_eq!(cli.provider, Some("gemini".to_string()));
    }

    #[test]
    fn test_cli_provider_long() {
        let cli = Cli::parse_from(["git-sc", "--provider", "claude"]);
        assert_eq!(cli.provider, Some("claude".to_string()));
    }

    #[test]
    fn test_cli_provider_with_other_options() {
        let cli = Cli::parse_from(["git-sc", "-p", "codex", "-a", "-y"]);
        assert_eq!(cli.provider, Some("codex".to_string()));
        assert!(cli.stage_all);
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_generate_for_full_hash() {
        let cli = Cli::parse_from([
            "git-sc",
            "--generate-for",
            "1234567890abcdef1234567890abcdef12345678",
        ]);
        assert_eq!(
            cli.generate_for,
            Some(vec!["1234567890abcdef1234567890abcdef12345678".to_string()])
        );
    }

    #[test]
    fn test_cli_amend_conflicts_with_squash() {
        let result = Cli::try_parse_from(["git-sc", "--amend", "--squash", "main"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_amend_conflicts_with_reword() {
        let result = Cli::try_parse_from(["git-sc", "--amend", "--reword", "HEAD"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_cli_generate_for_conflicts_with_squash() {
        let result = Cli::try_parse_from(["git-sc", "--generate-for", "HEAD", "--squash", "main"]);
        assert!(result.is_err());
    }

    // ============================================================
    // Init サブコマンドのテスト
    // ============================================================

    #[test]
    fn test_cli_init_subcommand() {
        let cli = Cli::parse_from(["git-sc", "init"]);
        assert!(matches!(cli.command, Some(Command::Init { force: false })));
    }

    #[test]
    fn test_cli_init_subcommand_with_force() {
        let cli = Cli::parse_from(["git-sc", "init", "--force"]);
        assert!(matches!(cli.command, Some(Command::Init { force: true })));
    }

    #[test]
    fn test_cli_no_subcommand() {
        let cli = Cli::parse_from(["git-sc"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_cli_command_enum_debug() {
        let cmd = Command::Init { force: false };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Init"));
    }

    #[test]
    fn test_cli_command_enum_clone() {
        let cmd = Command::Init { force: true };
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }
}
