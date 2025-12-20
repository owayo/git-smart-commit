use thiserror::Error;

/// アプリケーションエラーの種類
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Gitリポジトリではありません。Gitリポジトリ内でこのコマンドを実行してください。")]
    NotGitRepository,

    #[error("変更が見つかりません。コミットメッセージを生成するには変更を加えてください。")]
    NoChanges,

    #[error("ステージ済みの変更がありません。'git add'でファイルをステージするか、-aフラグをつけて実行してください。")]
    NoStagedChanges,

    #[error("AI CLIがインストールされていません。gemini、codex、またはclaudeのいずれかをインストールしてください。")]
    NoAiProviderInstalled,

    #[error("{0}")]
    AiProviderError(String),

    #[error("Gitコマンドが失敗しました: {0}")]
    GitError(String),

    #[error("ユーザーが操作をキャンセルしました")]
    UserCancelled,

    #[error("設定エラー: {0}")]
    ConfigError(String),

    #[error("ベースブランチが見つかりません。--base オプションで指定してください。")]
    NoBaseBranch,

    #[error("squash対象のコミットがありません。現在のブランチにベースからの変更がないか確認してください。")]
    NoCommitsToSquash,

    #[error("ベースブランチ上では squash できません。フィーチャーブランチに切り替えてください。")]
    OnBaseBranch,

    #[error("指定範囲にマージコミットが含まれています。rewordはマージコミットを含む範囲では使用できません。")]
    HasMergeCommits,

    #[error("rebase中にコンフリクトが発生しました。rebaseを中止しました。")]
    RebaseConflict,

    #[error("Invalid reword target. Please specify a valid commit hash.")]
    InvalidRewordTarget,

    #[error("無効なコミットハッシュ: {0}")]
    InvalidCommitHash(String),

    #[error("--generate-for と --{0} は同時に使用できません")]
    ConflictingOptions(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // AppError メッセージのテスト
    // ============================================================

    #[test]
    fn test_error_not_git_repository() {
        let err = AppError::NotGitRepository;
        assert_eq!(
            err.to_string(),
            "Gitリポジトリではありません。Gitリポジトリ内でこのコマンドを実行してください。"
        );
    }

    #[test]
    fn test_error_no_changes() {
        let err = AppError::NoChanges;
        assert_eq!(
            err.to_string(),
            "変更が見つかりません。コミットメッセージを生成するには変更を加えてください。"
        );
    }

    #[test]
    fn test_error_no_staged_changes() {
        let err = AppError::NoStagedChanges;
        assert_eq!(
            err.to_string(),
            "ステージ済みの変更がありません。'git add'でファイルをステージするか、-aフラグをつけて実行してください。"
        );
    }

    #[test]
    fn test_error_no_ai_provider_installed() {
        let err = AppError::NoAiProviderInstalled;
        assert_eq!(
            err.to_string(),
            "AI CLIがインストールされていません。gemini、codex、またはclaudeのいずれかをインストールしてください。"
        );
    }

    #[test]
    fn test_error_ai_provider_error() {
        let err = AppError::AiProviderError("API rate limit exceeded".to_string());
        assert_eq!(err.to_string(), "API rate limit exceeded");
    }

    #[test]
    fn test_error_git_error() {
        let err = AppError::GitError("fatal: not a git repository".to_string());
        assert_eq!(
            err.to_string(),
            "Gitコマンドが失敗しました: fatal: not a git repository"
        );
    }

    #[test]
    fn test_error_user_cancelled() {
        let err = AppError::UserCancelled;
        assert_eq!(err.to_string(), "ユーザーが操作をキャンセルしました");
    }

    #[test]
    fn test_error_config_error() {
        let err = AppError::ConfigError("Invalid TOML format".to_string());
        assert_eq!(err.to_string(), "設定エラー: Invalid TOML format");
    }

    #[test]
    fn test_error_no_base_branch() {
        let err = AppError::NoBaseBranch;
        assert_eq!(
            err.to_string(),
            "ベースブランチが見つかりません。--base オプションで指定してください。"
        );
    }

    #[test]
    fn test_error_no_commits_to_squash() {
        let err = AppError::NoCommitsToSquash;
        assert_eq!(
            err.to_string(),
            "squash対象のコミットがありません。現在のブランチにベースからの変更がないか確認してください。"
        );
    }

    #[test]
    fn test_error_on_base_branch() {
        let err = AppError::OnBaseBranch;
        assert_eq!(
            err.to_string(),
            "ベースブランチ上では squash できません。フィーチャーブランチに切り替えてください。"
        );
    }

    #[test]
    fn test_error_debug_format() {
        let err = AppError::NoBaseBranch;
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("NoBaseBranch"));
    }

    #[test]
    fn test_error_has_merge_commits() {
        let err = AppError::HasMergeCommits;
        assert_eq!(
            err.to_string(),
            "指定範囲にマージコミットが含まれています。rewordはマージコミットを含む範囲では使用できません。"
        );
    }

    #[test]
    fn test_error_rebase_conflict() {
        let err = AppError::RebaseConflict;
        assert_eq!(
            err.to_string(),
            "rebase中にコンフリクトが発生しました。rebaseを中止しました。"
        );
    }

    #[test]
    fn test_error_invalid_reword_target() {
        let err = AppError::InvalidRewordTarget;
        assert_eq!(
            err.to_string(),
            "Invalid reword target. Please specify a valid commit hash."
        );
    }

    #[test]
    fn test_error_invalid_commit_hash() {
        let err = AppError::InvalidCommitHash("xyz123".to_string());
        assert_eq!(err.to_string(), "無効なコミットハッシュ: xyz123");
    }

    #[test]
    fn test_error_conflicting_options() {
        let err = AppError::ConflictingOptions("amend".to_string());
        assert_eq!(
            err.to_string(),
            "--generate-for と --amend は同時に使用できません"
        );
    }
}
