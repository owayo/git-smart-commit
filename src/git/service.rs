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

    /// ステージ済みのdiffを取得
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

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// アンステージのdiffを取得
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

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

    /// 直前のコミットのdiffを取得
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

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
