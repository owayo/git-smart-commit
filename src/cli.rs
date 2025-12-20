use clap::Parser;

/// AI-powered smart commit message generator using coding agents (Gemini CLI, Codex CLI, or Claude Code)
#[derive(Parser, Debug)]
#[command(name = "git-sc")]
#[command(
    about = "AI-powered smart commit message generator using coding agents (Gemini CLI, Codex CLI, or Claude Code)"
)]
#[command(version)]
pub struct Cli {
    /// Skip confirmation prompt and commit directly
    #[arg(short = 'y', long = "yes")]
    pub auto_confirm: bool,

    /// Show generated message without committing
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Stage all changes including unstaged and commit
    #[arg(short = 'a', long = "all")]
    pub stage_all: bool,

    /// Amend the last commit with a newly generated message
    #[arg(long = "amend")]
    pub amend: bool,

    /// Squash all commits in branch into one with a new message (specify base branch)
    #[arg(long = "squash", value_name = "BASE")]
    pub squash: Option<String>,

    /// Regenerate commit message for specified commit hash (uses git rebase)
    #[arg(long = "reword", value_name = "HASH")]
    pub reword: Option<String>,

    /// Generate message from diff of specified commit hash(es) (output only, multiple allowed)
    #[arg(short = 'g', long = "generate-for", value_name = "HASH", num_args = 1..)]
    pub generate_for: Option<Vec<String>>,

    /// Generate commit message with body
    #[arg(short = 'b', long = "body")]
    pub with_body: bool,

    /// Language for commit message (overrides config file)
    #[arg(short = 'l', long = "lang")]
    pub language: Option<String>,

    /// Debug mode (show prompt sent to AI)
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
        assert!(cli.generate_for.is_none());
        assert!(!cli.with_body);
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
}
