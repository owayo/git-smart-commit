use thiserror::Error;

/// アプリケーションエラーの種類
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Gitリポジトリではありません。Gitリポジトリ内でこのコマンドを実行してください。")]
    NotGitRepository,

    #[error("変更が見つかりません。コミットメッセージを生成するには変更を加えてください。")]
    NoChanges,

    #[error("ステージ済みの変更がありません。'git add'でファイルをステージするか、-sフラグなしで実行してください。")]
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
}
