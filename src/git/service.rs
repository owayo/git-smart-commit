use std::path::PathBuf;
use std::process::Command;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::AppError;

/// 差分の最大文字数
const MAX_DIFF_CHARS: usize = 10000;

/// プレフィックススクリプトの実行結果
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptResult {
    /// プレフィックスが返された（exit 0 + 内容あり）
    Prefix(String),
    /// 空が返された（exit 0 + 内容なし）→ プレフィックスなし
    Empty,
    /// スクリプトが失敗（exit 1）→ AI生成のメッセージをそのまま使用
    Failed,
}

/// Git操作サービス
pub struct GitService {
    repo_path: PathBuf,
}

impl GitService {
    /// 現在のディレクトリに対するGitServiceを作成
    pub fn new() -> Self {
        Self {
            repo_path: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    // ============================================================
    // git コマンド実行ヘルパー
    // ============================================================

    /// git コマンドを実行し、成功時は stdout（trim済み）を返す
    fn run_git(&self, args: &[&str]) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;
        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// git コマンドを実行し、成功/失敗のみを返す（stdout不要のケース）
    fn run_git_ok(&self, args: &[&str]) -> Result<(), AppError> {
        self.run_git(args)?;
        Ok(())
    }

    /// git コマンドを実行し、成功時は Some(stdout)、失敗時は None を返す
    fn try_run_git(&self, args: &[&str]) -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .ok()?;

        if output.status.success() {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        } else {
            None
        }
    }

    /// コミットハッシュが有効か検証し、無効な場合は InvalidCommitHash エラーを返す
    fn verify_commit_hash(&self, hash: &str) -> Result<(), AppError> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", hash])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::InvalidCommitHash(hash.to_string()));
        }
        Ok(())
    }

    // ============================================================
    // 内部ユーティリティ
    // ============================================================

    /// Gitリポジトリのルートディレクトリを取得
    fn get_git_root(&self) -> Option<PathBuf> {
        self.try_run_git(&["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
    }

    /// .git-sc-ignoreファイルを読み込んでGitignoreを構築
    fn load_ignore_patterns(&self) -> Option<Gitignore> {
        let git_root = self.get_git_root()?;
        let ignore_path = git_root.join(".git-sc-ignore");

        if !ignore_path.exists() {
            return None;
        }

        let mut builder = GitignoreBuilder::new(&git_root);
        if builder.add(&ignore_path).is_some() {
            // エラーがあった場合はNoneを返す
            return None;
        }

        builder.build().ok()
    }

    /// diffからignoreパターンにマッチするファイルを除外
    fn filter_ignored_files(diff_text: &str, ignore: &Gitignore) -> String {
        if diff_text.is_empty() {
            return String::new();
        }

        let lines: Vec<&str> = diff_text.lines().collect();
        let mut filtered_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if line.starts_with("diff --git") {
                // ファイルパスを抽出 (例: "diff --git a/path/to/file b/path/to/file")
                let block_start = i;
                let file_path = Self::extract_file_path_from_diff_header(line);

                // ignoreパターンにマッチするかチェック
                let should_ignore = file_path
                    .map(|p| ignore.matched_path_or_any_parents(p, false).is_ignore())
                    .unwrap_or(false);

                // このブロックの終端を見つける
                i += 1;
                while i < lines.len() && !lines[i].starts_with("diff --git") {
                    i += 1;
                }

                // ignoreにマッチしなければブロックを追加
                if !should_ignore {
                    for line in lines.iter().take(i).skip(block_start) {
                        filtered_lines.push(*line);
                    }
                }
                continue;
            } else {
                filtered_lines.push(line);
            }
            i += 1;
        }

        filtered_lines.join("\n")
    }

    /// diffヘッダーからファイルパスを抽出
    fn extract_file_path_from_diff_header(header: &str) -> Option<&str> {
        // "diff --git a/path/to/file b/path/to/file" から "path/to/file" を抽出
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() >= 4 {
            // "a/path/to/file" から先頭の "a/" を除去
            let a_path = parts[2];
            if let Some(stripped) = a_path.strip_prefix("a/") {
                return Some(stripped);
            }
        }
        None
    }

    /// diffを最大文字数に切り詰める
    pub fn truncate_diff(diff: &str) -> String {
        if diff.chars().count() <= MAX_DIFF_CHARS {
            return diff.to_string();
        }

        // 文字数でカット
        let truncated: String = diff.chars().take(MAX_DIFF_CHARS).collect();

        // 最後の完全な行まで切り詰める（中途半端な行を避ける）
        if let Some(last_newline) = truncated.rfind('\n') {
            format!(
                "{}\n\n... (diff truncated: exceeded {} characters)",
                &truncated[..last_newline],
                MAX_DIFF_CHARS
            )
        } else {
            format!(
                "{}\n\n... (diff truncated: exceeded {} characters)",
                truncated, MAX_DIFF_CHARS
            )
        }
    }

    /// diffに対して全てのフィルタリングを適用
    fn apply_all_filters(&self, diff: &str) -> String {
        // 1. バイナリファイルを除外
        let filtered = Self::filter_binary_diff(diff);

        // 2. .git-sc-ignore パターンにマッチするファイルを除外
        let filtered = if let Some(ignore) = self.load_ignore_patterns() {
            Self::filter_ignored_files(&filtered, &ignore)
        } else {
            filtered
        };

        // 3. 文字数制限を適用
        Self::truncate_diff(&filtered)
    }

    /// git diffの出力からバイナリファイルの詳細差分を除外し、変更種別のみを出力
    fn filter_binary_diff(diff_text: &str) -> String {
        if diff_text.is_empty() {
            return String::new();
        }

        let lines: Vec<&str> = diff_text.lines().collect();
        let mut filtered_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if line.starts_with("diff --git") {
                // 新しいdiffブロックの開始
                let block_start = i;
                let file_path = Self::extract_file_path_from_diff_header(line)
                    .unwrap_or("unknown")
                    .to_string();
                i += 1;

                // ブロック内の情報を収集
                let mut is_binary = false;
                let mut is_new_file = false;
                let mut is_deleted = false;
                let mut rename_from: Option<String> = None;
                let mut rename_to: Option<String> = None;

                while i < lines.len() && !lines[i].starts_with("diff --git") {
                    let current_line = lines[i];

                    if current_line.starts_with("Binary files") {
                        is_binary = true;
                    } else if current_line.starts_with("new file mode") {
                        is_new_file = true;
                    } else if current_line.starts_with("deleted file mode") {
                        is_deleted = true;
                    } else if let Some(from) = current_line.strip_prefix("rename from ") {
                        rename_from = Some(from.to_string());
                    } else if let Some(to) = current_line.strip_prefix("rename to ") {
                        rename_to = Some(to.to_string());
                    }
                    i += 1;
                }

                if is_binary {
                    // バイナリファイルは変更種別のサマリーのみ出力
                    let summary = if let (Some(from), Some(to)) = (&rename_from, &rename_to) {
                        format!("[Binary] renamed: {} -> {}", from, to)
                    } else if is_new_file {
                        format!("[Binary] added: {}", file_path)
                    } else if is_deleted {
                        format!("[Binary] deleted: {}", file_path)
                    } else {
                        format!("[Binary] modified: {}", file_path)
                    };
                    filtered_lines.push(summary);
                } else {
                    // テキストファイルはそのまま出力
                    for line in lines.iter().take(i).skip(block_start) {
                        filtered_lines.push((*line).to_string());
                    }
                }
                continue;
            } else {
                filtered_lines.push(line.to_string());
            }
            i += 1;
        }

        filtered_lines.join("\n")
    }

    // ============================================================
    // パブリック API
    // ============================================================

    /// Gitリポジトリ内にいるか確認
    pub fn verify_repository(&self) -> Result<(), AppError> {
        // .git ディレクトリが直接存在するかチェック
        if self.repo_path.join(".git").exists() {
            return Ok(());
        }

        // Gitリポジトリのサブディレクトリにいる場合もチェック
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::NotGitRepository)
        }
    }

    /// ステージ済みのdiffを取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_staged_diff(&self) -> Result<String, AppError> {
        let raw = self.run_git(&["diff", "--cached", "-w", "-U0"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 直近のコミットメッセージを取得
    pub fn get_recent_commits(&self, count: usize) -> Result<Vec<String>, AppError> {
        let output = Command::new("git")
            .args(["log", "--format=%s", "-n", &count.to_string()])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            // コミットがまだない場合は空のベクタを返す
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("does not have any commits") {
                return Ok(Vec::new());
            }
            return Err(AppError::GitError(stderr.to_string()));
        }

        let commits: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        Ok(commits)
    }

    /// 全ての変更をステージング
    pub fn stage_all(&self) -> Result<(), AppError> {
        // Windows環境では "nul" は予約デバイス名
        // AIプロバイダー呼び出し時に意図せず "nul" ファイルが作成されることがあるため、
        // nul ファイルの削除 + ステージング除外の二重防御を行う
        #[cfg(windows)]
        {
            let nul_path = self.repo_path.join("nul");
            if nul_path.exists() {
                let _ = std::fs::remove_file(&nul_path);
            }
        }

        // Windows環境では "nul" を除外してステージング（pathspecの除外指定を使用）
        #[cfg(windows)]
        let args: &[&str] = &["add", "-A", "--", ".", ":!nul"];
        #[cfg(not(windows))]
        let args: &[&str] = &["add", "-A"];

        self.run_git_ok(args)?;

        // Windows環境で万が一 nul がステージされていた場合はアンステージ
        #[cfg(windows)]
        {
            let _ = Command::new("git")
                .args(["reset", "HEAD", "--", "nul"])
                .current_dir(&self.repo_path)
                .output();
        }

        Ok(())
    }

    /// 指定されたメッセージでコミットを作成
    pub fn commit(&self, message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "-m", message])
    }

    /// リモートにpush
    pub fn push(&self) -> Result<(), AppError> {
        self.run_git_ok(&["push"])
    }

    /// auto_push が有効かどうかを判定
    pub fn is_auto_push_enabled(&self, config_auto_push: Option<bool>) -> bool {
        config_auto_push.unwrap_or(false)
    }

    /// 直前のコミットのdiffを取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_last_commit_diff(&self) -> Result<String, AppError> {
        let raw = self.run_git(&["diff", "-w", "-U0", "HEAD~1", "HEAD"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 直前のコミットを新しいメッセージで修正
    pub fn amend_commit(&self, message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "--amend", "-m", message])
    }

    /// リモートURLを取得（origin）
    pub fn get_remote_url(&self) -> Option<String> {
        self.try_run_git(&["config", "--get", "remote.origin.url"])
    }

    /// 現在のブランチ名を取得
    pub fn get_current_branch(&self) -> Option<String> {
        self.try_run_git(&["branch", "--show-current"])
    }

    /// プレフィックススクリプトを実行してプレフィックスを取得
    ///
    /// 戻り値:
    /// - `Some(ScriptResult::Prefix(s))`: スクリプトがプレフィックスを返した（exit 0 + 内容あり）
    /// - `Some(ScriptResult::Empty)`: スクリプトが空を返した（exit 0 + 内容なし）→ プレフィックスなし
    /// - `Some(ScriptResult::Failed)`: スクリプトが失敗した（exit 1）→ AI生成メッセージを使用
    /// - `None`: スクリプトの実行自体に失敗
    ///
    /// スクリプトの stderr は直接ターミナルに出力されます。
    pub fn run_prefix_script(
        &self,
        script: &str,
        remote_url: &str,
        branch: &str,
    ) -> Option<ScriptResult> {
        use std::process::Stdio;

        let output = Command::new(script)
            .args([remote_url, branch])
            .current_dir(&self.repo_path)
            .stderr(Stdio::inherit()) // stderrは直接ターミナルに出力
            .output()
            .ok()?;

        if output.status.success() {
            let prefix = String::from_utf8_lossy(&output.stdout).to_string();
            if prefix.trim().is_empty() {
                Some(ScriptResult::Empty)
            } else {
                Some(ScriptResult::Prefix(prefix))
            }
        } else {
            // 終了コード1: AI生成のメッセージをそのまま使用
            Some(ScriptResult::Failed)
        }
    }

    /// ブランチが存在するか確認
    pub fn branch_exists(&self, branch: &str) -> bool {
        self.try_run_git(&["rev-parse", "--verify", branch])
            .is_some()
    }

    /// 2つのブランチのmerge-baseを取得
    pub fn get_merge_base(&self, base: &str, head: &str) -> Result<String, AppError> {
        self.run_git(&["merge-base", base, head]).map_err(|_| {
            AppError::GitError(format!(
                "Failed to find merge-base between {} and {}",
                base, head
            ))
        })
    }

    /// ベースからHEADまでのコミット数を取得
    pub fn count_commits_from_base(&self, base: &str) -> Result<usize, AppError> {
        let count_str = self.run_git(&["rev-list", "--count", &format!("{}..HEAD", base)])?;
        count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))
    }

    /// ベースからHEADまでの差分を取得（バイナリファイル、.git-sc-ignore対象、空白のみの変更を除外）
    pub fn get_diff_from_base(&self, base: &str) -> Result<String, AppError> {
        let raw = self.run_git(&["diff", "-w", "-U0", base, "HEAD"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 指定したコミットにsoft resetする
    pub fn soft_reset_to(&self, commit: &str) -> Result<(), AppError> {
        self.run_git_ok(&["reset", "--soft", commit])
    }

    /// 指定範囲にマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range(&self, n: usize) -> Result<bool, AppError> {
        // マージコミットは親が2つ以上ある
        let merges = self.run_git(&["rev-list", "--merges", &format!("HEAD~{}..HEAD", n)])?;
        Ok(!merges.is_empty())
    }

    /// 指定されたコミットハッシュの差分を取得
    pub fn get_commit_diff_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        // git show でそのコミットの差分を取得
        let raw = self.run_git(&["show", hash, "--format=", "--no-color", "-w", "-U0"])?;
        Ok(self.apply_all_filters(&raw))
    }

    /// 指定されたコミットハッシュのメッセージを取得
    pub fn get_commit_message_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        self.run_git(&["log", "-1", "--format=%s", hash])
    }

    /// reword対象として有効なコミットか確認（現在のHEAD履歴上に存在するか）
    fn validate_reword_target_hash(&self, hash: &str) -> Result<(), AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        // 履歴外のコミットをreword対象にすると誤ったコミット位置が算出されるため、祖先関係を厳密に確認
        let ancestor_output = Command::new("git")
            .args(["merge-base", "--is-ancestor", hash, "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        match ancestor_output.status.code() {
            Some(0) => Ok(()),
            Some(1) => Err(AppError::InvalidRewordTarget),
            _ => {
                let stderr = String::from_utf8_lossy(&ancestor_output.stderr)
                    .trim()
                    .to_string();
                if stderr.is_empty() {
                    Err(AppError::GitError(
                        "Failed to verify reword target ancestry".to_string(),
                    ))
                } else {
                    Err(AppError::GitError(stderr))
                }
            }
        }
    }

    /// 指定されたコミットハッシュがHEADから何個前かを取得
    pub fn get_commit_position_by_hash(&self, hash: &str) -> Result<usize, AppError> {
        self.validate_reword_target_hash(hash)?;

        // HEADからそのコミットまでのコミット数をカウント
        // git rev-list --count hash..HEAD で hash から HEAD までのコミット数を取得
        // これに1を足すと、そのコミット自体の位置になる
        let count_str = self.run_git(&["rev-list", "--count", &format!("{}..HEAD", hash)])?;
        let count: usize = count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))?;

        // count はそのコミットより新しいコミットの数なので、+1で位置になる
        Ok(count + 1)
    }

    /// 指定されたコミットハッシュからHEADまでにマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range_by_hash(&self, hash: &str) -> Result<bool, AppError> {
        self.validate_reword_target_hash(hash)?;

        // マージコミットは親が2つ以上ある
        let merges = self.run_git(&["rev-list", "--merges", &format!("{}..HEAD", hash)])?;
        Ok(!merges.is_empty())
    }

    /// 指定されたコミットハッシュのメッセージを変更（rebase使用）
    pub fn reword_commit_by_hash(&self, hash: &str, new_message: &str) -> Result<(), AppError> {
        // 位置を取得
        let n = self.get_commit_position_by_hash(hash)?;

        // 既存のreword_commitを利用
        self.reword_commit(n, new_message)
    }

    /// N個前のコミットのメッセージを変更（rebase使用）
    pub fn reword_commit(&self, n: usize, new_message: &str) -> Result<(), AppError> {
        if n == 0 {
            return Err(AppError::InvalidRewordTarget);
        }

        // n=1 の場合は amend で処理
        if n == 1 {
            return self.amend_commit_message(new_message);
        }

        // マージコミットをチェック
        if self.has_merge_commits_in_range(n)? {
            return Err(AppError::HasMergeCommits);
        }

        // 一時ファイルにメッセージを保存
        let temp_dir = std::env::temp_dir();
        let msg_file = temp_dir.join("git-sc-reword-message.txt");
        std::fs::write(&msg_file, new_message)
            .map_err(|e| AppError::GitError(format!("Failed to create temp file: {}", e)))?;

        // GIT_SEQUENCE_EDITOR: 最初の pick を reword に変更
        // シェル経由で実行するために sh -c でラップする
        let sequence_editor = if cfg!(windows) {
            // Windows環境では PowerShell を使用
            "powershell -Command \"(Get-Content $args[0]) -replace '^pick', 'reword' | Set-Content $args[0]\"".to_string()
        } else {
            // Unix系環境では sed を使用（macOS/Linux対応）
            // sh -c でラップし、-- の後に $1 を渡す
            "sh -c 'sed -i.bak '\"'\"'1s/^pick/reword/'\"'\"' \"$1\" && rm -f \"$1.bak\"' --"
                .to_string()
        };

        // GIT_EDITOR: 一時ファイルの内容をコミットメッセージへ反映
        let editor = if cfg!(windows) {
            format!(
                "powershell -Command \"Copy-Item '{}' $args[0]\"",
                msg_file.display()
            )
        } else {
            // sh -c でラップ
            format!("sh -c 'cp \"{}\" \"$1\"' --", msg_file.display())
        };

        // git rebase -i を実行
        let output = Command::new("git")
            .args(["rebase", "-i", &format!("HEAD~{}", n)])
            .env("GIT_SEQUENCE_EDITOR", &sequence_editor)
            .env("GIT_EDITOR", &editor)
            .env("EDITOR", &editor)
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        // 一時ファイルを削除
        let _ = std::fs::remove_file(&msg_file);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // コンフリクトの場合はrebaseを中止
            if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
                let _ = Command::new("git")
                    .args(["rebase", "--abort"])
                    .current_dir(&self.repo_path)
                    .output();
                return Err(AppError::RebaseConflict);
            }

            return Err(AppError::GitError(stderr.to_string()));
        }

        Ok(())
    }

    /// コミットメッセージを変更（amend）
    fn amend_commit_message(&self, new_message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "--amend", "-m", new_message])
    }
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    // ============================================================
    // filter_binary_diff のテスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_empty_input() {
        let result = GitService::filter_binary_diff("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_binary_diff_no_binary() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }"#;
        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_filter_binary_diff_shows_binary_info() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }
diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ"#;

        let result = GitService::filter_binary_diff(diff);

        // テキストファイルの差分が含まれる
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("println"));
        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: image.png"));
        // 詳細なバイナリ差分は含まれない
        assert!(!result.contains("Binary files a/"));
    }

    #[test]
    fn test_filter_binary_diff_only_binary() {
        let diff = r#"diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] modified: image.png");
    }

    #[test]
    fn test_filter_binary_diff_multiple_binaries() {
        let diff = r#"diff --git a/src/lib.rs b/src/lib.rs
index 1234567..abcdefg 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,2 @@
+// new comment
diff --git a/image1.png b/image1.png
Binary files a/image1.png and b/image1.png differ
diff --git a/image2.jpg b/image2.jpg
Binary files a/image2.jpg and b/image2.jpg differ
diff --git a/config.toml b/config.toml
index 1111111..2222222 100644
--- a/config.toml
+++ b/config.toml
@@ -1 +1,2 @@
+key = "value""#;

        let result = GitService::filter_binary_diff(diff);

        // テキストファイルの変更が含まれる
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("config.toml"));
        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: image1.png"));
        assert!(result.contains("[Binary] modified: image2.jpg"));
        // 詳細なバイナリ差分は含まれない
        assert!(!result.contains("Binary files a/"));
    }

    #[test]
    fn test_filter_binary_diff_binary_at_start() {
        let diff = r#"diff --git a/logo.svg b/logo.svg
Binary files a/logo.svg and b/logo.svg differ
diff --git a/README.md b/README.md
index aaa..bbb 100644
--- a/README.md
+++ b/README.md
@@ -1 +1,2 @@
+# Title"#;

        let result = GitService::filter_binary_diff(diff);

        // バイナリファイルは変更種別として出力される
        assert!(result.contains("[Binary] modified: logo.svg"));
        assert!(result.contains("README.md"));
        assert!(result.contains("# Title"));
    }

    #[test]
    fn test_filter_binary_diff_new_file() {
        let diff = r#"diff --git a/new_image.png b/new_image.png
new file mode 100644
Binary files /dev/null and b/new_image.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] added: new_image.png");
    }

    #[test]
    fn test_filter_binary_diff_deleted_file() {
        let diff = r#"diff --git a/old_image.png b/old_image.png
deleted file mode 100644
Binary files a/old_image.png and /dev/null differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] deleted: old_image.png");
    }

    #[test]
    fn test_filter_binary_diff_renamed_file() {
        let diff = r#"diff --git a/old_name.png b/new_name.png
similarity index 100%
rename from old_name.png
rename to new_name.png"#;

        let result = GitService::filter_binary_diff(diff);
        // リネームはテキストファイルとして扱われる（Binary filesがないため）
        assert!(result.contains("rename from old_name.png"));
    }

    #[test]
    fn test_filter_binary_diff_renamed_binary() {
        let diff = r#"diff --git a/old_name.png b/new_name.png
similarity index 90%
rename from old_name.png
rename to new_name.png
Binary files a/old_name.png and b/new_name.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "[Binary] renamed: old_name.png -> new_name.png");
    }

    #[test]
    fn test_filter_binary_diff_preserves_content_with_binary_keyword() {
        // "Binary"という文字列がコード内にある場合でも正しく処理
        let diff = r#"diff --git a/src/parser.rs b/src/parser.rs
index 1234567..abcdefg 100644
--- a/src/parser.rs
+++ b/src/parser.rs
@@ -1,3 +1,4 @@
+// Binary search implementation
 fn search() {}"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("Binary search implementation"));
    }

    // ============================================================
    // ScriptResult のテスト
    // ============================================================

    #[test]
    fn test_script_result_prefix() {
        let result = ScriptResult::Prefix("TICKET-123 ".to_string());
        assert_eq!(result, ScriptResult::Prefix("TICKET-123 ".to_string()));
    }

    #[test]
    fn test_script_result_empty() {
        let result = ScriptResult::Empty;
        assert_eq!(result, ScriptResult::Empty);
    }

    #[test]
    fn test_script_result_failed() {
        let result = ScriptResult::Failed;
        assert_eq!(result, ScriptResult::Failed);
    }

    #[test]
    fn test_script_result_equality() {
        assert_eq!(
            ScriptResult::Prefix("A".to_string()),
            ScriptResult::Prefix("A".to_string())
        );
        assert_ne!(
            ScriptResult::Prefix("A".to_string()),
            ScriptResult::Prefix("B".to_string())
        );
        assert_ne!(ScriptResult::Empty, ScriptResult::Failed);
    }

    // ============================================================
    // GitService 構造体のテスト
    // ============================================================

    #[test]
    fn test_git_service_new() {
        let service = GitService::new();
        // repo_pathが設定されていることを確認
        assert!(!service.repo_path.as_os_str().is_empty());
    }

    #[test]
    fn test_git_service_default() {
        let service = GitService::default();
        assert!(!service.repo_path.as_os_str().is_empty());
    }

    // ============================================================
    // Git リポジトリ操作のテスト（実際のリポジトリを使用）
    // ============================================================

    #[test]
    fn test_verify_repository_success() {
        // このテストは git-smart-commit リポジトリ内で実行される前提
        let service = GitService::new();
        let result = service.verify_repository();
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_current_branch() {
        let service = GitService::new();
        let branch = service.get_current_branch();
        // ブランチ名が取得できること（空でないこと）
        assert!(branch.is_some());
        assert!(!branch.unwrap().is_empty());
    }

    #[test]
    fn test_get_remote_url() {
        let service = GitService::new();
        let url = service.get_remote_url();
        // リモートURLが設定されている場合はgit-smart-commitを含む
        if let Some(remote) = url {
            assert!(remote.contains("git-smart-commit") || remote.contains("origin"));
        }
    }

    #[test]
    fn test_get_recent_commits() {
        let service = GitService::new();
        let commits = service.get_recent_commits(5);
        assert!(commits.is_ok());
        // このリポジトリにはコミットがあるはず
        let commits = commits.unwrap();
        assert!(!commits.is_empty());
    }

    #[test]
    fn test_get_recent_commits_limited() {
        let service = GitService::new();
        let commits = service.get_recent_commits(2);
        assert!(commits.is_ok());
        let commits = commits.unwrap();
        assert!(commits.len() <= 2);
    }

    // ============================================================
    // branch_exists のテスト
    // ============================================================

    #[test]
    fn test_branch_exists_main() {
        let service = GitService::new();
        // main または master ブランチが存在するはず
        let main_exists = service.branch_exists("main");
        let master_exists = service.branch_exists("master");
        assert!(main_exists || master_exists);
    }

    #[test]
    fn test_branch_exists_head() {
        let service = GitService::new();
        // HEAD は常に存在する
        assert!(service.branch_exists("HEAD"));
    }

    #[test]
    fn test_branch_exists_nonexistent() {
        let service = GitService::new();
        // 存在しないブランチ
        assert!(!service.branch_exists("nonexistent-branch-12345"));
    }

    #[test]
    fn test_branch_exists_with_origin_prefix() {
        let service = GitService::new();
        // origin/main または origin/master が存在する可能性
        let origin_main = service.branch_exists("origin/main");
        let origin_master = service.branch_exists("origin/master");
        // どちらかが存在するか、リモートがない場合は両方false
        // このテストはリモートの設定に依存するため、結果の検証は緩く
        // branch_exists が正常に動作することを確認（結果は環境依存）
        let _ = (origin_main, origin_master);
    }

    // ============================================================
    // get_merge_base のテスト
    // ============================================================

    #[test]
    fn test_get_merge_base_with_head() {
        let service = GitService::new();
        // HEAD と HEAD の merge-base は HEAD 自身
        let result = service.get_merge_base("HEAD", "HEAD");
        assert!(result.is_ok());
        let base = result.unwrap();
        // SHA-1 ハッシュは40文字
        assert_eq!(base.len(), 40);
    }

    #[test]
    fn test_get_merge_base_invalid_branch() {
        let service = GitService::new();
        let result = service.get_merge_base("nonexistent-branch", "HEAD");
        assert!(result.is_err());
    }

    // ============================================================
    // count_commits_from_base のテスト
    // ============================================================

    #[test]
    fn test_count_commits_from_base_same() {
        let service = GitService::new();
        // HEAD から HEAD までのコミット数は 0
        let result = service.count_commits_from_base("HEAD");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    // ============================================================
    // get_diff_from_base のテスト
    // ============================================================

    #[test]
    fn test_get_diff_from_base_same() {
        let service = GitService::new();
        // HEAD から HEAD までの差分は空
        let result = service.get_diff_from_base("HEAD");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ============================================================
    // ScriptResult Clone のテスト
    // ============================================================

    #[test]
    fn test_script_result_clone() {
        let original = ScriptResult::Prefix("TEST ".to_string());
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_script_result_debug() {
        let result = ScriptResult::Prefix("DEBUG ".to_string());
        let debug_str = format!("{:?}", result);
        assert!(debug_str.contains("Prefix"));
        assert!(debug_str.contains("DEBUG"));
    }

    // ============================================================
    // truncate_diff のテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_short_content() {
        let diff = "short content";
        let result = GitService::truncate_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exactly_at_limit() {
        // 10000文字ちょうどの場合は切り詰めない
        let diff: String = "a".repeat(10000);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exceeds_limit() {
        // 10001文字以上の場合は切り詰める（改行を含む現実的なdiff）
        let line = "This is a line of diff content\n";
        let diff: String = line.repeat(400); // 12000文字以上
        assert!(diff.chars().count() > MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        // 切り詰めメッセージが含まれることを確認
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_preserves_last_complete_line() {
        // 改行を含む長いテキスト
        let line = "This is a line of text\n";
        let diff: String = line.repeat(500); // 10500文字以上
        let result = GitService::truncate_diff(&diff);

        // 切り詰めメッセージが含まれる
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));

        // 最後の改行で切れている（中途半端な行がない）
        let lines: Vec<&str> = result.lines().collect();
        let last_content_line = lines
            .iter()
            .rev()
            .find(|l| !l.starts_with("...") && !l.is_empty());
        if let Some(line) = last_content_line {
            assert!(line.starts_with("This is a line"));
        }
    }

    // ============================================================
    // extract_file_path_from_diff_header のテスト
    // ============================================================

    #[test]
    fn test_extract_file_path_simple() {
        let header = "diff --git a/src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/main.rs"));
    }

    #[test]
    fn test_extract_file_path_nested() {
        let header = "diff --git a/path/to/nested/file.txt b/path/to/nested/file.txt";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path/to/nested/file.txt"));
    }

    #[test]
    fn test_extract_file_path_invalid_header() {
        let header = "not a diff header";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_no_a_prefix() {
        let header = "diff --git src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    // ============================================================
    // get_git_root のテスト
    // ============================================================

    #[test]
    fn test_get_git_root() {
        let service = GitService::new();
        let root = service.get_git_root();
        assert!(root.is_some());
        let root_path = root.unwrap();
        // .git ディレクトリが存在することを確認
        assert!(root_path.join(".git").exists());
    }

    // ============================================================
    // get_commit_diff_by_hash のテスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_with_head() {
        let service = GitService::new();
        // HEADは有効なコミット参照
        let result = service.get_commit_diff_by_hash("HEAD");
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_commit_diff_by_hash_invalid() {
        let service = GitService::new();
        // 存在しないハッシュ
        let result = service.get_commit_diff_by_hash("invalid_hash_xyz");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AppError::InvalidCommitHash(_)));
    }

    // ============================================================
    // filter_ignored_files のテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_no_ignore() {
        // ignoreパターンがない場合（実際にはGitignore構築が必要なので
        // filter_binary_diffと同様の動作を確認）
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }"#;

        // GitServiceでload_ignore_patternsがNoneを返す場合、
        // apply_all_filtersはfilter_ignored_filesをスキップする
        let service = GitService::new();

        // .git-sc-ignoreがない状態でテスト
        // この場合、apply_all_filtersはfilter_binary_diff + truncate_diffのみ適用
        let result = service.apply_all_filters(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("println"));
    }

    // ============================================================
    // is_auto_push_enabled のテスト
    // ============================================================

    #[test]
    fn test_is_auto_push_enabled_with_config_true() {
        let service = GitService::new();
        assert!(service.is_auto_push_enabled(Some(true)));
    }

    #[test]
    fn test_is_auto_push_enabled_with_config_false() {
        let service = GitService::new();
        assert!(!service.is_auto_push_enabled(Some(false)));
    }

    #[test]
    fn test_is_auto_push_enabled_with_config_none() {
        let service = GitService::new();
        // 未設定の場合は false
        assert!(!service.is_auto_push_enabled(None));
    }

    // ============================================================
    // filter_ignored_files のテスト (with actual ignore patterns)
    // ============================================================

    #[test]
    fn test_filter_ignored_files_with_patterns() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\nnode_modules/\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+new line
diff --git a/Cargo.lock b/Cargo.lock
index aaaaaaa..bbbbbbb 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,4 @@
+lock change"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(!result.contains("Cargo.lock"));
    }

    #[test]
    fn test_filter_ignored_files_empty_diff() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let result = GitService::filter_ignored_files("", &ignore);
        assert_eq!(result, "");
    }

    #[test]
    fn test_filter_ignored_files_all_ignored() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/Cargo.lock b/Cargo.lock
index aaaaaaa..bbbbbbb 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,4 @@
+lock change"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(!result.contains("Cargo.lock"));
    }

    #[test]
    fn test_filter_ignored_files_none_ignored() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.xyz\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+new line"#;

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("new line"));
    }

    // ============================================================
    // filter_binary_diff の追加エッジケーステスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_text_with_binary_keyword_in_filename() {
        let diff = r#"diff --git a/binary_helper.rs b/binary_helper.rs
index 1234567..abcdefg 100644
--- a/binary_helper.rs
+++ b/binary_helper.rs
@@ -1 +1,2 @@
+// binary helper code"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("binary_helper.rs"));
        assert!(result.contains("binary helper code"));
    }

    #[test]
    fn test_filter_binary_diff_multiple_text_files() {
        let diff = r#"diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1,2 @@
+// a
diff --git a/src/b.rs b/src/b.rs
index 333..444 100644
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1,2 @@
+// b
diff --git a/src/c.rs b/src/c.rs
index 555..666 100644
--- a/src/c.rs
+++ b/src/c.rs
@@ -1 +1,2 @@
+// c"#;

        let result = GitService::filter_binary_diff(diff);
        assert!(result.contains("src/a.rs"));
        assert!(result.contains("src/b.rs"));
        assert!(result.contains("src/c.rs"));
    }

    // ============================================================
    // truncate_diff の追加テスト
    // ============================================================

    #[test]
    fn test_truncate_diff_no_newlines() {
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_empty() {
        let result = GitService::truncate_diff("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_truncate_diff_no_newlines_content() {
        // 改行なしで制限超過の場合、rfind('\n')がNoneを返しelse分岐に入る
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 500);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // rfind('\n') == None なので、truncated全体 + トランケートメッセージ
        assert!(result.starts_with(&"a".repeat(MAX_DIFF_CHARS)));
    }

    // ============================================================
    // filter_ignored_files: diff前のプリアンブルテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_with_preamble() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let ignore_path = temp_dir.path().join(".git-sc-ignore");
        std::fs::write(&ignore_path, "*.lock\n").unwrap();

        let mut builder = GitignoreBuilder::new(temp_dir.path());
        builder.add(&ignore_path);
        let ignore = builder.build().unwrap();

        // diff --git ヘッダーの前にプリアンブルテキストがある場合
        let diff = "some preamble\nanother preamble line\ndiff --git a/src/main.rs b/src/main.rs\nindex 123..456 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1,2 @@\n+new line";

        let result = GitService::filter_ignored_files(diff, &ignore);
        // プリアンブルが保存されていること
        assert!(result.contains("some preamble"));
        assert!(result.contains("another preamble line"));
        // diffブロックも保存されていること
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn test_apply_all_filters_text_only() {
        let gs = GitService::default();
        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let result = gs.apply_all_filters(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_binary_excluded() {
        let gs = GitService::default();
        let diff = "diff --git a/image.png b/image.png\nnew file mode 100644\nBinary files /dev/null and b/image.png differ\ndiff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let result = gs.apply_all_filters(diff);
        assert!(result.contains("[Binary]"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_empty() {
        let gs = GitService::default();
        let result = gs.apply_all_filters("");
        assert!(result.is_empty());
    }

    // ============================================================
    // extract_file_path_from_diff_header: ルートファイルのテスト
    // ============================================================

    #[test]
    fn test_extract_file_path_root_file() {
        let header = "diff --git a/Cargo.toml b/Cargo.toml";
        assert_eq!(
            GitService::extract_file_path_from_diff_header(header),
            Some("Cargo.toml")
        );
    }

    // ============================================================
    // get_commit_message_by_hash のテスト
    // ============================================================

    #[test]
    fn test_get_commit_message_by_hash_head() {
        let service = GitService::new();
        let result = service.get_commit_message_by_hash("HEAD");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_get_commit_message_by_hash_invalid() {
        let service = GitService::new();
        let result = service.get_commit_message_by_hash("0000000000000000000000000000000000000000");
        assert!(result.is_err());
    }
}
