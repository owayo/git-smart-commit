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

/// テスト用ヘルパー: フェイク Codex CLI を配置した PATH を作成
#[cfg(unix)]
fn setup_fake_codex_path(dir: &TempDir) -> String {
    let bin_dir = dir.path().join("fake-bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let script_path = bin_dir.join("codex");
    let script_body = r#"#!/bin/sh
out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ] || [ "$1" = "--output-last-message" ]; then
    shift
    out="$1"
  fi
  shift || break
done

if [ -z "$out" ]; then
  echo "missing codex output file" >&2
  exit 2
fi

cat >/dev/null
echo "codex"
echo "bad transcript line"
printf '%s\n' "fix: use codex output file" > "$out"
"#;

    std::fs::write(&script_path, script_body).unwrap();

    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let current_path = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", bin_dir.display(), current_path)
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
// .git-sc-ignore が読めないときは差分を送らず中止する (fail-closed)
// ============================================================

#[test]
fn test_unreadable_ignore_file_aborts_instead_of_sending_diff() {
    // 除外設定が適用できない状態のまま続行すると、除外したかったファイルが
    // そのまま AI プロバイダーへ送られる。CLI の出口でも中止することを固定する。
    let dir = setup_git_repo_with_commit();

    // `.git-sc-ignore` をディレクトリにすると読み取りは必ず失敗する
    // (パーミッション 000 は root 実行だと読めてしまい環境依存になる)。
    std::fs::create_dir(dir.path().join(".git-sc-ignore")).unwrap();

    std::fs::write(dir.path().join("secret.env"), "TOKEN=abc\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "secret.env"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // --dry-run でもコミットは作らないが、差分取得の時点で失敗するべき
    git_sc!()
        .arg("--dry-run")
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains(".git-sc-ignore"));
}

// ============================================================
// rebase 進行中は reword を拒否する
// ============================================================

#[test]
fn test_reword_refuses_while_another_rebase_is_in_progress() {
    // reword は失敗した rebase を必ず `git rebase --abort` で終わらせるため、
    // ユーザーの rebase の上で起動すると解決作業中の内容を破棄してしまう。
    // また、rebase 中は HEAD が detached なので、素通しすると原因と無関係な
    // InvalidRewordTarget が表示される。CLI の出口で正しい理由が出ることを固定する。
    let dir = setup_git_repo_with_commit();
    let repo = dir.path();

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
    };

    let base_branch = String::from_utf8(git(&["branch", "--show-current"]).stdout)
        .unwrap()
        .trim()
        .to_string();

    // side ブランチと元ブランチで同じ行を書き換え、rebase で必ず衝突させる
    git(&["checkout", "-b", "side"]);
    std::fs::write(repo.join("conflict.txt"), "side\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "side change"]);

    git(&["checkout", &base_branch]);
    std::fs::write(repo.join("conflict.txt"), "main\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-m", "main change"]);

    let target = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();

    // ユーザーの rebase を衝突で停止させる
    let rebase = git(&["rebase", "side"]);
    assert!(
        !rebase.status.success(),
        "衝突で停止させたいので rebase は失敗するはず"
    );

    git_sc!()
        .args(["--reword", &target, "--dry-run"])
        .current_dir(repo)
        .assert()
        .failure()
        .stderr(predicate::str::contains("rebaseが進行中です"));

    // ユーザーの rebase が生き残っていることが本題
    let status = git(&["status", "--porcelain=v1", "-b"]);
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status.contains("HEAD (no branch)"),
        "git-sc がユーザーの進行中 rebase を破棄してはいけない: {status}"
    );
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
    let multibyte_hash = "あいうえおかきく"; // 24バイト（8文字 * 3バイト）

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

#[cfg(unix)]
#[test]
fn test_codex_provider_uses_output_file_not_transcript_stdout() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_codex_path(&dir);
    stage_change(&dir, "# Test\ncodex\n");

    git_sc!()
        .args(["--quiet", "--dry-run", "--provider", "codex"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("fix: use codex output file"))
        .stdout(predicate::str::contains("bad transcript line").not())
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
fn test_squash_with_existing_staged_changes_fails_before_reset() {
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
        .args(["checkout", "-b", "feature/staged-squash"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit_change(&dir, "# Test\nfeature\n", "feature commit");

    std::fs::write(dir.path().join("staged.txt"), "staged\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "staged.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    git_sc!()
        .args(["--quiet", "--squash", &base_branch, "--yes"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "squash を実行する前に staged 変更",
        ));

    let staged_output = std::process::Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&staged_output.stdout).trim(),
        "staged.txt"
    );

    let message_output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%s"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&message_output.stdout).trim(),
        "feature commit"
    );
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
fn test_generate_for_combined_diff_is_truncated_to_max_chars() {
    // 回帰テスト: --generate-for は複数コミットの diff を結合する。
    // 各コミットの diff は個別に MAX_DIFF_CHARS(10000) 以下でも、結合後の総量は
    // 上限を超えうる。結合後にも切り詰めが効くことを、デバッグ出力（stderr）に現れる
    // 切り詰めマーカーで確認する。修正前は結合後に切り詰めず、マーカーが出なかった。
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    // 各コミットで別ファイルを追加する。各 diff は約6KB（10000字未満）だが、
    // 2コミット結合で約12KB となり 10000字上限を超える。
    let mut content1 = String::new();
    let mut content2 = String::new();
    for i in 0..150 {
        content1.push_str(&format!("f1 line {i:04}: lorem ipsum dolor sit amet\n"));
        content2.push_str(&format!("f2 line {i:04}: lorem ipsum dolor sit amet\n"));
    }

    std::fs::write(dir.path().join("file1.txt"), &content1).unwrap();
    std::process::Command::new("git")
        .args(["add", "file1.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "add file1"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash1 = head_hash(&dir);

    std::fs::write(dir.path().join("file2.txt"), &content2).unwrap();
    std::process::Command::new("git")
        .args(["add", "file2.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "add file2"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash2 = head_hash(&dir);

    git_sc!()
        .args(["--generate-for", &hash1, &hash2, "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "diff truncated: exceeded 10000 characters",
        ));
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

/// `rebase.abbreviateCommands=true` でも reword が成功することを確認する。
///
/// 以前の実装は GIT_SEQUENCE_EDITOR で `^pick` のみを置換していたため、
/// Git が todo を `p <hash>` 形式（短縮形）で出力する設定下では置換が当たらず、
/// rebase は成功扱いになる一方でメッセージは元のまま、という静かな不具合があった。
/// 修正後は rebase 起動時に `-c rebase.abbreviateCommands=false` を明示するため、
/// この設定があっても reword が機能する必要がある。
/// detached HEAD 状態で prefix_scripts の URL パターンが一致するとき、
/// `Running prefix script for...` を表示した直後に黙ってフォールスルーしないこと
/// （スキップ理由を明示することで黙ったフォールスルーを防ぐ）を検証する。
#[cfg(unix)]
#[test]
fn test_prefix_script_skips_with_notice_on_detached_head() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    // 2 つ目のコミットを追加して detached HEAD で戻れるようにする
    commit_change(&dir, "# Test\nsecond\n", "second commit");
    let detach_hash = head_hash(&dir);

    // リモート URL を設定（prefix_scripts は url_pattern で照合するため必要）
    std::process::Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://example.com/test/repo.git",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // ダミーの prefix script を配置（本テストでは実行されないことを確認するため、
    // 実行されてしまうと検出できるよう exit 2 で失敗させる）。
    let script_path = dir.path().join("prefix.sh");
    std::fs::write(
        &script_path,
        "#!/bin/sh\necho 'SHOULD NOT RUN' >&2\nexit 2\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    // プロジェクト設定 .git-sc に prefix_scripts を1つ登録
    let project_config = format!(
        r#"providers = ["opencode"]

[[prefix_scripts]]
url_pattern = "example\\.com"
script = "{}"
"#,
        script_path.display()
    );
    std::fs::write(dir.path().join(".git-sc"), project_config).unwrap();

    // detached HEAD 状態にする
    std::process::Command::new("git")
        .args(["checkout", "--detach", &detach_hash])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // ステージ変更を作る（dry-run で生成のみ）
    std::fs::write(dir.path().join("changed.txt"), "changed\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "changed.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    git_sc!()
        .args(["--dry-run", "--provider", "opencode"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        // URL マッチを試みたメッセージが表示される
        .stdout(predicate::str::contains("Running prefix script"))
        // スキップ理由が明示される（黙ったフォールスルーではない）
        .stdout(predicate::str::contains("branch name unavailable"))
        // フェイク opencode の生成メッセージが表示される（フォールスルー成功）
        .stdout(predicate::str::contains("feat: quiet integration test"))
        // スクリプトは実行されない（実行されていれば stderr に "SHOULD NOT RUN" が出る）
        .stderr(predicate::str::contains("SHOULD NOT RUN").not());
}

#[test]
fn test_reword_succeeds_with_rebase_abbreviate_commands_true() {
    let dir = setup_git_repo_with_commit();
    let path = setup_fake_opencode_path(&dir);

    // リポジトリ単位で rebase.abbreviateCommands=true を設定
    std::process::Command::new("git")
        .args(["config", "rebase.abbreviateCommands", "true"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // reword 対象となる最も古いコミットのハッシュ（initial commit）を控える
    let old_hash = head_hash(&dir);

    // reword 対象より新しいコミットを 1 件追加（reword は HEAD 以外を対象にする必要がある）
    commit_change(&dir, "# Test\nsecond\n", "second commit");

    git_sc!()
        .args(["--reword", &old_hash, "--yes"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success();

    // 最も古いコミットのメッセージが書き換わっていることを確認
    // ※ rebase によりハッシュは変わるため、件名でアサートする
    let log = std::process::Command::new("git")
        .args(["log", "--format=%s"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let messages = String::from_utf8_lossy(&log.stdout);

    // fake opencode は "feat: quiet integration test" を返す → AI 生成側のメッセージで上書き
    assert!(
        messages.contains("feat: quiet integration test"),
        "reword 後のログに新メッセージが含まれていない: {}",
        messages
    );
    // 元のメッセージは消える
    assert!(
        !messages.contains("initial commit"),
        "reword 後のログに元メッセージが残存: {}",
        messages
    );
}

/// squash で `reset --soft` 後の `git commit` がフックに拒否された場合、
/// ブランチが merge-base まで巻き戻されたまま放置されず、元の HEAD へ
/// 復旧されることを確認する(復旧がないと元のコミット列は reflog にしか残らない)
#[cfg(unix)]
#[test]
fn test_squash_commit_failure_restores_original_head() {
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
        .args(["checkout", "-b", "feature/squash-recovery"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit_change(&dir, "# Test\nfeature1\n", "feature commit 1");
    commit_change(&dir, "# Test\nfeature2\n", "feature commit 2");

    let head_output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let original_head = String::from_utf8_lossy(&head_output.stdout)
        .trim()
        .to_string();

    // squash 本体の commit だけを拒否する pre-commit フックを配置
    // (テスト用コミットの作成後に配置するため、上記の commit_change には影響しない)
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
    let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&hook_path, perms).unwrap();

    git_sc!()
        .args(["--quiet", "--squash", &base_branch, "--yes"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .failure();

    // ブランチが元の HEAD に復旧していること
    let restored_head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&restored_head.stdout).trim(),
        original_head,
        "squash 失敗後にブランチが元の HEAD へ復旧されていない"
    );

    // staged 状態も実行前(なし)に戻っていること
    let staged_output = std::process::Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&staged_output.stdout).trim(),
        "",
        "squash 失敗後に staged 変更が残存している"
    );
}

/// --generate-for は「stdout には生成メッセージのみ」が契約。
/// --debug を併用してもデバッグ出力(設定・コマンド・ストリーミング)は
/// すべて stderr へ出力され、stdout がメッセージのみであることを確認する
#[test]
fn test_generate_for_debug_keeps_stdout_message_only() {
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
        .args(["--generate-for", &head_hash, "--debug"])
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("XDG_CONFIG_HOME", dir.path())
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::eq("feat: quiet integration test\n"))
        .stderr(predicate::str::contains("=== DEBUG"));
}
