use std::path::PathBuf;
use std::process::Command;

use crate::error::AppError;

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
                    // ループでインクリメントされるのでデクリメント
                    if i < lines.len() {
                        i -= 1;
                    }
                }
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

    /// アンステージのdiffを取得（バイナリファイルを除外）
    pub fn get_unstaged_diff(&self) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(["diff"])
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
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}
