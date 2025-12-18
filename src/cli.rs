use clap::Parser;

/// AI CLI（Gemini、Codex、Claude）を使用したスマートコミットメッセージ生成ツール
#[derive(Parser, Debug)]
#[command(name = "git-sc")]
#[command(about = "AI CLI（Gemini、Codex、Claude）を使用したスマートコミットメッセージ生成ツール")]
#[command(version)]
pub struct Cli {
    /// 確認プロンプトをスキップして直接コミット
    #[arg(short = 'y', long = "yes")]
    pub auto_confirm: bool,

    /// 実際にコミットせずに生成されたメッセージを表示
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// アンステージの変更も含めて全てをステージングしてコミット
    #[arg(short = 'a', long = "all")]
    pub stage_all: bool,

    /// 直前のコミットを新しく生成されたメッセージで修正
    #[arg(long = "amend")]
    pub amend: bool,

    /// ブランチ内の全コミットを1つにまとめて新しいメッセージを生成（ベースブランチを指定）
    #[arg(long = "squash", value_name = "BASE")]
    pub squash: Option<String>,

    /// N個前のコミットメッセージを再生成（git rebase を使用）
    #[arg(long = "reword", value_name = "N")]
    pub reword: Option<usize>,

    /// コミットメッセージの言語（設定ファイルを上書き）
    #[arg(short = 'l', long = "lang")]
    pub language: Option<String>,

    /// デバッグモード（AIに渡すプロンプトを表示）
    #[arg(short = 'd', long = "debug")]
    pub debug: bool,
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
        assert!(cli.language.is_none());
        assert!(!cli.debug);
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
        let cli = Cli::parse_from(["git-sc", "--reword", "3"]);
        assert_eq!(cli.reword, Some(3));
    }

    #[test]
    fn test_cli_reword_with_confirm() {
        let cli = Cli::parse_from(["git-sc", "--reword", "5", "-y"]);
        assert_eq!(cli.reword, Some(5));
        assert!(cli.auto_confirm);
    }

    #[test]
    fn test_cli_reword_with_dry_run() {
        let cli = Cli::parse_from(["git-sc", "--reword", "2", "-n"]);
        assert_eq!(cli.reword, Some(2));
        assert!(cli.dry_run);
    }
}
