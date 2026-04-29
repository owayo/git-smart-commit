use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
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

/// テスト用ヘルパー: フェイク opencode を配置した PATH を作成
fn setup_fake_opencode_path(dir: &TempDir) -> String {
    let bin_dir = dir.path().join("fake-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    #[cfg(windows)]
    let script_path = bin_dir.join("opencode.cmd");
    #[cfg(not(windows))]
    let script_path = bin_dir.join("opencode");

    #[cfg(windows)]
    let script_body = "@echo off\r\necho feat: quiet integration test\r\n";
    #[cfg(not(windows))]
    let script_body = "#!/bin/sh\necho \"feat: quiet integration test\"\n";

    std::fs::write(&script_path, script_body).unwrap();

    #[cfg(unix)]
    {
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();
    }

    let current_path = std::env::var("PATH").unwrap_or_default();
    let separator = if cfg!(windows) { ";" } else { ":" };
    format!("{}{}{}", bin_dir.display(), separator, current_path)
}

fn resolve_real_git_path() -> String {
    #[cfg(windows)]
    let output = std::process::Command::new("where")
        .arg("git")
        .output()
        .unwrap();
    #[cfg(not(windows))]
    let output = std::process::Command::new("which")
        .arg("git")
        .output()
        .unwrap();

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap()
        .trim()
        .to_string()
}

/// テスト用ヘルパー: フェイク opencode とローカライズ済み git ラッパーを配置した PATH を作成
fn setup_fake_opencode_and_localized_git_path(dir: &TempDir) -> String {
    let bin_dir = dir.path().join("fake-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    #[cfg(windows)]
    let opencode_path = bin_dir.join("opencode.cmd");
    #[cfg(not(windows))]
    let opencode_path = bin_dir.join("opencode");

    #[cfg(windows)]
    let opencode_body = "@echo off\r\necho feat: quiet integration test\r\n";
    #[cfg(not(windows))]
    let opencode_body = "#!/bin/sh\necho \"feat: quiet integration test\"\n";

    std::fs::write(&opencode_path, opencode_body).unwrap();

    #[cfg(windows)]
    let git_path = bin_dir.join("git.cmd");
    #[cfg(not(windows))]
    let git_path = bin_dir.join("git");

    let real_git = resolve_real_git_path();

    #[cfg(windows)]
    let git_body = format!(
        "@echo off\r\n\
if \"%1\"==\"log\" if \"%2\"==\"--format=%s\" (\r\n\
  >&2 echo fatal: このブランチにはまだコミットがありません\r\n\
  exit /b 128\r\n\
)\r\n\
\"{}\" %*\r\n",
        real_git
    );
    #[cfg(not(windows))]
    let git_body = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"log\" ] && [ \"$2\" = \"--format=%s\" ]; then\n\
  echo \"fatal: このブランチにはまだコミットがありません\" >&2\n\
  exit 128\n\
fi\n\
exec \"{}\" \"$@\"\n",
        real_git
    );

    std::fs::write(&git_path, git_body).unwrap();

    #[cfg(unix)]
    {
        let mut opencode_perms = std::fs::metadata(&opencode_path).unwrap().permissions();
        opencode_perms.set_mode(0o755);
        std::fs::set_permissions(&opencode_path, opencode_perms).unwrap();

        let mut git_perms = std::fs::metadata(&git_path).unwrap().permissions();
        git_perms.set_mode(0o755);
        std::fs::set_permissions(&git_path, git_perms).unwrap();
    }

    let current_path = std::env::var("PATH").unwrap_or_default();
    let separator = if cfg!(windows) { ";" } else { ":" };
    format!("{}{}{}", bin_dir.display(), separator, current_path)
}

fn stage_change(dir: &TempDir, contents: &str) {
    std::fs::write(dir.path().join("README.md"), contents).unwrap();
    std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir.path())
        .output()
        .unwrap();
}

fn commit_change(dir: &TempDir, contents: &str, message: &str) {
    stage_change(dir, contents);
    std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir.path())
        .output()
        .unwrap();
}

fn head_hash(dir: &TempDir) -> String {
    let hash_output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string()
}

fn setup_git_repo_with_merge_commit() -> (TempDir, String) {
    let dir = setup_git_repo_with_commit();
    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let base_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/reword-merge"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("feature.txt"), "feature\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "feature.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "feature commit"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    std::process::Command::new("git")
        .args(["checkout", &base_branch])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("main.txt"), "main\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "main.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "main commit"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let merge_output = std::process::Command::new("git")
        .args([
            "merge",
            "--no-ff",
            "feature/reword-merge",
            "-m",
            "merge feature",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        merge_output.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&merge_output.stderr)
    );

    let merge_hash = head_hash(&dir);
    (dir, merge_hash)
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
        .stdout(predicate::str::contains("AI"));
}

#[test]
fn test_short_help_flag() {
    git_sc!()
        .arg("-h")
        .assert()
        .success()
        .stdout(predicate::str::contains("AI"));
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
        .stdout(predicate::str::contains("設定ファイルを生成"));
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

#[test]
fn test_amend_with_squash_conflict() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--amend", "--squash", "main"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--amend"));
}

#[test]
fn test_amend_with_reword_conflict() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--amend", "--reword", "HEAD"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--amend"));
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
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_quiet_squash_on_base_branch_suppresses_progress_output() {
    let dir = setup_git_repo_with_commit();
    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let current_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    git_sc!()
        .args(["--quiet", "--squash", &current_branch])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "ベースブランチ上では squash できません",
        ));
}

#[test]
fn test_quiet_reword_invalid_hash_suppresses_progress_output() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--quiet", "--reword", "invalid_hash_xyz"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("無効なコミットハッシュ"));
}

#[test]
fn test_reword_multibyte_hash_does_not_panic() {
    // 表示用の短縮ハッシュ計算で文字境界違反によるパニックが起きないことを確認する。
    // マルチバイト文字を含む長いハッシュ（合計バイト長 > 7）を渡しても、
    // バリデーションエラーで終了する必要がある。
    let dir = setup_git_repo_with_commit();
    let multibyte_hash = "あいうえおかきく"; // 24 bytes (8 chars * 3 bytes)

    git_sc!()
        .args(["--reword", multibyte_hash])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("無効なコミットハッシュ"));
}

#[test]
fn test_quiet_reword_hash_outside_head_history_fails() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let base_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/reword-outside"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit_change(&dir, "# Test\nfeature\n", "feature commit");
    let outside_hash = head_hash(&dir);

    std::process::Command::new("git")
        .args(["checkout", &base_branch])
        .current_dir(dir.path())
        .output()
        .unwrap();

    git_sc!()
        .args(["--quiet", "--reword", &outside_hash, "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "無効なreword対象です。有効なコミットハッシュを指定してください。",
        ));
}

#[test]
fn test_quiet_reword_merge_commit_target_fails() {
    let (dir, merge_hash) = setup_git_repo_with_merge_commit();

    git_sc!()
        .args(["--quiet", "--reword", &merge_hash, "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "指定範囲にマージコミットが含まれています。rewordはマージコミットを含む範囲では使用できません。",
        ));
}

#[test]
fn test_quiet_dry_run_with_ai_generation_suppresses_provider_output() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    stage_change(&dir, "# Test\nupdated\n");

    git_sc!()
        .args(["--quiet", "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_empty_repo_with_localized_git_stderr_still_generates_message() {
    let dir = setup_git_repo();
    let path = setup_fake_opencode_and_localized_git_path(&dir);

    std::fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    git_sc!()
        .args(["--quiet", "--dry-run", "--provider", "opencode"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_quiet_amend_dry_run_with_ai_generation_suppresses_provider_output() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    commit_change(&dir, "# Test\nsecond\n", "second commit");

    git_sc!()
        .args(["--quiet", "--amend", "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_amend_dry_run_with_single_commit_repo() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    git_sc!()
        .args(["--amend", "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"));
}

#[test]
fn test_quiet_reword_dry_run_with_ai_generation_suppresses_provider_output() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    let hash_output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let head_hash = String::from_utf8_lossy(&hash_output.stdout)
        .trim()
        .to_string();

    git_sc!()
        .args(["--quiet", "--reword", &head_hash, "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_quiet_squash_dry_run_with_ai_generation_suppresses_provider_output() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let base_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/quiet-test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit_change(&dir, "# Test\nfeature\n", "feature commit");

    git_sc!()
        .args(["--quiet", "--squash", &base_branch, "--dry-run"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("feat:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn test_agent_context_is_included_in_amend_debug_prompt() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    commit_change(&dir, "# Test\nsecond\n", "second commit");

    git_sc!()
        .args(["--amend", "--dry-run", "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("CLAW_HOOKS_AGENT_MESSAGE", "agent-intent-amend")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("agent-intent-amend"));
}

#[test]
fn test_agent_context_is_included_in_reword_debug_prompt() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    let hash = head_hash(&dir);

    git_sc!()
        .args(["--reword", &hash, "--dry-run", "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("CLAW_HOOKS_AGENT_MESSAGE", "agent-intent-reword")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("agent-intent-reword"));
}

#[test]
fn test_agent_context_is_included_in_squash_debug_prompt() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    let branch_output = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let base_branch = String::from_utf8_lossy(&branch_output.stdout)
        .trim()
        .to_string();

    std::process::Command::new("git")
        .args(["checkout", "-b", "feature/context-test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit_change(&dir, "# Test\nfeature\n", "feature commit");

    git_sc!()
        .args(["--squash", &base_branch, "--dry-run", "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("CLAW_HOOKS_AGENT_MESSAGE", "agent-intent-squash")
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("agent-intent-squash"));
}

#[test]
fn test_agent_context_is_included_in_generate_for_debug_prompt() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);
    let hash = head_hash(&dir);

    git_sc!()
        .args(["--generate-for", &hash, "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .env("CLAW_HOOKS_AGENT_MESSAGE", "agent-intent-generate-for")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("agent-intent-generate-for"));
}

#[test]
fn test_generate_for_with_reword_conflict() {
    let dir = setup_git_repo_with_commit();

    git_sc!()
        .args(["--generate-for", "HEAD", "--reword", "HEAD"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--generate-for"));
}
