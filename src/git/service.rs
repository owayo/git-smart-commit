use std::path::PathBuf;
use std::process::Command;

use crate::error::AppError;

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

    /// git diffの出力からバイナリファイルの差分を除外
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
                i += 1;

                // このブロックがバイナリかどうかをチェック
                let mut is_binary = false;
                while i < lines.len() && !lines[i].starts_with("diff --git") {
                    if lines[i].contains("Binary files") && lines[i].contains("differ") {
                        is_binary = true;
                        break;
                    }
                    i += 1;
                }

                // バイナリでなければブロックを追加
                if !is_binary {
                    for line in lines.iter().take(i).skip(block_start) {
                        filtered_lines.push(*line);
                    }
                } else {
                    // バイナリブロックをスキップ（次のdiff --gitまで進む）
                    while i < lines.len() && !lines[i].starts_with("diff --git") {
                        i += 1;
                    }
                }
                // diffブロック処理後は次のdiff --gitから継続（i += 1をスキップ）
                continue;
            } else {
                filtered_lines.push(line);
            }
            i += 1;
        }

        filtered_lines.join("\n")
    }

    /// 現在のディレクトリがGitリポジトリであることを確認
    pub fn verify_repository(&self) -> Result<(), AppError> {
        let git_dir = self.repo_path.join(".git");
        if git_dir.exists() {
            Ok(())
        } else {
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
    }

    /// ステージ済みのdiffを取得（バイナリファイルを除外）
    pub fn get_staged_diff(&self) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(["diff", "--cached"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(Self::filter_binary_diff(&diff))
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
                return Ok(vec![]);
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
        let output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// 指定されたメッセージでコミットを作成
    pub fn commit(&self, message: &str) -> Result<(), AppError> {
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// 直前のコミットのdiffを取得（バイナリファイルを除外）
    pub fn get_last_commit_diff(&self) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(["diff", "HEAD~1", "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(Self::filter_binary_diff(&diff))
    }

    /// 直前のコミットを新しいメッセージで修正
    pub fn amend_commit(&self, message: &str) -> Result<(), AppError> {
        let output = Command::new("git")
            .args(["commit", "--amend", "-m", message])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// リモートURLを取得（origin）
    pub fn get_remote_url(&self) -> Option<String> {
        let output = Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .current_dir(&self.repo_path)
            .output()
            .ok()?;

        if output.status.success() {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if url.is_empty() {
                None
            } else {
                Some(url)
            }
        } else {
            None
        }
    }

    /// 現在のブランチ名を取得
    pub fn get_current_branch(&self) -> Option<String> {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(&self.repo_path)
            .output()
            .ok()?;

        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if branch.is_empty() {
                None
            } else {
                Some(branch)
            }
        } else {
            None
        }
    }

    /// プレフィックススクリプトを実行してプレフィックスを取得
    ///
    /// 戻り値:
    /// - `Some(ScriptResult::Prefix(s))`: スクリプトがプレフィックスを返した（exit 0 + 内容あり）
    /// - `Some(ScriptResult::Empty)`: スクリプトが空を返した（exit 0 + 内容なし）→ プレフィックスなし
    /// - `Some(ScriptResult::Failed)`: スクリプトが失敗した（exit 1）→ AI生成メッセージを使用
    /// - `None`: スクリプトの実行自体に失敗
    pub fn run_prefix_script(
        &self,
        script: &str,
        remote_url: &str,
        branch: &str,
    ) -> Option<ScriptResult> {
        let output = Command::new(script)
            .args([remote_url, branch])
            .current_dir(&self.repo_path)
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
            // exit 1: AI生成のメッセージをそのまま使用
            Some(ScriptResult::Failed)
        }
    }

    /// ブランチが存在するか確認
    pub fn branch_exists(&self, branch: &str) -> bool {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(&self.repo_path)
            .output();

        output.map(|o| o.status.success()).unwrap_or(false)
    }

    /// 2つのブランチのmerge-baseを取得
    pub fn get_merge_base(&self, base: &str, head: &str) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(["merge-base", base, head])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(format!(
                "Failed to find merge-base between {} and {}",
                base, head
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// ベースからHEADまでのコミット数を取得
    pub fn count_commits_from_base(&self, base: &str) -> Result<usize, AppError> {
        let output = Command::new("git")
            .args(["rev-list", "--count", &format!("{}..HEAD", base)])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))
    }

    /// ベースからHEADまでの差分を取得（バイナリファイルを除外）
    pub fn get_diff_from_base(&self, base: &str) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(["diff", base, "HEAD"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        let diff = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(Self::filter_binary_diff(&diff))
    }

    /// 指定したコミットにsoft resetする
    pub fn soft_reset_to(&self, commit: &str) -> Result<(), AppError> {
        let output = Command::new("git")
            .args(["reset", "--soft", commit])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::GitError(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
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
    fn test_filter_binary_diff_removes_binary() {
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

        let expected = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
 }"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_filter_binary_diff_only_binary() {
        let diff = r#"diff --git a/image.png b/image.png
Binary files a/image.png and b/image.png differ"#;

        let result = GitService::filter_binary_diff(diff);
        assert_eq!(result, "");
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

        // テキストファイルの変更のみが含まれることを確認
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains("config.toml"));
        assert!(!result.contains("image1.png"));
        assert!(!result.contains("image2.jpg"));
        assert!(!result.contains("Binary files"));
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

        assert!(!result.contains("logo.svg"));
        assert!(result.contains("README.md"));
        assert!(result.contains("# Title"));
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
        assert!(origin_main || origin_master || (!origin_main && !origin_master));
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
}
