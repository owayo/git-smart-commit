use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use tempfile::TempDir;

/// テスト用ヘルパー: git-sc コマンドを取得
macro_rules! git_sc {
    () => {
        cargo_bin_cmd!("git-sc")
    };
}

/// テスト用ヘルパー: 一時的なGitリポジトリを作成
fn setup_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

/// テスト用ヘルパー: 初期コミットを含むGitリポジトリを作成
fn setup_git_repo_with_commit() -> TempDir {
    let dir = setup_git_repo();
    std::fs::write(dir.path().join("README.md"), "# Test").unwrap();
    std::process::Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "initial commit"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

// ============================================================
// --help のテスト
// ============================================================

#[test]
fn test_help_flag() {
    git_sc!()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("AI-powered smart commit"));
}

#[test]
fn test_short_help_flag() {
    git_sc!()
        .arg("-h")
        .assert()
        .success()
        .stdout(predicate::str::contains("AI-powered smart commit"));
}

// ============================================================
// --version のテスト
// ============================================================

#[test]
fn test_version_flag() {
    git_sc!()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("git-sc"));
}

#[test]
fn test_short_version_flag() {
    git_sc!()
        .arg("-V")
        .assert()
        .success()
        .stdout(predicate::str::contains("git-sc"));
}

// ============================================================
// init サブコマンドのテスト
// ============================================================

#[test]
fn test_init_help() {
    git_sc!()
        .args(["init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialize"));
}

// ============================================================
// dry-run のテスト（ステージ済み変更がない場合）
// ============================================================

#[test]
fn test_dry_run_no_staged_changes() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .arg("--dry-run")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("ステージ済みの変更がありません"));
}

// ============================================================
// 非Gitリポジトリでの実行テスト
// ============================================================

#[test]
fn test_run_outside_git_repo() {
    let dir = TempDir::new().unwrap();

    // Gitリポジトリ以外で実行すると正常終了（exit 0）
    git_sc!().current_dir(dir.path()).assert().success();
}

// ============================================================
// --all で変更がない場合のテスト
// ============================================================

#[test]
fn test_stage_all_no_changes() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--all", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("変更がありません"));
}

// ============================================================
// 排他オプションのテスト
// ============================================================

#[test]
fn test_generate_for_with_amend_conflict() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--generate-for", "HEAD", "--amend"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--generate-for"));
}

#[test]
fn test_generate_for_with_squash_conflict() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--generate-for", "HEAD", "--squash", "main"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--generate-for"));
}

// ============================================================
// --quiet のテスト
// ============================================================

#[test]
fn test_quiet_stage_all_no_changes() {
    let dir = setup_git_repo_with_commit();

    // --quiet + --all + --dry-run で変更なしの場合、出力が抑制される
    git_sc!()
        .args(["--quiet", "--all", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success();
}
