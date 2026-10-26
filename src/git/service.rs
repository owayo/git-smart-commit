use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::AppError;

/// 差分の最大文字数
const MAX_DIFF_CHARS: usize = 10000;

/// 除外対象はパスだけを残し、本文を渡していないことを AI に知らせる。
const IGNORED_CONTENT_NOTICE: &str = "[content omitted by .git-sc-ignore]";

struct DiffFileExcerpt {
    overview: String,
    content: String,
    body_omitted: bool,
}

/// diff の出力形式をユーザーの Git 設定から切り離すための引数
///
/// `.git-sc-ignore` の除外は `diff --git a/... b/...` 行からパスを読んで判定する。
/// ところが次の設定はその行の形を変えてしまい、パスが読めないと除外判定は
/// 「対象なし」に倒れる ⇒ 隠したかったファイルがそのまま AI へ送られる。
///
/// - `diff.noprefix = true` … `diff --git path path`(`a/` `b/` が付かない)
/// - `diff.mnemonicPrefix = true` … `diff --git c/path i/path`
/// - `diff.srcPrefix` / `diff.dstPrefix` … 任意の文字列に差し替わる
/// - `color.ui = always` … 行頭に ANSI エスケープが入り `diff --git` で始まらなくなる
/// - `diff.external` … 出力そのものが別形式に差し替わる(`diff --git` 行ごと消える)
///
/// `diff.relative = true` だけは毛色が違い、**パスの基準そのもの**を変える。git は
/// カレントディレクトリで起動する(`GitService::repo_path` は Git ルートではなく cwd)
/// ため、サブディレクトリから実行すると
///
/// - ヘッダーのパスが Git ルート相対ではなく cwd 相対になる
///   (ルートの `.git-sc-ignore` に書いた `src/secrets/**` が `secrets/key.txt` と
///   照合されて一致せず、除外が外れる)
/// - **cwd の外にある変更が diff から丸ごと消える**
///   (コミットされる内容の一部しか見ずにメッセージを書くことになる)
///
/// の 2 つが同時に起きる。`--no-relative` で常に Git ルート基準に固定する。
///
/// サブモジュール関連の 2 つも同じ 2 系統に分かれる。
///
/// - `diff.ignoreSubmodules = all`(および `.gitmodules` の
///   `submodule.<name>.ignore = all`)は `diff.relative` と同じ「変更が diff から
///   丸ごと消える」側で、サブモジュールのポインタ変更(160000 エントリ)が見えなくなる。
///   `.gitmodules` は**リポジトリにコミットされるファイル**なので、ローカル設定の
///   ミスを必要とせず clone しただけで成立する。`--ignore-submodules=none` で固定する。
/// - `diff.submodule = log` / `= diff` は「ヘッダー形状が変わる」側で、ブロック開始行が
///   `Submodule <path> <a>..<b>:` になり `diff --git` で始まらなくなる
///   (`is_diff_block_start` が認識できず、除外が丸ごと no-op になる)。
///   `--submodule=short` で標準のポインタヘッダーに固定する。
///
/// 除外は fail-closed が設計要件なので、ユーザー設定に左右されないよう形式を固定する。
const DIFF_FORMAT_ARGS: &[&str] = &[
    "--no-ext-diff",
    "--no-color",
    "--no-relative",
    "--ignore-submodules=none",
    "--submodule=short",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

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

/// reword 用コミットメッセージ一時ファイル
///
/// 固定名を避けて一意なファイルを作成し、Drop 時に自動削除する。
struct TempRewordMessageFile {
    path: PathBuf,
}

impl TempRewordMessageFile {
    /// 競合しない一時ファイルを作成し、コミットメッセージを書き込む
    fn create(message: &str) -> Result<Self, AppError> {
        let temp_dir = std::env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        for attempt in 0..100 {
            let path = temp_dir.join(format!(
                "git-sc-reword-message-{}-{}-{}.txt",
                std::process::id(),
                timestamp,
                attempt
            ));

            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // reword 用メッセージを他ユーザーに読めない権限で作成する。
                options.mode(0o600);
            }

            match options.open(&path) {
                Ok(mut file) => {
                    // 書き込み/sync失敗時はファイルを削除してからエラーを返す
                    if let Err(e) = file
                        .write_all(message.as_bytes())
                        .and_then(|_| file.sync_all())
                    {
                        drop(file);
                        let _ = std::fs::remove_file(&path);
                        return Err(AppError::GitError(format!(
                            "Failed to write temp file: {}",
                            e
                        )));
                    }
                    drop(file);
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(AppError::GitError(format!(
                        "Failed to create temp file: {}",
                        e
                    )));
                }
            }
        }

        Err(AppError::GitError(
            "Failed to create unique temp file".to_string(),
        ))
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempRewordMessageFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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

    /// 指定したリポジトリパスに対するGitServiceを作成（テスト専用）
    ///
    /// `new()` はカレントディレクトリ固定だが、こちらは任意のパスを対象にできる。
    /// 現状は一時リポジトリを対象としたユニットテストでのみ利用するため、
    /// 通常ビルドで dead_code 警告（CI の `-D warnings` で失敗）にならないよう
    /// `#[cfg(test)]` でゲートしている。非テストコードから使う必要が生じたら外す。
    #[cfg(test)]
    pub fn with_repo_path(repo_path: PathBuf) -> Self {
        Self { repo_path }
    }

    // ============================================================
    // git コマンド実行ヘルパー
    // ============================================================

    /// git コマンドを実行し、成功時は stdout（trim済み）を返す
    fn run_git(&self, args: &[&str]) -> Result<String, AppError> {
        self.run_git_raw(args)
            .map(|output| output.trim().to_string())
    }

    /// 差分本文の末尾空白を保持するため、成功時の標準出力を整形せず返す。
    fn run_git_raw(&self, args: &[&str]) -> Result<String, AppError> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    format!("exit code: {}", output.status)
                } else {
                    stdout
                }
            } else {
                stderr
            };
            return Err(AppError::GitError(msg));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
    ///
    /// `^{commit}` サフィックスで型制約し、tree/blob 等の非コミットオブジェクトを拒否する。
    fn verify_commit_hash(&self, hash: &str) -> Result<(), AppError> {
        self.resolve_commit_hash(hash).map(|_| ())
    }

    /// 相対参照やタグを一度だけ解決し、後から参照先が動いても同じコミットを扱う。
    pub fn resolve_commit_hash(&self, hash: &str) -> Result<String, AppError> {
        let rev = format!("{}^{{commit}}", hash);
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "--end-of-options", &rev])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            return Err(AppError::InvalidCommitHash(hash.to_string()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    // ============================================================
    // 内部ユーティリティ
    // ============================================================

    /// Gitリポジトリのルートディレクトリを取得
    pub fn get_git_root(&self) -> Option<PathBuf> {
        let mut output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&self.repo_path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // Git が付ける LF だけを除去する。trim() はパス末尾の空白や改行まで壊す。
        if output.stdout.last() == Some(&b'\n') {
            output.stdout.pop();
        }
        if output.stdout.is_empty() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Some(PathBuf::from(std::ffi::OsString::from_vec(output.stdout)))
        }
        #[cfg(not(unix))]
        {
            Some(PathBuf::from(
                String::from_utf8_lossy(&output.stdout).as_ref(),
            ))
        }
    }

    /// .git-sc-ignoreファイルを読み込んでGitignoreを構築
    ///
    /// 返り値が `Ok(None)` なのは「除外設定そのものが無い」場合だけ。
    /// `.git-sc-ignore` が存在するのに読めない/解釈できない場合はエラーにする(fail-closed)。
    /// ここを fail-open にすると、除外したかったファイル(認証情報・鍵・生成物)の差分が
    /// そのまま AI プロバイダーへ送信される。「除外設定を書いたのに効かず、しかも黙っている」
    /// のが最悪の失敗なので、内容が確実に適用できないときは処理を止める。
    fn load_ignore_patterns(&self) -> Result<Option<Gitignore>, AppError> {
        let Some(git_root) = self.get_git_root() else {
            return Ok(None);
        };
        let ignore_path = git_root.join(".git-sc-ignore");

        // リンク切れも「設定が存在するが読めない」に含める。exists() はリンク先を
        // たどるため false になり、除外するはずの内容をそのまま送ってしまう。
        match std::fs::symlink_metadata(&ignore_path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AppError::ConfigError(format!(
                    "{} を確認できませんでした: {}。除外設定が適用できないため中止します。",
                    ignore_path.display(),
                    e
                )));
            }
            Ok(_) => {}
        }

        let mut builder = GitignoreBuilder::new(&git_root);
        // `add` はパターン単位の部分エラーも返す。1 行でも解釈できなかったなら
        // 除外は不完全なので、その状態で差分を送らずにエラーとして知らせる。
        if let Some(err) = builder.add(&ignore_path) {
            return Err(AppError::ConfigError(format!(
                "{} を読み込めませんでした: {}。除外設定が適用できないため中止します。",
                ignore_path.display(),
                err
            )));
        }

        builder.build().map(Some).map_err(|e| {
            AppError::ConfigError(format!(
                "{} のパターンを解釈できませんでした: {}。除外設定が適用できないため中止します。",
                ignore_path.display(),
                e
            ))
        })
    }

    /// プレフィックススクリプトを実行する基準ディレクトリを取得
    ///
    /// `.git-sc` は Git ルートに置かれるため、相対パスの解決と
    /// スクリプトの作業ディレクトリは Git ルートにそろえる。
    fn prefix_script_base_dir(&self) -> PathBuf {
        self.get_git_root()
            .unwrap_or_else(|| self.repo_path.clone())
    }

    /// プレフィックススクリプトの実行パスを解決
    fn resolve_prefix_script_path(&self, script: &str) -> PathBuf {
        let script_path = Path::new(script);
        if script_path.is_absolute() {
            script_path.to_path_buf()
        } else {
            self.prefix_script_base_dir().join(script_path)
        }
    }

    /// diff ブロックの開始行か判定する
    ///
    /// マージコミットに対する `git show` は既定で combined diff を出力し、ヘッダーが
    /// `diff --cc <path>` / `diff --combined <path>` になる(`a/` `b/` は付かない)。
    /// `diff --git` だけを見ていると combined diff のブロックを 1 つも認識できず、
    /// `.git-sc-ignore` の除外もバイナリ要約も丸ごと素通りする。除外を書いたのに
    /// 効かず、しかも黙っている——`load_ignore_patterns` を fail-closed にしたのと
    /// 同じ事故が、`--generate-for <マージコミット>` と `--amend`(HEAD がマージ)で
    /// 起きていた。
    fn is_diff_block_start(line: &str) -> bool {
        line.starts_with("diff --git")
            || line.starts_with("diff --cc ")
            || line.starts_with("diff --combined ")
    }

    /// combined diff ヘッダー(`diff --cc <path>`)からファイルパスを取り出す
    ///
    /// combined diff は before/after が同一パスで、`a/` `b/` のプレフィックスも付かない。
    /// Git がクォートするのは非 ASCII 等を含む場合だけで、スペースだけならクォート
    /// されない(実測: `diff --cc sp ace.txt` / `diff --cc "nonascii-\303\251.txt"`)。
    /// そのため非クォート側は行末までを 1 つのパスとして扱う。
    fn extract_combined_diff_path(header: &str) -> Option<String> {
        let rest = header
            .strip_prefix("diff --cc ")
            .or_else(|| header.strip_prefix("diff --combined "))?;
        if rest.is_empty() {
            return None;
        }
        match rest.strip_prefix('"') {
            Some(after_quote) => {
                Self::decode_quoted_diff_path_with_rest(after_quote).map(|(path, _)| path)
            }
            None => Some(rest.to_string()),
        }
    }

    /// diff を行に分ける。`\r\n` の `\r` は行の内容として残す。
    ///
    /// `str::lines()` は `\r\n` の `\r` まで取り除くので、`join("\n")` で戻すと消えたままになる。
    /// LF → CRLF のように改行コードだけが変わった行は `-line` / `+line` の同じ組に見え、
    /// AI から変更が見えなくなる(空白の変更も AI に見せる方針と食い違う)。
    /// 区切りは `\n` だけにする。末尾の改行 1 つは `lines()` と同じく区切りとして扱い、
    /// 末尾に空の要素を作らない(出力の末尾の形を `lines()` のときから変えないため)。
    fn split_diff_lines(text: &str) -> Vec<&str> {
        if text.is_empty() {
            return Vec::new();
        }
        text.strip_suffix('\n')
            .unwrap_or(text)
            .split('\n')
            .collect()
    }

    /// ignoreパターンに一致するファイルは、パスを含むヘッダーだけを残す。
    fn filter_ignored_files(diff_text: &str, ignore: &Gitignore) -> String {
        if diff_text.is_empty() {
            return String::new();
        }

        let lines = Self::split_diff_lines(diff_text);
        let mut filtered_lines = Vec::new();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i];

            if Self::is_diff_block_start(line) {
                // このブロックの終端を見つける
                let block_start = i;
                i += 1;
                while i < lines.len() && !Self::is_diff_block_start(lines[i]) {
                    i += 1;
                }

                // ignoreパターンにマッチするかチェック
                let should_ignore = Self::ignore_candidate_paths(&lines[block_start..i])
                    .iter()
                    .any(|path| ignore.matched_path_or_any_parents(path, false).is_ignore());

                // パスは残すが、本文とそれ以外のメタデータは AI に送らない。
                // 元のヘッダーを使い、rename の両側やクォートされたパスも保持する。
                if should_ignore {
                    filtered_lines.push(line);
                    filtered_lines.push(IGNORED_CONTENT_NOTICE);
                } else {
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

    /// `.git-sc-ignore` と照合するパスを diff ブロックから集める
    ///
    /// ヘッダー(`diff --git a/... b/...`)から読んだ変更前/変更後のパスに加えて、
    /// 最初の `@@` より前にある rename / copy のメタデータ行のパスも候補にする。
    /// 非クォートのヘッダーはパスに ` b/` を含むと境界が一意に決まらず、
    /// `a/notes.txt b/config/plan b/notes.txt` を最後の ` b/` で切ると
    /// `notes.txt b/config/plan` と `notes.txt` という実在しない 2 つのパスになって、
    /// `/config/` の除外が外れる(移動先の内容がそのまま AI へ送られる)。
    /// メタデータ行は 1 行に 1 パスなので曖昧さが無い。候補を足す方向にしか働かないため、
    /// ヘッダーで一致していたものが外れることはない(fail-closed)。
    fn ignore_candidate_paths(block: &[&str]) -> Vec<String> {
        let mut paths = Vec::new();
        if let Some((before_path, after_path)) = block
            .first()
            .and_then(|header| Self::extract_file_paths_from_diff_header(header))
        {
            paths.push(before_path);
            paths.push(after_path);
        }
        for line in block
            .iter()
            .skip(1)
            .take_while(|line| !line.starts_with("@@"))
        {
            let metadata_path = ["rename from ", "rename to ", "copy from ", "copy to "]
                .into_iter()
                .find_map(|prefix| line.strip_prefix(prefix));
            if let Some(path) = metadata_path.and_then(Self::decode_diff_metadata_path) {
                paths.push(path);
            }
        }
        paths
    }

    /// diffヘッダーから変更前/変更後のファイルパスを抽出
    ///
    /// Git は `diff --git` 行でファイル名にスペースが含まれる場合もクォート形式に
    /// しないため、素朴な空白分割では誤抽出になる（例: `diff --git a/foo bar.txt
    /// b/foo bar.txt` → `foo` と `bar.txt` に分割されてしまう）。
    /// 非クォートの同一パス対称ケースでは中央分割で正しく抽出し、片側だけが
    /// クォートされているケースではクォート側を優先的に解析する。
    fn extract_file_paths_from_diff_header(header: &str) -> Option<(String, String)> {
        // combined diff(マージコミット)は before/after が同一の 1 パス
        if let Some(path) = Self::extract_combined_diff_path(header) {
            return Some((path.clone(), path));
        }

        let rest = header.strip_prefix("diff --git ")?;

        if !rest.contains('"') {
            // 同一パスのスペース含み変更を先に中央分割で処理する。
            if let Some(paths) = Self::try_split_symmetric_unquoted_header(rest) {
                return Some(paths);
            }
            // rename などで before/after が異なる場合は、最後の ` b/` を境界にする。
            if let Some(paths) = Self::try_split_asymmetric_unquoted_header(rest) {
                return Some(paths);
            }
        }

        // 「非クォート(スペース含み得る) → クォート」の rename ヘッダーを処理する
        // (例: `diff --git a/old name.txt "b/new\303\251.txt"`)。
        // Git は a 側 / b 側を独立にクォートするため、スペースだけを含む旧パスは
        // 非クォート、非 ASCII の新パスはクォートという混在形が実際に出力される。
        // 非クォート側のパスに `"` は現れない(含むなら Git がクォートする)ので、
        // 行内で最初に現れる `"` がクォート側の開始位置として一意に定まる。
        if !rest.starts_with('"')
            && let Some(quote_pos) = rest.find('"')
        {
            let before_part = rest[..quote_pos].strip_suffix(' ')?;
            let (after_path, _) = Self::decode_quoted_diff_path_with_rest(&rest[quote_pos + 1..])?;
            return Some((
                before_part.strip_prefix("a/")?.to_string(),
                after_path.strip_prefix("b/")?.to_string(),
            ));
        }

        // 先頭がクォートの場合はクォート解析、そうでなければ空白分割
        let (before_path, rest_after) = Self::take_diff_header_path(rest)?;
        let rest_after = rest_after.strip_prefix(' ')?;

        // 残り側がクォートなら再度クォート解析、そうでなければ末尾までを
        // そのまま after_path として扱う（`"a/旧.txt" b/new name.txt` のような
        // 片側クォート + スペース含み非クォートに対応）
        let after_path = if rest_after.starts_with('"') {
            Self::take_diff_header_path(rest_after)?.0
        } else {
            rest_after.to_string()
        };

        Some((
            before_path.strip_prefix("a/")?.to_string(),
            after_path.strip_prefix("b/")?.to_string(),
        ))
    }

    /// 非クォートの `a/PATH b/PATH` 形式を中央分割で抽出する
    ///
    /// `a/PATH b/PATH` の全長は `5 + 2 * len(PATH)` となり必ず奇数であることを
    /// 利用して中央位置を特定し、前後パートが `a/`・`b/` で始まり、
    /// パス部分が一致する場合のみ成功とする。
    fn try_split_symmetric_unquoted_header(rest: &str) -> Option<(String, String)> {
        let total_len = rest.len();
        // `a/x b/x` で最短7文字、全長は奇数である必要がある
        if total_len < 7 || total_len.is_multiple_of(2) {
            return None;
        }
        let mid = total_len / 2;
        if rest.as_bytes().get(mid) != Some(&b' ') {
            return None;
        }
        let before_part = &rest[..mid];
        let after_part = &rest[mid + 1..];
        let before_path = before_part.strip_prefix("a/")?;
        let after_path = after_part.strip_prefix("b/")?;
        if before_path != after_path {
            return None;
        }
        // パス部にクォート文字が残る場合はこの経路では扱わない
        if before_path.contains('"') {
            return None;
        }
        Some((before_path.to_string(), after_path.to_string()))
    }

    /// 非クォートの `a/OLD b/NEW` 形式を末尾側の ` b/` で分割する。
    ///
    /// Git はスペースを含む rename でも `diff --git a/old name b/new name`
    /// のように出すため、同一パス前提の中央分割だけでは移動先を抽出できない。
    fn try_split_asymmetric_unquoted_header(rest: &str) -> Option<(String, String)> {
        let rest = rest.strip_prefix("a/")?;
        let (before_path, after_path) = rest.rsplit_once(" b/")?;
        if before_path.is_empty() || after_path.is_empty() {
            return None;
        }
        if before_path.contains('"') || after_path.contains('"') {
            return None;
        }
        Some((before_path.to_string(), after_path.to_string()))
    }

    /// diffヘッダーからファイルパスを1つ読み取る
    fn take_diff_header_path(input: &str) -> Option<(String, &str)> {
        let input = input.trim_start();

        if let Some(after_quote) = input.strip_prefix('"') {
            Self::decode_quoted_diff_path_with_rest(after_quote)
        } else {
            let end = input.find(char::is_whitespace).unwrap_or(input.len());
            if end == 0 {
                return None;
            }
            Some((input[..end].to_string(), &input[end..]))
        }
    }

    /// diffヘッダーからファイルパスを抽出
    #[cfg(test)]
    fn extract_file_path_from_diff_header(header: &str) -> Option<String> {
        Self::extract_file_paths_from_diff_header(header).map(|(path, _)| path)
    }

    /// Git の quoted path をデコードする
    ///
    /// Git は `core.quotePath=true` のとき、非 ASCII 文字を 8 進エスケープで出力する。
    /// `.git-sc-ignore` との照合に使うため、実際のパス文字列へ復元する。
    fn decode_quoted_diff_path_with_rest(input: &str) -> Option<(String, &str)> {
        fn is_octal(byte: u8) -> bool {
            matches!(byte, b'0'..=b'7')
        }

        let bytes = input.as_bytes();
        let mut decoded = Vec::new();
        let mut i = 0;
        let mut closed_quote = false;

        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    closed_quote = true;
                    i += 1;
                    break;
                }
                b'\\' => {
                    i += 1;
                    let escaped = *bytes.get(i)?;

                    match escaped {
                        // Git の `quote.c` (sq_lookup) が文字エスケープとして使うのは
                        // \a \b \t \n \v \f \r " \ の 9 種。取りこぼすと下の `other`
                        // 分岐でリテラルの英字になり(`se\acret/` が `secret/` ではなく
                        // `seacret/` に復元される)、`.git-sc-ignore` の照合が
                        // 実在しないパスに対して行われて除外が外れる。
                        b'a' => decoded.push(0x07),
                        b'b' => decoded.push(0x08),
                        b'n' => decoded.push(b'\n'),
                        b't' => decoded.push(b'\t'),
                        b'v' => decoded.push(0x0b),
                        b'f' => decoded.push(0x0c),
                        b'r' => decoded.push(b'\r'),
                        b'\\' => decoded.push(b'\\'),
                        b'"' => decoded.push(b'"'),
                        b'0'..=b'7' => {
                            let second = *bytes.get(i + 1)?;
                            let third = *bytes.get(i + 2)?;
                            if !is_octal(second) || !is_octal(third) {
                                return None;
                            }

                            // Gitのquoted pathの3桁8進エスケープは1バイトを表す
                            // 0o400..=0o777は不正入力として拒否する
                            let value = (escaped - b'0') as u16 * 64
                                + (second - b'0') as u16 * 8
                                + (third - b'0') as u16;
                            if value > u8::MAX as u16 {
                                return None;
                            }
                            decoded.push(value as u8);
                            i += 2;
                        }
                        other => decoded.push(other),
                    }
                }
                byte => decoded.push(byte),
            }

            i += 1;
        }

        if !closed_quote || decoded.is_empty() {
            return None;
        }

        Some((String::from_utf8_lossy(&decoded).into_owned(), &input[i..]))
    }

    /// Git の quoted path をデコードする
    #[cfg(test)]
    fn decode_quoted_diff_path(input: &str) -> Option<String> {
        Self::decode_quoted_diff_path_with_rest(input).map(|(path, _)| path)
    }

    /// diffを最大文字数に切り詰める
    #[cfg(test)]
    pub fn truncate_diff(diff: &str) -> String {
        Self::truncate_diff_with_source(diff, diff, None)
    }

    /// 上限以内の差分はそのまま。超過した場合は raw diff の全ファイル一覧を残す。
    fn truncate_diff_with_source(diff: &str, source: &str, ignore: Option<&Gitignore>) -> String {
        // バイト長 <= MAX_DIFF_CHARS なら必ず文字数も収まる (UTF-8 は 1 文字 >= 1 バイト)
        // 大半の diff はこの早期 return で全文走査を回避する
        if diff.len() <= MAX_DIFF_CHARS {
            return diff.to_string();
        }

        // バイト長は超過しているが文字数では収まる可能性がある (マルチバイト混在時)
        // MAX_DIFF_CHARS+1 文字目までだけ走査して切り出し位置を求める
        let cutoff = match diff.char_indices().nth(MAX_DIFF_CHARS) {
            Some((idx, _)) => idx,
            None => return diff.to_string(),
        };

        if let Some(excerpts) = Self::diff_file_excerpts(source, ignore) {
            return Self::render_diff_excerpts(excerpts);
        }

        let truncated = &diff[..cutoff];

        // 最後の完全な行まで切り詰める（中途半端な行を避ける）
        let body = truncated
            .rfind('\n')
            .map(|p| &truncated[..p])
            .unwrap_or(truncated);
        format!(
            "{}\n\n... (diff truncated: exceeded {} characters)",
            body, MAX_DIFF_CHARS
        )
    }

    /// 行数は本文を除外・要約する前に数える。hunk 外の --- / +++ は数えない。
    fn diff_block_overview(block: &[&str]) -> String {
        let paths = Self::ignore_candidate_paths(block);
        let path = match paths.as_slice() {
            [.., before, after] if before != after => format!(
                "{} -> {}",
                Self::summary_path(before),
                Self::summary_path(after)
            ),
            [.., path] => Self::summary_path(path),
            [] => Self::summary_path(block.first().copied().unwrap_or("unknown")),
        };
        if block
            .iter()
            .any(|line| line.starts_with("Binary files ") || *line == "GIT binary patch")
        {
            return format!("[File] {path} | binary");
        }

        let mut added = 0usize;
        let mut deleted = 0usize;
        let mut columns = 0usize;
        let mut combined = false;
        for line in block {
            let at_count = line.bytes().take_while(|byte| *byte == b'@').count();
            if at_count >= 2 && line.as_bytes().get(at_count) == Some(&b' ') {
                columns = if at_count == 2 { 1 } else { at_count - 1 };
                combined |= columns > 1;
                continue;
            }
            if columns == 0 || line.len() < columns {
                continue;
            }
            let signs = &line.as_bytes()[..columns];
            // combined diff の親別列は行を重複計数せず、表示された変更行として数える。
            if signs.iter().all(|sign| matches!(sign, b' ' | b'+' | b'-')) {
                added += usize::from(signs.contains(&b'+'));
                deleted += usize::from(signs.contains(&b'-'));
            }
        }
        let qualifier = if combined { " (combined rows)" } else { "" };
        format!("[File] {path} | +{added} -{deleted}{qualifier}")
    }

    fn diff_file_excerpts(diff: &str, ignore: Option<&Gitignore>) -> Option<Vec<DiffFileExcerpt>> {
        let lines = Self::split_diff_lines(diff);
        let mut excerpts = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            if !Self::is_diff_block_start(lines[index]) {
                index += 1;
                continue;
            }
            let start = index;
            index += 1;
            while index < lines.len() && !Self::is_diff_block_start(lines[index]) {
                index += 1;
            }
            let block = &lines[start..index];
            let raw = block.join("\n");
            let filtered = match ignore {
                Some(ignore) => Self::filter_ignored_files(&raw, ignore),
                None => raw.clone(),
            };
            let ignored = filtered != raw;
            // パスと省略通知は一覧に載せるので、本文の予算を使わない。
            let content = if ignored {
                String::new()
            } else {
                Self::summarize_diff(&filtered)
            };
            let mut overview = Self::diff_block_overview(block);
            if ignored {
                overview.push(' ');
                overview.push_str(IGNORED_CONTENT_NOTICE);
            } else if content != filtered {
                overview.push_str(" [lockfile/binary content omitted]");
            }
            excerpts.push(DiffFileExcerpt {
                overview,
                body_omitted: content != raw,
                content,
            });
        }
        (!excerpts.is_empty()).then_some(excerpts)
    }

    /// 小さいファイルの余りを再配分し、先頭の大きなファイルによる予算の独占を防ぐ。
    fn render_diff_excerpts(excerpts: Vec<DiffFileExcerpt>) -> String {
        let mut sizes = excerpts
            .iter()
            .enumerate()
            .filter(|(_, excerpt)| !excerpt.content.is_empty())
            .map(|(index, excerpt)| (excerpt.content.chars().count(), index))
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        let mut allocations = vec![0; excerpts.len()];
        // 本文を連結する改行も本文予算に含める。一覧と通知はこの予算の別枠。
        let mut remaining = MAX_DIFF_CHARS.saturating_sub(sizes.len().saturating_sub(1));
        for (position, (size, index)) in sizes.iter().enumerate() {
            let share = remaining / (sizes.len() - position);
            allocations[*index] = (*size).min(share);
            remaining -= allocations[*index];
        }
        let mut overview = String::from("[Complete change overview]\n");
        let mut bodies = Vec::new();
        let mut omitted = 0usize;
        for (excerpt, allocation) in excerpts.iter().zip(allocations) {
            let body = match excerpt.content.char_indices().nth(allocation) {
                Some((cutoff, _)) => {
                    let prefix = &excerpt.content[..cutoff];
                    prefix.rfind('\n').map_or("", |end| &prefix[..end])
                }
                None => &excerpt.content,
            };
            overview.push_str(&excerpt.overview);
            if body.len() < excerpt.content.len() {
                overview.push_str(" [body truncated]");
            }
            overview.push('\n');
            omitted += usize::from(excerpt.body_omitted || body.len() < excerpt.content.len());
            if !body.is_empty() {
                bodies.push(body);
            }
        }
        format!(
            "{overview}\n... (diff truncated: exceeded {MAX_DIFF_CHARS} characters)\n\
             [{omitted} of {} file bodies omitted or truncated; complete file list above; \
             body budget {MAX_DIFF_CHARS} characters]\n\n{}",
            excerpts.len(),
            bodies.join("\n")
        )
    }

    /// diffに対して全てのフィルタリングを適用
    ///
    /// `.git-sc-ignore` が存在するのに適用できない場合はエラーを返す。
    /// 呼び出し元(diff 取得 API)はそのままエラーを伝播し、除外漏れの差分を
    /// AI へ送らずに終了する。
    fn apply_all_filters(&self, diff: &str) -> Result<String, AppError> {
        // 1. .git-sc-ignore パターンにマッチするファイルの本文を省略
        //    バイナリフィルタより先に実行する必要がある。
        //    summarize_diff は diff --git ヘッダーをサマリー行に変換するため、
        //    先に実行するとignoreパターンがバイナリファイルに適用されなくなる。
        let ignore = self.load_ignore_patterns()?;
        let filtered = if let Some(ignore) = &ignore {
            Self::filter_ignored_files(diff, ignore)
        } else {
            diff.to_string()
        };

        // 2. バイナリ・lock ファイルをサマリーに変換
        let filtered = Self::summarize_diff(&filtered);

        // 3. 文字数制限を適用
        Ok(Self::truncate_diff_with_source(
            &filtered,
            diff,
            ignore.as_ref(),
        ))
    }

    /// Git のパスはプラットフォームによらず `/` 区切り。
    /// 親ディレクトリ名ではなく、ファイル名だけで lock ファイルを判定する。
    fn is_lockfile(path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.ends_with(".lock")
            || name.ends_with(".lockb")
            || name.ends_with(".lockfile")
            || matches!(
                name,
                "package-lock.json"
                    | "npm-shrinkwrap.json"
                    | "pnpm-lock.yaml"
                    | "go.sum"
                    | "Package.resolved"
            )
    }

    /// デコード済みパスを一行で表示する。制御文字で要約行が分裂するのを防ぐ。
    fn summary_path(path: &str) -> String {
        if path.trim() != path || path.chars().any(char::is_control) || path.contains(['"', '\\']) {
            format!("{path:?}")
        } else {
            path.to_string()
        }
    }

    /// rename/copy メタデータのパスには a/・b/ プレフィックスが付かない。
    fn decode_diff_metadata_path(path: &str) -> Option<String> {
        match path.strip_prefix('"') {
            Some(quoted) => Self::decode_quoted_diff_path_with_rest(quoted).map(|(path, _)| path),
            None => Some(path.to_string()),
        }
    }

    /// 本文を省略するブロックだけ要約を返す。
    fn summarize_diff_block(block: &[&str]) -> Option<String> {
        // 除外済みの lock ファイルもパスと省略通知をそのまま通す。
        // 変更種別のメタデータは既に省かれているため、modified と推測しない。
        if block.len() == 2 && block[1] == IGNORED_CONTENT_NOTICE {
            return None;
        }

        let mut is_binary = false;
        let mut change = "modified";
        let mut path_from = None;
        let mut path_to = None;
        for line in block
            .iter()
            .skip(1)
            .take_while(|line| !line.starts_with("@@"))
        {
            if line.starts_with("Binary files") {
                is_binary = true;
            } else if line.starts_with("new file mode") {
                change = "added";
            } else if line.starts_with("deleted file mode") {
                change = "deleted";
            } else if let Some(from) = line.strip_prefix("rename from ") {
                change = "renamed";
                path_from = Some(from);
            } else if let Some(from) = line.strip_prefix("copy from ") {
                change = "copied";
                path_from = Some(from);
            } else if let Some(to) = line
                .strip_prefix("rename to ")
                .or_else(|| line.strip_prefix("copy to "))
            {
                path_to = Some(to);
            }
        }
        // スペースを含む非クォートのヘッダーは分割が曖昧なため、移動・コピーは
        // 一行に一つのパスを持つメタデータを優先する（例: b/new b/path.lock）。
        let file_paths = path_from
            .zip(path_to)
            .and_then(|(from, to)| {
                Some((
                    Self::decode_diff_metadata_path(from)?,
                    Self::decode_diff_metadata_path(to)?,
                ))
            })
            .or_else(|| {
                block
                    .first()
                    .and_then(|header| Self::extract_file_paths_from_diff_header(header))
            });
        // rename / copy の片側だけが lock ファイルでも本文は送らない。
        let is_lockfile = file_paths
            .as_ref()
            .is_some_and(|(before, after)| Self::is_lockfile(before) || Self::is_lockfile(after));
        if !is_binary && !is_lockfile {
            return None;
        }

        let kind = if is_lockfile { "Lockfile" } else { "Binary" };
        let paths = match file_paths {
            Some((before, after)) if matches!(change, "renamed" | "copied") => {
                format!(
                    "{} -> {}",
                    Self::summary_path(&before),
                    Self::summary_path(&after)
                )
            }
            Some((before, _)) if change == "deleted" => Self::summary_path(&before),
            Some((_, after)) => Self::summary_path(&after),
            None => "unknown".to_string(),
        };
        Some(format!("[{kind}] {change}: {paths}"))
    }

    /// バイナリ・lock ファイルの本文を省き、パスと変更種別のみを出力する。
    fn summarize_diff(diff_text: &str) -> String {
        // CRLF の `\r` を残すため `lines()` は使わない(split_diff_lines を参照)
        let lines = Self::split_diff_lines(diff_text);
        let mut filtered_lines = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let block_start = i;
            i += 1;
            if Self::is_diff_block_start(lines[block_start]) {
                while i < lines.len() && !Self::is_diff_block_start(lines[i]) {
                    i += 1;
                }
                if let Some(summary) = Self::summarize_diff_block(&lines[block_start..i]) {
                    filtered_lines.push(summary);
                    continue;
                }
            }
            filtered_lines.extend(lines[block_start..i].iter().map(|line| (*line).to_string()));
        }
        filtered_lines.join("\n")
    }

    // ============================================================
    // パブリック API
    // ============================================================

    /// Gitリポジトリ内にいるか確認
    pub fn verify_repository(&self) -> Result<(), AppError> {
        // `.git` という通常ファイルやディレクトリがあるだけではリポジトリとは限らないため、
        // Git 自身に作業ツリー内かどうかを判定させる。
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        let is_work_tree = String::from_utf8_lossy(&output.stdout).trim() == "true";
        if output.status.success() && is_work_tree {
            Ok(())
        } else {
            Err(AppError::NotGitRepository)
        }
    }

    /// ステージ済み差分を取得し、バイナリ・lock ファイル・除外対象の本文を省略する。
    pub fn get_staged_diff(&self) -> Result<String, AppError> {
        // Python や YAML はインデントにも意味があるため、空白の変更も AI に渡す。
        let mut args = vec!["diff", "--cached", "-U0"];
        args.extend_from_slice(DIFF_FORMAT_ARGS);
        let raw = self.run_git_raw(&args)?;
        self.apply_all_filters(&raw)
    }

    /// 直近のコミットメッセージを取得
    pub fn get_recent_commits(&self, count: usize) -> Result<Vec<String>, AppError> {
        // 空リポジトリでは `git log` のエラーメッセージがロケール依存になるため、
        // 先に HEAD コミットの有無を機械的に判定してから `git log` を呼び出す。
        if !self.has_head_commit()? {
            return Ok(Vec::new());
        }

        let output = Command::new("git")
            .args([
                "log",
                "--no-show-signature",
                "--format=%s",
                "-n",
                &count.to_string(),
            ])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if stderr.is_empty() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    format!("exit code: {}", output.status)
                } else {
                    stdout
                }
            } else {
                stderr
            };
            return Err(AppError::GitError(msg));
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
        // nul の探索・除外はいずれも Git ルート基準にする。`repo_path` は cwd なので、
        // サブディレクトリから実行するとルート直下の nul を取りこぼす。
        #[cfg(windows)]
        {
            let nul_path = self
                .get_git_root()
                .unwrap_or_else(|| self.repo_path.clone())
                .join("nul");
            if nul_path.exists() {
                let _ = std::fs::remove_file(&nul_path);
            }
        }

        // Windows環境では "nul" を除外してステージング（pathspecの除外指定を使用）。
        // pathspec に `.` を渡すと cwd 配下だけが対象になり、Unix 側の `git add -A`
        // (作業ツリー全体) と範囲が食い違う。サブディレクトリから `-a` を使うと
        // cwd 外の変更が黙って取り残されるため、`:/` でルート基準の全体指定にし、
        // nul の除外も `top` 属性でルート基準へ固定する。
        #[cfg(windows)]
        let args: &[&str] = &["add", "-A", "--", ":/", ":(exclude,top)nul"];
        #[cfg(not(windows))]
        let args: &[&str] = &["add", "-A"];

        self.run_git_ok(args)?;

        // Windows環境で万が一 nul がステージされていた場合はアンステージ
        #[cfg(windows)]
        {
            let _ = Command::new("git")
                .args(["reset", "HEAD", "--", ":(top)nul"])
                .current_dir(&self.repo_path)
                .output();
        }

        Ok(())
    }

    /// ステージ済みの変更が存在するかチェック
    pub fn has_staged_changes(&self) -> bool {
        // git diff --cached --quiet は差分があると exit 1 を返す。
        //
        // `--no-relative` は差分取得側 (`DIFF_FORMAT_ARGS`) と基準をそろえるために必須。
        // `repo_path` は Git ルートではなく cwd なので、`diff.relative = true` の設定下で
        // サブディレクトリから実行すると cwd 配下しか見えず、外側の staged 変更を
        // 「無し」と誤判定する。差分取得は `--no-relative` で全体を見ているため、
        // 「メッセージは生成できたのにコミットはスキップされる」「squash 前のガードが
        // 素通りして無関係な staged 変更が squash コミットへ混入する」という食い違いになる。
        //
        // `--ignore-submodules=none` も同じ理由で必要。`diff.ignoreSubmodules = all` や
        // `.gitmodules` の `submodule.<name>.ignore = all` があると、staged 済みの
        // サブモジュールポインタ変更が `--quiet` から見えず exit 0(= 変更なし)になる。
        // `--submodule` は出力形式だけを変える設定なので、presence のみを見る
        // `--quiet` には影響せず、ここでは不要。
        Command::new("git")
            .args([
                "diff",
                "--cached",
                "--quiet",
                "--no-relative",
                "--ignore-submodules=none",
            ])
            .current_dir(&self.repo_path)
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(false)
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

    /// 直前の差分を取得し、バイナリ・lock・除外対象の本文を省略する。空白の変更は残す。
    pub fn get_last_commit_diff(&self) -> Result<String, AppError> {
        self.get_commit_diff_by_hash("HEAD")
    }

    /// 直前のコミットを新しいメッセージで修正
    pub fn amend_commit(&self, message: &str) -> Result<(), AppError> {
        // depth 1 の shallow clone では HEAD 自身が境界で、amend すると親が外れる
        self.refuse_shallow_boundary(1)?;
        self.run_git_ok(&["commit", "--amend", "--only", "-m", message])
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

        let base_dir = self.prefix_script_base_dir();
        let script_path = self.resolve_prefix_script_path(script);

        let output = Command::new(&script_path)
            .args([remote_url, branch])
            .current_dir(base_dir)
            .stderr(Stdio::inherit()) // stderrは直接ターミナルに出力
            .output()
            .ok()?;

        if output.status.success() {
            let prefix = String::from_utf8_lossy(&output.stdout).to_string();
            // `echo` など一般的なスクリプト出力の行末だけを除去し、
            // プレフィックスとして意図された末尾スペースは保持する。
            let prefix = prefix.trim_end_matches(['\r', '\n']).to_string();
            if prefix.trim().is_empty() {
                Some(ScriptResult::Empty)
            } else {
                Some(ScriptResult::Prefix(prefix))
            }
        } else if output.status.code() == Some(1) {
            // 終了コード 1 は「AI 生成メッセージをそのまま使用」の明示シグナル。
            Some(ScriptResult::Failed)
        } else {
            None
        }
    }

    /// ブランチが存在するか確認
    pub fn branch_exists(&self, branch: &str) -> bool {
        // branch はユーザー入力(`--squash <branch>`)。`-` 始まりの値がオプションとして
        // 解釈されないよう `--end-of-options` の後ろに revision として渡す
        // (ハッシュ受け取り経路と同じオプション注入対策をそろえる)。
        self.try_run_git(&["rev-parse", "--verify", "--end-of-options", branch])
            .is_some()
    }

    /// 2つのブランチのmerge-baseを取得
    pub fn get_merge_base(&self, base: &str, head: &str) -> Result<String, AppError> {
        // base はユーザー入力(`--squash <branch>`)。オプション注入を防ぐため
        // `--end-of-options` の後ろに revision として渡す(ハッシュ経路と防御をそろえる)。
        self.run_git(&["merge-base", "--end-of-options", base, head])
            .map_err(|_| {
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

    /// ベースから HEAD までの差分を取得し、バイナリ・lock ファイル・除外対象の本文を省略する。
    pub fn get_diff_from_base(&self, base: &str) -> Result<String, AppError> {
        let mut args = vec!["diff", "-U0"];
        args.extend_from_slice(DIFF_FORMAT_ARGS);
        // 末尾の `--` はリビジョンとパスの区切り。無いと git がリビジョンを作業ツリーの
        // ファイル名とも照合し、`HEAD` という名前のファイルがあるだけで
        // `ambiguous argument 'HEAD': both revision and filename` で失敗する。
        args.extend_from_slice(&[base, "HEAD", "--"]);
        let raw = self.run_git_raw(&args)?;
        self.apply_all_filters(&raw)
    }

    /// 現在のHEADのコミットハッシュを取得する
    ///
    /// squash の `reset --soft` 前に復旧先として控えるために使う。
    pub fn get_head_hash(&self) -> Result<String, AppError> {
        self.run_git(&["rev-parse", "HEAD"])
    }

    /// 履歴が動いていないかを比べるための HEAD スナップショット
    ///
    /// `write_tree()` による index の突き合わせは「ステージ内容が変わったか」しか見ない。
    /// squash と amend が説明するのは index ではなく **HEAD 側の履歴**なので、生成中に
    /// 別の端末が `git commit` しても index はきれいなままで、そちらのガードは素通りする。
    /// 実際に squash では、AI が見ていない後発コミットが squash 対象に巻き込まれる
    /// (実機で再現)。amend も同様に、旧コミットを説明したメッセージで別コミットを
    /// 書き換えてしまう。
    ///
    /// HEAD が無いリポジトリ(初回コミット前)では `None` を返し、比較を成立させる。
    pub fn head_snapshot(&self) -> Option<String> {
        self.run_git(&["rev-parse", "HEAD"]).ok()
    }

    /// 現在の index を表す tree ハッシュを取得する
    ///
    /// 「AI へ渡した差分を作った時点の index」と「コミット直前の index」が同じかを
    /// 比べるために使う。`has_staged_changes()` は有無しか見ないので、AI 生成中に
    /// 別の端末が `git add` してもすり抜ける。tree ハッシュなら内容の変化を検出できる。
    ///
    /// `git write-tree` は index からツリーオブジェクトを書き出してハッシュを返す。
    /// index と作業ツリーは変化せず、書かれたツリーはコミットから参照されなければ
    /// gc の対象になるだけなので、検査目的で呼んでも副作用にならない。
    /// 未解決のコンフリクトが index にあると失敗するため、呼び出し側は取得できない
    /// 場合に検査を省略できるよう `Result` のまま扱うこと。
    pub fn write_tree(&self) -> Result<String, AppError> {
        self.run_git(&["write-tree"])
    }

    /// 指定したコミットにsoft resetする
    pub fn soft_reset_to(&self, commit: &str) -> Result<(), AppError> {
        // `--` が無いと、コミットと同名のファイルが作業ツリーにあるだけで
        // 「リビジョンかファイル名か曖昧」として失敗する(get_diff_from_base と同じ)。
        self.run_git_ok(&["reset", "--soft", commit, "--"])
    }

    /// 指定範囲にマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range(&self, n: usize) -> Result<bool, AppError> {
        if n == 0 {
            return Err(AppError::InvalidRewordTarget);
        }

        // n は HEAD~n（first-parent 深さ）として使うため、判定も first-parent 深さで揃える。
        let first_parent_total = self.get_head_first_parent_count()?;
        if n > first_parent_total {
            return Err(AppError::InvalidRewordTarget);
        }

        // マージコミットは親が2つ以上ある。
        // 末尾の `--` が無いと、`HEAD` という名前のファイルが作業ツリーにあるだけで
        // 「リビジョンかファイル名か曖昧」として失敗する(範囲指定の側もそろえて付ける)。
        let merges = if n == first_parent_total {
            // 最古コミットを含む範囲では HEAD~n が無効になるため履歴全体を対象にする
            self.run_git(&["rev-list", "--merges", "HEAD", "--"])?
        } else {
            self.run_git(&["rev-list", "--merges", &format!("HEAD~{}..HEAD", n), "--"])?
        };
        Ok(!merges.is_empty())
    }

    /// HEAD から first-parent 経路を辿った深さ（コミット数）を取得
    ///
    /// reword で使う位置 `n` は後段で `HEAD~n`（= first-parent 深さ）として消費されるため、
    /// 「最古コミットか（`n == 深さ`）」「範囲外か（`n > 深さ`）」の判定もこの first-parent
    /// 深さで揃える必要がある。`rev-list --count HEAD`（全祖先のトポロジカル数）で数えると、
    /// マージで取り込まれた側ブランチのコミットが余分に加算され、`HEAD~n` の世界と乖離する。
    fn get_head_first_parent_count(&self) -> Result<usize, AppError> {
        // `--` が無いと `HEAD` という名前のファイルがあるだけで曖昧として失敗する
        let count_str = self.run_git(&["rev-list", "--count", "--first-parent", "HEAD", "--"])?;
        count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))
    }

    /// shallow clone の境界(親を取得していないコミット)を書き換えようとしていたら止める
    ///
    /// `n` は HEAD から first-parent をたどった位置(HEAD が 1)で、対象は `HEAD~(n-1)`。
    /// 境界のコミットは親が取得されていないため、git からは親の無いコミットに見える。
    /// そのまま書き換えると、reword は最古コミット用の `--root` で作り直して本来の親を
    /// 失った新しい root コミットにし(exit 0 で成功と表示される)、`git commit --amend` も
    /// 境界のコミットの親を引き継げず parent 行の無いコミットを作る(depth 1 の clone で
    /// HEAD を amend して確認)。どちらも force push するとリモートの履歴が切れるため、
    /// 書き換える前に止めて全履歴の取得を案内する。
    fn refuse_shallow_boundary(&self, n: usize) -> Result<(), AppError> {
        // shallow でなければ境界は無い。コミットごとの判定より先に、安い全体判定で抜ける
        if self.run_git(&["rev-parse", "--is-shallow-repository"])? != "true" {
            return Ok(());
        }
        // shallow かどうかはリポジトリ全体の状態で、対象が境界とは限らない。shallow clone の
        // 中で `git switch --orphan` して作った本当の root コミットは、親を失う心配が無い。
        // オブジェクトに記録された親の数と、git が履歴をたどるときに見える親の数を比べ、
        // 見える方が少ないコミットだけを境界とみなす。
        let target = format!("HEAD~{}", n.saturating_sub(1));
        let object = self.run_git(&["cat-file", "commit", &target])?;
        let recorded_parents = object
            .lines()
            .take_while(|line| !line.is_empty())
            .filter(|line| line.starts_with("parent "))
            .count();
        let visible_parents = self
            .run_git(&["rev-list", "--parents", "-n", "1", &target, "--"])?
            .split_whitespace()
            .count()
            .saturating_sub(1);
        if visible_parents >= recorded_parents {
            return Ok(());
        }
        Err(AppError::InvalidArgument(
            "shallow clone の境界にあるコミット(親コミットを取得していないコミット)は書き換えられません。書き換えると本来の親が外れて履歴が切れます。`git fetch --unshallow` で全履歴を取得してから再実行してください"
                .to_string(),
        ))
    }

    /// rebase が進行中か確認する
    ///
    /// `git rebase -i`(merge backend)は `rebase-merge`、`git rebase --apply` や
    /// `git am` は `rebase-apply` を作る。どちらが残っていても新しい rebase は開始できない。
    /// 位置は worktree ごとに変わる(linked worktree では `.git/worktrees/<name>/` 配下)ため、
    /// `.git` を直接組み立てず `git rev-parse --git-path` に解決させる。
    ///
    /// 検出は「開始前の門番」として使う。開始せずに止めることで、失敗時の
    /// `git rebase --abort` が常に自分の rebase だけを対象にできる。
    pub fn rebase_in_progress(&self) -> bool {
        ["rebase-merge", "rebase-apply"].iter().any(|name| {
            self.try_run_git(&["rev-parse", "--git-path", name])
                // `--git-path` は cwd 基準の相対パスを返すことがある。
                // git コマンドは repo_path を cwd として実行しているので、そこを基準に解決する
                // (絶対パスが返った場合は join がそちらを優先するため、どちらでも正しい)。
                .map(|path| self.repo_path.join(path).exists())
                .unwrap_or(false)
        })
    }

    /// HEAD が有効なコミットを指しているか確認
    fn has_head_commit(&self) -> Result<bool, AppError> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let msg = if stderr.is_empty() {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if stdout.is_empty() {
                        format!("exit code: {}", output.status)
                    } else {
                        stdout
                    }
                } else {
                    stderr
                };
                Err(AppError::GitError(msg))
            }
        }
    }

    /// 指定されたコミットハッシュの差分を取得
    pub fn get_commit_diff_by_hash(&self, hash: &str) -> Result<String, AppError> {
        self.apply_all_filters(&self.get_raw_commit_diff_by_hash(hash)?)
    }

    /// 各コミットの raw diff を結合してから一度だけ要約・予算配分する。
    pub fn get_commit_diffs_by_hashes(&self, hashes: &[String]) -> Result<String, AppError> {
        let mut combined = String::new();
        for hash in hashes {
            let raw = self.get_raw_commit_diff_by_hash(hash)?;
            if !raw.trim().is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&raw);
            }
        }
        self.apply_all_filters(&combined)
    }

    fn get_raw_commit_diff_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // 検証と同時にコミット ID へ解決し、以降は解決済みの ID だけを git に渡す。
        // 注釈付きタグ(`--generate-for v1` など)を生のまま `git show` に渡すと、diff の前に
        // タグオブジェクト自身(`tag v1` / `Tagger: 名前 <メール>` / タグ本文)が出力され、
        // そのまま AI へ送られる。署名者を `--no-show-signature` で外しているのと同じ理由で、
        // コミット以外の情報はプロンプトに入れない。
        let commit = self.resolve_commit_hash(hash)?;

        // git show でそのコミットの差分を取得。
        // 解決済みの ID でも `--end-of-options` の後ろに置き、オプションとして解釈される
        // 余地を残さない(`--output=<path>` のような名前の ref を生のまま渡すと
        // `git show --output=<path>` で任意ファイルを上書きできてしまう)。
        // 末尾の `--` はリビジョンとパスの区切り。無いと `HEAD` などと同名のファイルが
        // 作業ツリーにあるだけで「リビジョンかファイル名か曖昧」として失敗する。
        // log.showRoot=false はルートコミットの差分を隠すため、--root で明示的に取得する。
        let mut args = vec!["show", "--root", "--no-show-signature", "--format=", "-U0"];
        args.extend_from_slice(DIFF_FORMAT_ARGS);
        args.extend_from_slice(&["--end-of-options", &commit, "--"]);
        self.run_git_raw(&args)
    }

    /// 指定されたコミットハッシュのメッセージを取得
    pub fn get_commit_message_by_hash(&self, hash: &str) -> Result<String, AppError> {
        // 差分の取得(get_commit_diff_by_hash)と同じく、解決済みのコミット ID だけを渡す
        let commit = self.resolve_commit_hash(hash)?;

        // `--end-of-options` でオプションとして解釈される余地を残さず、
        // 末尾の `--` で同名のファイルとの曖昧さを断つ。
        self.run_git(&[
            "log",
            "--no-show-signature",
            "-1",
            "--format=%s",
            "--end-of-options",
            &commit,
            "--",
        ])
    }

    /// reword対象として有効なコミットか確認（現在のHEAD履歴上に存在するか）
    fn validate_reword_target_hash(&self, hash: &str) -> Result<(), AppError> {
        // まずコミットハッシュが有効か確認
        self.verify_commit_hash(hash)?;

        // 履歴外のコミットをreword対象にすると誤ったコミット位置が算出されるため、祖先関係を厳密に確認。
        // hash をオプション位置に置かないよう `--end-of-options` の後ろに渡す（オプション注入による
        // 祖先判定の撹乱・他コマンドとの一貫性のため）。
        let ancestor_output = Command::new("git")
            .args([
                "merge-base",
                "--is-ancestor",
                "--end-of-options",
                hash,
                "HEAD",
            ])
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

        // 位置 n は後段で `HEAD~n`（first-parent 深さ）として消費されるため、ここでも
        // `--first-parent` で数える。`rev-list --count hash..HEAD`（全祖先のトポロジカル数）
        // だと、マージで取り込まれた側ブランチのコミットが余分に加算されて n が過大になり、
        // `HEAD~n` が無効化して不可解な git エラー（fatal: ambiguous argument 'HEAD~n..HEAD'）
        // を招く。first-parent 経路で hash..HEAD のコミット数を数え、+1 で対象自身の位置にする。
        // hash 由来の `<hash>..HEAD` をオプション位置に置かないよう `--end-of-options` の後ろに渡す
        // （他のハッシュ受け取り経路とそろえてオプション注入を一律で防ぐ）。
        let count_str = self.run_git(&[
            "rev-list",
            "--count",
            "--first-parent",
            "--end-of-options",
            &format!("{}..HEAD", hash),
        ])?;
        let count: usize = count_str
            .parse()
            .map_err(|_| AppError::GitError("Failed to parse commit count".to_string()))?;

        // count はそのコミットより新しい（first-parent 経路上の）コミット数なので、+1で位置になる
        Ok(count + 1)
    }

    /// 指定されたコミットハッシュからHEADまでにマージコミットが含まれているかチェック
    pub fn has_merge_commits_in_range_by_hash(&self, hash: &str) -> Result<bool, AppError> {
        // 対象コミット自身がマージコミットの場合も拒否対象に含める必要があるため、
        // ハッシュ指定を HEAD からの位置に変換し、既存の位置ベース判定を再利用する。
        let n = self.get_commit_position_by_hash(hash)?;
        self.has_merge_commits_in_range(n)
    }

    /// 指定されたコミットハッシュのメッセージを変更（rebase使用）
    pub fn reword_commit_by_hash(&self, hash: &str, new_message: &str) -> Result<(), AppError> {
        // 位置算出より先に rebase 進行中を弾く。rebase 中の HEAD は detached で
        // 別の位置を指すため、先に位置を求めると「履歴外」を意味する
        // InvalidRewordTarget が返り、本当の原因（rebase 中）が見えなくなる。
        if self.rebase_in_progress() {
            return Err(AppError::RebaseInProgress);
        }

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

        // 実行前から rebase が進行中なら何もせずに止める。
        // この関数は rebase 失敗時に必ず `git rebase --abort` を実行するが、
        // 既存の rebase が進行中だと git-sc 側の `git rebase -i` はそもそも
        // 「別の rebase が進行中」として起動に失敗する。その失敗を自分の rebase の
        // 失敗と取り違えて abort すると、ユーザーが解決作業中だった rebase
        // (コンフリクト解決の途中経過を含む)を破棄してしまう。
        // amend 経路(n == 1)でも rebase 中の HEAD は detached で意図しない位置を
        // 指すため、同じくここで弾く。
        if self.rebase_in_progress() {
            return Err(AppError::RebaseInProgress);
        }

        // n=1 の場合は amend で処理。ただし HEAD がマージコミットの場合は
        // 通常の reword と同様に拒否する（CLAUDE.md の API 保証を維持するため、
        // 内部経路で呼ばれても安全に動作するようにする）。
        if n == 1 {
            if self.has_merge_commits_in_range(1)? {
                return Err(AppError::HasMergeCommits);
            }
            // depth 1 の shallow clone では HEAD 自身が境界になり、amend でも親が外れる
            self.refuse_shallow_boundary(1)?;
            return self.amend_commit_message(new_message);
        }

        // n は HEAD~n（first-parent 深さ）として使うため、判定も first-parent 深さで揃える。
        let first_parent_total = self.get_head_first_parent_count()?;
        if n > first_parent_total {
            return Err(AppError::InvalidRewordTarget);
        }

        // マージコミットをチェック
        if self.has_merge_commits_in_range(n)? {
            return Err(AppError::HasMergeCommits);
        }

        // shallow clone の境界は first-parent の終端に見え、下の `--root` 経路に入ってしまう
        self.refuse_shallow_boundary(n)?;

        // 一意な一時ファイルにメッセージを保存
        let msg_file = TempRewordMessageFile::create(new_message)?;

        // GIT_SEQUENCE_EDITOR: 先頭行(対象のコミット)は `pick` のまま残し、その直後に
        // メッセージを差し替える `exec` 行を差し込む。範囲内の他のコミットには触れない。
        //
        // 以前は先頭行を `reword` に置換し、GIT_EDITOR でメッセージファイルを流し込んでいた。
        // `reword` だと git は「エディタで編集したメッセージ」として扱い、既定の
        // `commit.cleanup=strip` が `#` で始まる行をコメントとして削る。件名が
        // `#123 fix login` なら件名が消えて本文の 1 行目が件名に繰り上がり、件名だけなら
        // `Aborting commit due to empty commit message.` で失敗していた。HEAD の reword
        // (n == 1) と通常のコミットは `-m` なので whitespace の整形だけで `#` 行が残り、
        // 経路によって結果が食い違っていた。
        //
        // exec で実行する `git commit --amend --only --allow-empty -F "$GIT_SC_MSG_FILE"` は
        // エディタを開かないので、整形は n == 1 の `-m` と同じ whitespace になる。
        // - `--only`: n == 1 の amend と同じく index の内容を混ぜない
        // - `--allow-empty`: 空のコミットも reword できた旧方式を保つ(付けないと
        //   「amend すると空になる」として拒否される)
        // - メッセージファイルのパスは todo に書かず `$GIT_SC_MSG_FILE` のまま残し、exec を
        //   実行するシェルに展開させる(パスをシェル文字列に埋め込まない)。スクリプト中の
        //   `\"\$GIT_SC_MSG_FILE\"` は内側の sh で `"$GIT_SC_MSG_FILE"` という文字列になる
        // `-c commit.cleanup=whitespace` を rebase 全体に付ける案は、範囲内で pick されるだけの
        // 他のコミットのメッセージ(末尾空白・連続空行)まで書き換えるので採らない。exec は
        // 対象の 1 コミットにしか効かない。exec が失敗する(commit-msg フックの拒否など)と
        // rebase はそこで止まって非ゼロで終わるため、下の失敗経路で abort される。
        //
        // `rebase.abbreviateCommands=false` を後段の rebase 起動側で強制するため、
        // todo は必ず `pick <hash>` 形式で出力される（短縮形 `p` は出ない）。
        //
        // Windows でも同じ sh 版を使う。git はエディタ文字列にシェルメタ文字が含まれると
        // OS を問わず `sh -c '<editor> "$@"' <editor> <path>` の形で起動する
        // (Git for Windows では同梱の sh)。以前の PowerShell 版はその外側の sh に
        // `$lines` / `$args` を先に展開されて中身が消え、さらに PowerShell の `-Command` は
        // 後続引数を `$args` へ渡さないため、二重に機能していなかった。
        // Git for Windows は sh / sed / mv を同梱しているので Unix 版へ一本化する。
        //
        // 先頭行が `pick ` でなければ非ゼロ終了して rebase ごと失敗させる。先頭に別の行が
        // 入っていると、exec が対象とは別のコミットの直後に入って別のコミットのメッセージを
        // 書き換えるか、何も書き換わらないまま rebase が exit 0 で完了する(「成功と表示
        // されるのにメッセージが変わらない」無言の no-op)。既知の原因(`rebase.rebaseMerges`)は
        // `--no-rebase-merges` で塞いだが、todo の先頭へ別の前置きを足す設定が将来増えても、
        // 黙って誤動作する代わりに失敗として現れる。
        let sequence_editor = concat!(
            r#"sh -c 'IFS= read -r first < "$1" || exit 1; "#,
            r#"case "$first" in "pick "*) ;; "#,
            r#"*) echo "git-sc: unexpected rebase todo (first line is not a pick)" >&2; exit 1;; esac; "#,
            r#"{ printf "%s\n" "$first" "exec git commit --amend --only --allow-empty -F \"\$GIT_SC_MSG_FILE\""; "#,
            r#"sed 1d "$1"; } > "$1.git-sc" && mv "$1.git-sc" "$1"' --"#
        )
        .to_string();

        // GIT_EDITOR: メッセージは exec の `-F` で渡すので、エディタを使う場面は無い。
        // 万一呼ばれても対話にならないよう、git が「何もしないエディタ」として扱う `:` にする
        // (ユーザーの core.editor が GUI エディタでも開かない)。
        let editor = ":";

        // git rebase -i を実行（最古コミット対象時は --root を使う）。
        // `rebase.abbreviateCommands=false` で todo を必ず `pick` で出力させ、
        // 短縮形 `p` を前提にした先頭行の検査が誤って失敗するのを防ぐ。
        // `--no-autosquash` は `rebase.autoSquash=true` のユーザー設定を隔離するために必須。
        // autosquash が効くと範囲内の `fixup!`/`squash!` コミットが todo 上で並べ替えられて
        // 対象コミットへ黙って溶け込み、reword 以外の履歴改変が起きる(対象の直後に並ぶため、
        // 差し込んだ exec の後で amend 済みのコミットへ結合される)。
        // `--no-rebase-merges` は `rebase.rebaseMerges=true` のユーザー設定を隔離するために
        // 必須。これが効くと todo の先頭に `label onto` / `reset onto` の前置きが入り、
        // 1 行目を対象にした差し込みがずれる。以前の `reword` 置換では空振りしても todo は
        // 全行 `pick` のまま有効なので rebase は exit 0 で完了し、git-sc は「成功」と表示
        // しながらメッセージが一切変わらなかった(= 無言の no-op)。ユーザーは reword 済みと
        // 信じて force push まで案内される。
        // rebase.updateRefs=false で他ブランチの自動更新を防ぐ。
        // --no-update-refs が無い Git 2.38 未満でも動くよう、設定を実行時だけ上書きする。
        let mut rebase_cmd = Command::new("git");
        rebase_cmd
            .args(["-c", "rebase.abbreviateCommands=false"])
            .args(["-c", "rebase.updateRefs=false"]);
        rebase_cmd
            .arg("rebase")
            .arg("-i")
            .arg("--no-autosquash")
            .arg("--no-rebase-merges");
        if n == first_parent_total {
            rebase_cmd.arg("--root");
        } else {
            rebase_cmd.arg(format!("HEAD~{}", n));
        }

        // 起動の直前にもう一度確認する。関数の入口で確認してから、履歴の検査と
        // 一時ファイル作成を挟むぶん時間が空くため、その隙に別プロセスが rebase を
        // 始めていると、後段の無条件 abort がその rebase を壊す。ここまで詰めても
        // 「確認してから git が rebase ディレクトリを作るまで」の窓は原理的に残るが、
        // git 側に外から取れるロックが無いのでこれが実務上の下限。
        if self.rebase_in_progress() {
            return Err(AppError::RebaseInProgress);
        }

        let output = rebase_cmd
            .env("GIT_SEQUENCE_EDITOR", &sequence_editor)
            .env("GIT_EDITOR", editor)
            .env("EDITOR", editor)
            .env("GIT_SC_MSG_FILE", msg_file.path().as_os_str())
            .current_dir(&self.repo_path)
            .output()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            // CONFLICTでも commit-msg hook 失敗等でも、rebaseが途中の状態を残さないよう
            // 失敗時は常に --abort を試みる（rebase未開始の場合は no-op で安全）。
            let _ = Command::new("git")
                .args(["rebase", "--abort"])
                .current_dir(&self.repo_path)
                .output();

            if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
                return Err(AppError::RebaseConflict);
            }

            return Err(AppError::GitError(stderr.to_string()));
        }

        Ok(())
    }

    /// コミットメッセージを変更（amend）
    fn amend_commit_message(&self, new_message: &str) -> Result<(), AppError> {
        self.run_git_ok(&["commit", "--amend", "--only", "-m", new_message])
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
    use rstest::rstest;
    use std::process::Command;

    /// 署名の表示設定にかかわらず、件名と差分だけを取得する。
    #[test]
    #[cfg(unix)]
    fn test_signature_verification_output_is_not_included_in_prompts() {
        let dir = setup_temp_git_repo();
        let repo = dir.path();
        let key = repo.join("signing-key");
        let generated = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output();
        let generated = match generated {
            Ok(output) => output,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("ssh-keygen がないため、実署名を使う回帰テストをスキップします");
                return;
            }
            Err(e) => panic!("署名鍵を作成できませんでした: {e}"),
        };
        assert!(generated.status.success());
        let allowed_signers = repo.join("allowed-signers");
        std::fs::write(
            &allowed_signers,
            format!(
                "test@example.com {}",
                std::fs::read_to_string(key.with_extension("pub")).unwrap()
            ),
        )
        .unwrap();
        run_git_in(repo, &["config", "gpg.format", "ssh"]);
        run_git_in(repo, &["config", "user.signingkey", key.to_str().unwrap()]);
        run_git_in(repo, &["config", "commit.gpgsign", "true"]);
        run_git_in(repo, &["config", "log.showSignature", "true"]);
        run_git_in(
            repo,
            &[
                "config",
                "gpg.ssh.allowedSignersFile",
                allowed_signers.to_str().unwrap(),
            ],
        );
        std::fs::write(repo.join("file.txt"), "signed change\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "feat: signed subject"]);
        let service = GitService::with_repo_path(repo.to_path_buf());

        assert_eq!(
            service.get_recent_commits(1).unwrap(),
            ["feat: signed subject"]
        );
        assert_eq!(
            service.get_commit_message_by_hash("HEAD").unwrap(),
            "feat: signed subject"
        );
        let diff = service.get_last_commit_diff().unwrap();
        assert!(diff.starts_with("diff --git "), "{diff}");
        assert!(!diff.contains("signature"), "{diff}");
        assert!(!diff.contains("test@example.com"), "{diff}");
    }

    // ============================================================
    // summarize_diff のテスト
    // ============================================================

    #[test]
    fn test_staged_lockfile_copy_with_git_copy_detection() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);
        run_git_in(repo, &["config", "diff.renames", "copies"]);
        let content = "dependency content\n".repeat(100);
        std::fs::write(repo.join("template.txt"), &content).unwrap();
        run_git_in(repo, &["add", "template.txt"]);
        run_git_in(repo, &["commit", "-m", "initial"]);
        std::fs::write(repo.join("Cargo.lock"), &content).unwrap();
        std::fs::write(repo.join("template.txt"), format!("{content}new line\n")).unwrap();
        run_git_in(repo, &["add", "template.txt", "Cargo.lock"]);
        let diff = GitService::with_repo_path(repo.to_path_buf())
            .get_staged_diff()
            .unwrap();
        assert!(
            diff.contains("[Lockfile] copied: template.txt -> Cargo.lock"),
            "{diff}"
        );
        assert!(diff.contains("+new line"));
        // 元ファイルの hunk ヘッダーには既存行が表示されることがある。
        assert!(!diff.contains("+dependency content"));
    }

    #[test]
    fn test_summarize_lockfile_copy_keeps_source_and_destination() {
        let diff = "diff --git a/Cargo.lock b/nested/Cargo.lock\n\
                    similarity index 100%\n\
                    copy from Cargo.lock\n\
                    copy to nested/Cargo.lock";
        assert_eq!(
            GitService::summarize_diff(diff),
            "[Lockfile] copied: Cargo.lock -> nested/Cargo.lock"
        );
    }

    #[test]
    fn test_summarize_lockfile_control_characters_stay_on_one_line() {
        let diff = r#"diff --git "a/line\nbreak.lock" "b/line\nbreak.lock"
@@ -1 +1 @@
-old
+new"#;
        assert_eq!(
            GitService::summarize_diff(diff),
            r#"[Lockfile] modified: "line\nbreak.lock""#
        );
    }

    #[test]
    fn test_summarize_lockfile_rename_decodes_both_paths() {
        let diff = r#"diff --git "a/\303\251.lock" "b/new\303\251.lock"
similarity index 100%
rename from "\303\251.lock"
rename to "new\303\251.lock""#;
        assert_eq!(
            GitService::summarize_diff(diff),
            "[Lockfile] renamed: é.lock -> newé.lock"
        );
    }

    #[rstest]
    #[case(".lock")]
    #[case("Cargo.lock")]
    #[case("yarn.lock")]
    #[case("Gemfile.lock")]
    #[case("uv.lock")]
    #[case("composer.lock")]
    #[case("bun.lock")]
    #[case("bun.lockb")]
    #[case(".lockb")]
    #[case("project.lockfile")]
    #[case(".lockfile")]
    #[case("package-lock.json")]
    #[case("npm-shrinkwrap.json")]
    #[case("pnpm-lock.yaml")]
    #[case("go.sum")]
    #[case("Package.resolved")]
    fn test_summarize_lockfile_contents(#[case] name: &str) {
        for path in [name.to_string(), format!("nested dir/{name}")] {
            let diff = format!(
                "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1 @@\n-old dependency\n+new dependency"
            );
            assert_eq!(
                GitService::summarize_diff(&diff),
                format!("[Lockfile] modified: {path}")
            );
        }
    }

    #[rstest]
    #[case("package.json")]
    #[case("Cargo.toml")]
    #[case("go.mod")]
    #[case("Cargo.lock.backup")]
    #[case("vendor.lock/src.rs")]
    #[case("pnpm-lock.yaml/readme.md")]
    fn test_summarize_diff_preserves_non_lockfiles(#[case] path: &str) {
        let diff = format!("diff --git a/{path} b/{path}\n@@ -1 +1 @@\n-old\n+new");
        assert_eq!(GitService::summarize_diff(&diff), diff);
    }

    #[rstest]
    #[case("new file mode 100644", "added")]
    #[case("deleted file mode 100644", "deleted")]
    #[case("old mode 100644\nnew mode 100755", "modified")]
    #[case("Binary files a/bun.lockb and b/bun.lockb differ", "modified")]
    #[case("GIT binary patch\nliteral 4\nencoded-content", "modified")]
    fn test_summarize_lockfile_change_kind(#[case] metadata: &str, #[case] kind: &str) {
        let diff = format!("diff --git a/bun.lockb b/bun.lockb\n{metadata}\n+content");
        assert_eq!(
            GitService::summarize_diff(&diff),
            format!("[Lockfile] {kind}: bun.lockb")
        );
    }

    #[rstest]
    #[case("old name.txt", "new name.lock")]
    #[case("old name.lock", "new name.txt")]
    #[case("Cargo.lock", "new b/path.lock")]
    #[case("Cargo.lock", "new b/path.txt")]
    fn test_summarize_lockfile_rename_checks_both_paths(#[case] from: &str, #[case] to: &str) {
        let diff = format!(
            "diff --git a/{from} b/{to}\nsimilarity index 90%\nrename from {from}\nrename to {to}\n@@ -1 +1 @@\n-old\n+new"
        );
        assert_eq!(
            GitService::summarize_diff(&diff),
            format!("[Lockfile] renamed: {from} -> {to}")
        );
    }

    #[rstest]
    #[case("diff --cc nested dir/Cargo.lock", "nested dir/Cargo.lock")]
    #[case("diff --combined pnpm-lock.yaml", "pnpm-lock.yaml")]
    #[case(
        r#"diff --git "a/\303\251/Cargo.lock" "b/\303\251/Cargo.lock""#,
        "é/Cargo.lock"
    )]
    fn test_summarize_lockfile_combined_and_quoted_paths(#[case] header: &str, #[case] path: &str) {
        let diff = format!("{header}\nindex abc,def..123\n@@@ -1 -1 +1 @@@\n++content");
        assert_eq!(
            GitService::summarize_diff(&diff),
            format!("[Lockfile] modified: {path}")
        );
    }

    #[test]
    fn test_lockfile_summary_precedes_truncation_and_respects_ignore() {
        let dir = tempfile::TempDir::new().unwrap();
        run_git_in(dir.path(), &["init"]);
        let service = GitService::with_repo_path(dir.path().to_path_buf());
        let text_diff = "diff --git a/src/main.rs b/src/main.rs\n@@ -1 +1 @@\n-old code\n+new code";
        let diff = format!(
            "diff --git a/Cargo.lock b/Cargo.lock\n@@ -1 +1 @@\n{}\n{text_diff}",
            "+dependency content\n".repeat(MAX_DIFF_CHARS)
        );
        assert_eq!(
            service.apply_all_filters(&diff).unwrap(),
            format!("[Lockfile] modified: Cargo.lock\n{text_diff}")
        );
        std::fs::write(dir.path().join(".git-sc-ignore"), "Cargo.lock\n").unwrap();
        assert_eq!(
            service.apply_all_filters(&diff).unwrap(),
            format!("diff --git a/Cargo.lock b/Cargo.lock\n{IGNORED_CONTENT_NOTICE}\n{text_diff}")
        );
    }

    #[test]
    fn test_filter_binary_diff_empty_input() {
        let result = GitService::summarize_diff("");
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
        let result = GitService::summarize_diff(diff);
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

        let result = GitService::summarize_diff(diff);

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

        let result = GitService::summarize_diff(diff);
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

        let result = GitService::summarize_diff(diff);

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

        let result = GitService::summarize_diff(diff);

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

        let result = GitService::summarize_diff(diff);
        assert_eq!(result, "[Binary] added: new_image.png");
    }

    #[test]
    fn test_filter_binary_diff_deleted_file() {
        let diff = r#"diff --git a/old_image.png b/old_image.png
deleted file mode 100644
Binary files a/old_image.png and /dev/null differ"#;

        let result = GitService::summarize_diff(diff);
        assert_eq!(result, "[Binary] deleted: old_image.png");
    }

    #[test]
    fn test_filter_binary_diff_renamed_file() {
        let diff = r#"diff --git a/old_name.png b/new_name.png
similarity index 100%
rename from old_name.png
rename to new_name.png"#;

        let result = GitService::summarize_diff(diff);
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

        let result = GitService::summarize_diff(diff);
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

        let result = GitService::summarize_diff(diff);
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
    fn test_verify_repository_rejects_plain_git_file() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join(".git"), "not a gitdir").unwrap();
        let service = GitService {
            repo_path: temp_dir.path().to_path_buf(),
        };

        let result = service.verify_repository();

        assert!(matches!(result, Err(AppError::NotGitRepository)));
    }

    #[test]
    fn test_get_current_branch() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        run_git_in(repo, &["symbolic-ref", "HEAD", "refs/heads/test-branch"]);
        run_git_in(repo, &["commit", "--allow-empty", "-m", "initial"]);
        let service = GitService::with_repo_path(repo.to_path_buf());

        assert_eq!(service.get_current_branch().as_deref(), Some("test-branch"));
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

    #[test]
    fn test_get_recent_commits_empty_repo_without_locale_specific_stderr_parsing() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let commits = service.get_recent_commits(5).unwrap();
        assert!(commits.is_empty());
    }

    // ============================================================
    // branch_exists のテスト
    // ============================================================

    #[test]
    fn test_branch_exists_main() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        run_git_in(repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        run_git_in(repo, &["commit", "--allow-empty", "-m", "initial"]);
        let service = GitService::with_repo_path(repo.to_path_buf());

        assert!(service.branch_exists("main"));
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
    fn test_large_diff_overview_keeps_rename_copy_and_combined_counts() {
        let rename = "diff --git a/old.txt b/new b/path.txt\nsimilarity index 90%\nrename from old.txt\nrename to new b/path.txt\n@@ -1 +1,2 @@\n-old\n+new\n+++literal plus";
        let copy = "diff --git a/source.txt b/copy.txt\nsimilarity index 100%\ncopy from source.txt\ncopy to copy.txt";
        let combined = "diff --cc \"caf\\303\\251.txt\"\nindex a,b..c\n@@@ -1 -1 +1 @@@\n--removed\n++added\n  context";
        let large = format!(
            "diff --git a/a.txt b/a.txt\n@@ -0,0 +1 @@\n+{}",
            "長".repeat(MAX_DIFF_CHARS)
        );
        let result = GitService::truncate_diff(&format!("{large}\n{rename}\n{copy}\n{combined}"));
        for summary in [
            "old.txt -> new b/path.txt | +2 -1",
            "source.txt -> copy.txt | +0 -0",
            "café.txt | +1 -1 (combined rows)",
        ] {
            assert!(result.contains(summary), "missing {summary}: {result}");
        }
    }

    #[test]
    fn test_large_diff_overview_never_truncates_inventory() {
        let diff = (0..400)
            .map(|index| format!("diff --git a/file-{index:03}.txt b/file-{index:03}.txt\n@@ -1 +1 @@\n-old\n+new\n"))
            .collect::<String>();
        let result = GitService::truncate_diff(&diff);
        // 一覧自体が本文予算を超える場合も名前と行数を削らない。
        for index in 0..400 {
            assert!(result.contains(&format!("file-{index:03}.txt | +1 -1")));
        }
        assert!(result.contains("file bodies omitted or truncated"));
        let (_, body) = result
            .split_once("body budget 10000 characters]\n\n")
            .unwrap();
        assert!(body.chars().count() <= MAX_DIFF_CHARS);
    }

    #[test]
    fn test_large_diff_shares_body_budget_across_large_files() {
        let diff = ["first", "second"]
            .into_iter()
            .map(|name| {
                format!(
                    "diff --git a/{name}.txt b/{name}.txt\n@@ -0,0 +1,1500 @@\n{}",
                    format!("+{name} body line\n").repeat(1500)
                )
            })
            .collect::<String>();
        let result = GitService::truncate_diff(&diff);
        let (_, body) = result
            .split_once("body budget 10000 characters]\n\n")
            .unwrap();
        assert!(body.contains("+first body line\n"));
        assert!(body.contains("+second body line\n"));
        assert!(body.chars().count() <= MAX_DIFF_CHARS);
        assert!(result.contains("2 of 2 file bodies omitted or truncated"));
        assert_eq!(result.matches("[body truncated]").count(), 2);
    }

    #[test]
    fn test_many_diff_files_keep_inventory_without_exceeding_body_budget() {
        let count = MAX_DIFF_CHARS + 2;
        let diff = (0..count)
            .map(|index| {
                format!("diff --git a/file-{index}.txt b/file-{index}.txt\n@@ -0,0 +1 @@\n+added\n")
            })
            .collect::<String>();
        let result = GitService::truncate_diff(&diff);
        let (overview, body) = result
            .split_once("body budget 10000 characters]\n\n")
            .unwrap();
        assert_eq!(
            overview
                .lines()
                .filter(|line| line.starts_with("[File] "))
                .count(),
            count
        );
        assert!(body.chars().count() <= MAX_DIFF_CHARS);
    }

    #[test]
    fn test_large_diff_preserves_spaces_and_escapes_control_characters_in_paths() {
        let mut diff = format!(
            "diff --git a/large.txt b/large.txt\n@@ -0,0 +1 @@\n+{}\n",
            "x".repeat(MAX_DIFF_CHARS)
        );
        for name in [
            " leading.txt",
            "trailing.txt ",
            " both.txt ",
            "line\n[File] | \"name\".txt",
        ] {
            let a = if name.contains(['\n', '"']) {
                format!("{:?}", format!("a/{name}"))
            } else {
                format!("a/{name}")
            };
            let b = if name.contains(['\n', '"']) {
                format!("{:?}", format!("b/{name}"))
            } else {
                format!("b/{name}")
            };
            diff.push_str(&format!("diff --git {a} {b}\n@@ -0,0 +1 @@\n+change\n"));
        }
        let result = GitService::truncate_diff(&diff);
        for expected in [
            "[File] \" leading.txt\" | +1 -0",
            "[File] \"trailing.txt \" | +1 -0",
            "[File] \" both.txt \" | +1 -0",
            "[File] \"line\\n[File] | \\\"name\\\".txt\" | +1 -0",
        ] {
            assert!(result.contains(expected), "missing {expected}: {result}");
        }
        assert_eq!(
            result
                .lines()
                .filter(|line| line.starts_with("[File] "))
                .count(),
            5
        );
        assert!(!result.contains(" -> "));
        assert_eq!(
            GitService::extract_combined_diff_path("diff --cc  name "),
            Some(" name ".to_string())
        );
        assert_eq!(
            GitService::extract_file_paths_from_diff_header("diff --git a/old  \"b/new\\tname\""),
            Some(("old ".to_string(), "new\tname".to_string()))
        );
    }

    #[test]
    fn test_large_diff_allocation_keeps_crlf_and_small_file_bodies() {
        let diff = format!(
            "diff --git a/a.txt b/a.txt\n@@ -0,0 +1 @@\n+{}\ndiff --git a/z.txt b/z.txt\n@@ -1 +1 @@\n-old\n+new\r\n",
            "文".repeat(MAX_DIFF_CHARS)
        );
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("z.txt | +1 -1"));
        assert!(result.contains("+new\r\n") || result.ends_with("+new\r"));
        assert!(!result.contains('\u{fffd}'));
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    fn test_ignored_excerpts_do_not_spend_body_or_separator_budget(#[case] only_ignored: bool) {
        let dir = tempfile::tempdir().unwrap();
        let mut builder = GitignoreBuilder::new(dir.path());
        builder.add_line(None, "data/").unwrap();
        let ignore = builder.build().unwrap();
        // 除外ファイルの区切りだけで予算を使い切る件数も扱う。
        let count = MAX_DIFF_CHARS + 2;
        let mut diff = (0..count)
            .map(|index| {
                format!(
                    "diff --git a/data/{index}.txt b/data/{index}.txt\n@@ -0,0 +1 @@\n+hidden\n"
                )
            })
            .collect::<String>();
        let visible = "diff --git a/visible.txt b/visible.txt\n@@ -0,0 +1 @@\n+visible change";
        if !only_ignored {
            diff.push_str(visible);
        }
        let filtered = GitService::filter_ignored_files(&diff, &ignore);
        let result = GitService::truncate_diff_with_source(&filtered, &diff, Some(&ignore));
        let (overview, body) = result
            .split_once("body budget 10000 characters]\n\n")
            .unwrap();
        assert_eq!(overview.matches(IGNORED_CONTENT_NOTICE).count(), count);
        assert!(!overview.contains("[body truncated]"));
        assert!(!body.contains("hidden"));
        assert_eq!(body, if only_ignored { "" } else { visible });
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
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_nested() {
        let header = "diff --git a/path/to/nested/file.txt b/path/to/nested/file.txt";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path/to/nested/file.txt".to_string()));
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

    #[test]
    fn test_commit_by_hash_blocks_git_option_injection() {
        // `--output=<path>` という名前の ref が存在すると verify_commit_hash を通過しうるが、
        // hash を git show / git log のオプション位置へ渡さないため、任意ファイルを書き込まない。
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        run_git_in(repo, &["add", "a.txt"]);
        run_git_in(repo, &["commit", "-m", "init"]);

        // 攻撃用 ref を HEAD に向けて作成する。`--output=PWNED` は git の cwd(=repo)相対で
        // 書き込まれるため、注入が成立するとマーカーファイルが生成される。
        run_git_in(repo, &["update-ref", "refs/heads/--output=PWNED", "HEAD"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let malicious = "--output=PWNED";
        let marker = repo.join("PWNED");

        // diff 取得・メッセージ取得のいずれもマーカーを書き込まない（注入が無効化されている）
        let _ = service.get_commit_diff_by_hash(malicious);
        assert!(
            !marker.exists(),
            "get_commit_diff_by_hash がオプション注入で任意ファイルを書き込んだ"
        );

        let _ = service.get_commit_message_by_hash(malicious);
        assert!(
            !marker.exists(),
            "get_commit_message_by_hash がオプション注入で任意ファイルを書き込んだ"
        );
    }

    #[test]
    fn test_squash_base_branch_blocks_git_option_injection() {
        // squash の base_branch(`--squash <branch>` のユーザー入力)も、ハッシュ経路と同様に
        // git のオプション位置へ渡さない。`--end-of-options` により `-` 始まりの値は
        // revision として解決され、オプション注入(任意ファイル書き込み等)を防ぐ。
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        run_git_in(repo, &["add", "a.txt"]);
        run_git_in(repo, &["commit", "-m", "init"]);

        // オプション風の名前のブランチを HEAD に向けて実在させる。
        run_git_in(repo, &["update-ref", "refs/heads/--output=PWNED", "HEAD"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let malicious = "--output=PWNED";
        let marker = repo.join("PWNED");

        // branch_exists: `--end-of-options` により revision として解決されるため、
        // 実在するブランチは true を返し、かつマーカーは書き込まれない。
        // (修正前は `--output=PWNED` がオプション扱いされて false になっていた)
        assert!(
            service.branch_exists(malicious),
            "--end-of-options 後なら実在ブランチとして解決されるべき"
        );
        assert!(
            !marker.exists(),
            "branch_exists がオプション注入で任意ファイルを書き込んだ"
        );

        // get_merge_base: revision として解決され、HEAD との merge-base(=HEAD 自身)を返す。
        let mb = service.get_merge_base(malicious, "HEAD");
        assert!(
            mb.is_ok(),
            "--end-of-options 後なら revision として merge-base が解決されるべき"
        );
        assert!(
            !marker.exists(),
            "get_merge_base がオプション注入で任意ファイルを書き込んだ"
        );
    }

    // ============================================================
    // filter_ignored_files のテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_no_ignore() {
        // ignoreパターンがない場合（実際にはGitignore構築が必要なので
        // summarize_diffと同様の動作を確認）
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
        // この場合、apply_all_filtersはsummarize_diff + truncate_diffのみ適用
        let result = service.apply_all_filters(diff).unwrap();
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

    #[rstest]
    #[case(
        "diff --git a/docs/added.md b/docs/added.md",
        "new file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/docs/added.md\n@@ -0,0 +1 @@\n+PRIVATE-ADDED"
    )]
    #[case(
        "diff --git a/docs/deleted.md b/docs/deleted.md",
        "deleted file mode 100644\nindex 1234567..0000000\n--- a/docs/deleted.md\n+++ /dev/null\n@@ -1 +0,0 @@\n-PRIVATE-DELETED"
    )]
    #[case(
        "diff --git a/docs/modified.md b/docs/modified.md",
        "index 1234567..7654321\n@@ -1 +1 @@\n-PRIVATE-OLD\n+PRIVATE-NEW"
    )]
    #[case(
        "diff --git a/docs/script.sh b/docs/script.sh",
        "old mode 100644\nnew mode 100755"
    )]
    #[case(
        "diff --git a/docs/old.md b/src/new.md",
        "similarity index 80%\nrename from docs/old.md\nrename to src/new.md\n@@ -1 +1 @@\n-PRIVATE-OLD\n+PRIVATE-NEW"
    )]
    #[case(
        "diff --git a/src/old.md b/docs/copied.md",
        "similarity index 80%\ncopy from src/old.md\ncopy to docs/copied.md\n@@ -1 +1 @@\n-PRIVATE-OLD\n+PRIVATE-NEW"
    )]
    #[case(
        "diff --combined docs/merged.md",
        "index 1234567,7654321..abcdef0\n@@@ -1 -1 +1 @@@\n++PRIVATE-MERGED"
    )]
    fn test_filter_ignored_files_retains_only_header(#[case] header: &str, #[case] body: &str) {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "docs").unwrap();
        let ignore = builder.build().unwrap();
        let diff = format!("{header}\n{body}");
        assert_eq!(
            GitService::filter_ignored_files(&diff, &ignore),
            format!("{header}\n{IGNORED_CONTENT_NOTICE}")
        );
    }

    #[test]
    fn test_filter_ignored_files_negated_pattern_restores_contents() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "docs/*").unwrap();
        builder.add_line(None, "!docs/keep.md").unwrap();
        let ignore = builder.build().unwrap();
        let visible =
            "diff --git a/docs/keep.md b/docs/keep.md\n@@ -1 +1 @@\n-old\n+VISIBLE-CONTENT";
        let diff = format!(
            "diff --git a/docs/hidden.md b/docs/hidden.md\n@@ -1 +1 @@\n-PRIVATE-OLD\n+PRIVATE-NEW\n{visible}"
        );
        assert_eq!(
            GitService::filter_ignored_files(&diff, &ignore),
            format!(
                "diff --git a/docs/hidden.md b/docs/hidden.md\n{IGNORED_CONTENT_NOTICE}\n{visible}"
            )
        );
    }

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
        assert!(result.contains(&format!(
            "diff --git a/Cargo.lock b/Cargo.lock\n{IGNORED_CONTENT_NOTICE}"
        )));
        assert!(!result.contains("lock change"));
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
        assert_eq!(
            result,
            format!("diff --git a/Cargo.lock b/Cargo.lock\n{IGNORED_CONTENT_NOTICE}")
        );
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

    #[test]
    fn test_filter_ignored_files_matches_rename_destination_path() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "generated/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/generated/main.rs\n\
                    similarity index 100%\n\
                    rename from src/main.rs\n\
                    rename to generated/main.rs\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("{}\n{IGNORED_CONTENT_NOTICE}", diff.lines().next().unwrap())
        );
    }

    #[test]
    fn test_filter_ignored_files_excludes_combined_diff_block() {
        // マージコミットへの `git show` は combined diff (`diff --cc <path>`) を出す。
        // `diff --git` しか見ていなかった頃はブロックを 1 つも認識できず、
        // .git-sc-ignore の除外が丸ごと素通りして秘密が AI へ送られていた。
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "secrets/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --cc secrets/key.txt\n\
                    index a3d4dbe,f9a1862..59be8f3\n\
                    --- a/secrets/key.txt\n\
                    +++ b/secrets/key.txt\n\
                    @@@ -1,1 -1,1 +1,1 @@@\n\
                    - MAIN-SIDE-SECRET\n\
                    \x20-FEATURE-SIDE-SECRET\n\
                    ++RESOLVED-SECRET\n\
                    diff --cc public.txt\n\
                    index 1111111,2222222..3333333\n\
                    --- a/public.txt\n\
                    +++ b/public.txt\n\
                    @@@ -1,1 -1,1 +1,1 @@@\n\
                    ++RESOLVED-PUBLIC\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(
            !result.contains("RESOLVED-SECRET"),
            "combined diff で除外が効いていない: {result}"
        );
        assert!(
            result.contains(&format!(
                "diff --cc secrets/key.txt\n{IGNORED_CONTENT_NOTICE}"
            )),
            "除外対象のパスと本文の省略通知が残っていない: {result}"
        );
        assert!(
            result.contains("RESOLVED-PUBLIC"),
            "除外していないブロックまで消えている: {result}"
        );
    }

    #[test]
    fn test_extract_combined_diff_path_handles_spaces_and_quoting() {
        // Git は combined diff のパスを、非 ASCII を含むときだけクォートする
        // (スペースだけならクォートしない)。実測した両方の形を押さえる。
        assert_eq!(
            GitService::extract_combined_diff_path("diff --cc plain.txt"),
            Some("plain.txt".to_string())
        );
        assert_eq!(
            GitService::extract_combined_diff_path("diff --cc sp ace.txt"),
            Some("sp ace.txt".to_string())
        );
        assert_eq!(
            GitService::extract_combined_diff_path("diff --cc \"nonascii-\\303\\251.txt\""),
            Some("nonascii-é.txt".to_string())
        );
        // `diff --combined` 形式も同じ扱い
        assert_eq!(
            GitService::extract_combined_diff_path("diff --combined a/b.txt"),
            Some("a/b.txt".to_string())
        );
        // 通常の diff --git ヘッダーは combined ではない
        assert_eq!(
            GitService::extract_combined_diff_path("diff --git a/x.txt b/x.txt"),
            None
        );
    }

    #[test]
    fn test_filter_ignored_files_matches_asymmetric_rename_with_spaces() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "generated/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/old file.txt b/generated/new file.txt\n\
                    similarity index 100%\n\
                    rename from old file.txt\n\
                    rename to generated/new file.txt\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("{}\n{IGNORED_CONTENT_NOTICE}", diff.lines().next().unwrap())
        );
    }

    #[test]
    fn test_filter_ignored_files_matches_unquoted_space_to_quoted_rename() {
        // 「非クォート(スペース含み) → クォート」混在ヘッダーの rename でも
        // 移動先パスが ignore 照合されて本文が省略されること
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "generated/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/old file.txt \"b/generated/\\343\\203\\206.txt\"\n\
                    similarity index 100%\n\
                    rename from old file.txt\n\
                    rename to generated/テ.txt\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("{}\n{IGNORED_CONTENT_NOTICE}", diff.lines().next().unwrap())
        );
    }

    #[test]
    fn test_filter_ignored_files_matches_unquoted_path_with_spaces() {
        // 非クォートでスペースを含むパス（Git はスペースをクォートしない）を
        // .git-sc-ignore で正しく除外できることを確認する
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "logs/**").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/logs/access log.txt b/logs/access log.txt\n\
                    index 111..222 100644\n\
                    --- a/logs/access log.txt\n\
                    +++ b/logs/access log.txt\n\
                    @@ -1 +1,2 @@\n\
                    +new line\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("{}\n{IGNORED_CONTENT_NOTICE}", diff.lines().next().unwrap())
        );
    }

    #[test]
    fn test_filter_ignored_files_keeps_unquoted_path_with_spaces_when_not_ignored() {
        // 非クォートでスペースを含むパスが ignore 対象でない場合は保持されること
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "*.lock").unwrap();
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/docs/read me.md b/docs/read me.md\n\
                    index 111..222 100644\n\
                    --- a/docs/read me.md\n\
                    +++ b/docs/read me.md\n\
                    @@ -1 +1,2 @@\n\
                    +new line\n";

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("docs/read me.md"));
        assert!(result.contains("new line"));
    }

    // ============================================================
    // summarize_diff の追加エッジケーステスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_text_with_binary_keyword_in_filename() {
        let diff = r#"diff --git a/binary_helper.rs b/binary_helper.rs
index 1234567..abcdefg 100644
--- a/binary_helper.rs
+++ b/binary_helper.rs
@@ -1 +1,2 @@
+// binary helper code"#;

        let result = GitService::summarize_diff(diff);
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

        let result = GitService::summarize_diff(diff);
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
        let result = gs.apply_all_filters(diff).unwrap();
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_binary_excluded() {
        let gs = GitService::default();
        let diff = "diff --git a/image.png b/image.png\nnew file mode 100644\nBinary files /dev/null and b/image.png differ\ndiff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let result = gs.apply_all_filters(diff).unwrap();
        assert!(result.contains("[Binary]"));
        assert!(result.contains("+new"));
    }

    #[test]
    fn test_apply_all_filters_empty() {
        let gs = GitService::default();
        let result = gs.apply_all_filters("").unwrap();
        assert!(result.is_empty());
    }

    /// ignoreパターンがバイナリファイルにも正しく適用されることを検証
    ///
    /// filter_ignored_files が summarize_diff より先に実行されなければ、
    /// バイナリファイルの diff --git ヘッダーがサマリー行に変換されてしまい、
    /// ignore パターンが適用されなくなる。
    #[test]
    fn test_apply_all_filters_binary_ignored_by_pattern() {
        use tempfile::TempDir;

        // 一時Gitリポジトリを作成
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // .git-sc-ignore に *.png を追加
        std::fs::write(dir.path().join(".git-sc-ignore"), "*.png\n").unwrap();

        let gs = GitService {
            repo_path: dir.path().to_path_buf(),
        };

        // バイナリPNGとテキストファイルを含むdiff
        let diff = concat!(
            "diff --git a/image.png b/image.png\n",
            "new file mode 100644\n",
            "Binary files /dev/null and b/image.png differ\n",
            "diff --git a/src/main.rs b/src/main.rs\n",
            "--- a/src/main.rs\n",
            "+++ b/src/main.rs\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n",
        );

        let result = gs.apply_all_filters(diff).unwrap();

        // *.png はパスだけ残し、バイナリサマリーやメタデータも省略する。
        assert!(
            result.contains(&format!(
                "diff --git a/image.png b/image.png\n{IGNORED_CONTENT_NOTICE}"
            )),
            "image.png path should remain in result: {}",
            result
        );
        assert!(!result.contains("[Binary]"));
        assert!(!result.contains("Binary files"));
        assert!(!result.contains("new file mode"));
        // テキストファイルは残る
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+new"));
    }

    // ============================================================
    // extract_file_path_from_diff_header: ルートファイルのテスト
    // ============================================================

    #[test]
    fn test_extract_file_path_root_file() {
        let header = "diff --git a/Cargo.toml b/Cargo.toml";
        assert_eq!(
            GitService::extract_file_path_from_diff_header(header),
            Some("Cargo.toml".to_string())
        );
    }

    #[test]
    fn test_extract_file_path_with_quoted_spaces() {
        let header =
            r#"diff --git "a/path with spaces/file name.txt" "b/path with spaces/file name.txt""#;
        assert_eq!(
            GitService::extract_file_path_from_diff_header(header),
            Some("path with spaces/file name.txt".to_string())
        );
    }

    /// テスト用の一時Gitリポジトリで git コマンドを実行する
    fn run_git_in(path: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// テスト用の一時Gitリポジトリで git コマンドを実行し、stdout を返す
    fn git_output_in(path: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// テスト用の一時 Git リポジトリを作成する
    fn setup_temp_git_repo() -> tempfile::TempDir {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        temp_dir
    }

    /// テスト用スクリプトを書き込み、実行可能にする
    fn write_test_script(repo: &std::path::Path, relative_path: &str, body: &str) -> PathBuf {
        let script_path = repo.join(relative_path);
        if let Some(parent) = script_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&script_path, body).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions).unwrap();
        }

        script_path
    }

    #[test]
    fn test_run_prefix_script_resolves_relative_path_from_git_root() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let nested_dir = repo.join("nested");
        std::fs::create_dir_all(&nested_dir).unwrap();

        #[cfg(windows)]
        let relative_script_path = "scripts\\prefix.cmd";
        #[cfg(not(windows))]
        let relative_script_path = "scripts/prefix.sh";

        #[cfg(windows)]
        let script_body = "@echo off\r\necho RELATIVE-PREFIX\r\n";
        #[cfg(not(windows))]
        let script_body = "#!/bin/sh\necho RELATIVE-PREFIX\n";

        write_test_script(repo, relative_script_path, script_body);

        let service = GitService {
            repo_path: nested_dir,
        };

        let result = service.run_prefix_script(relative_script_path, "origin", "main");

        match result {
            Some(ScriptResult::Prefix(prefix)) => assert_eq!(prefix.trim(), "RELATIVE-PREFIX"),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn test_run_prefix_script_uses_git_root_as_working_directory() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let nested_dir = repo.join("nested");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(repo.join("repo-root.marker"), "marker").unwrap();

        #[cfg(windows)]
        let script_relative_path = "scripts\\cwd-check.cmd";
        #[cfg(not(windows))]
        let script_relative_path = "scripts/cwd-check.sh";

        #[cfg(windows)]
        let script_body =
            "@echo off\r\nif exist repo-root.marker (echo ROOT-CWD) else exit /b 2\r\n";
        #[cfg(not(windows))]
        let script_body =
            "#!/bin/sh\nif [ -f repo-root.marker ]; then\n  echo ROOT-CWD\nelse\n  exit 2\nfi\n";

        let script_path = write_test_script(repo, script_relative_path, script_body);

        let service = GitService {
            repo_path: nested_dir,
        };

        let result = service.run_prefix_script(
            script_path.to_str().unwrap(),
            "https://github.com/example/repo.git",
            "feature/test",
        );

        match result {
            Some(ScriptResult::Prefix(prefix)) => assert_eq!(prefix.trim(), "ROOT-CWD"),
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn test_run_prefix_script_strips_line_ending_only() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        #[cfg(windows)]
        let script_relative_path = "scripts\\prefix-newline.cmd";
        #[cfg(not(windows))]
        let script_relative_path = "scripts/prefix-newline.sh";

        #[cfg(windows)]
        let script_body = "@echo off\r\npowershell -NoProfile -Command \"[Console]::Out.Write('TICKET-123 '); [Console]::Out.WriteLine()\"\r\n";
        #[cfg(not(windows))]
        let script_body = "#!/bin/sh\nprintf 'TICKET-123 \\n'\n";

        write_test_script(repo, script_relative_path, script_body);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.run_prefix_script(script_relative_path, "origin", "main");

        assert_eq!(
            result,
            Some(ScriptResult::Prefix("TICKET-123 ".to_string()))
        );
    }

    #[test]
    fn test_run_prefix_script_exit_one_returns_failed() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        #[cfg(windows)]
        let script_relative_path = "scripts\\exit-one.cmd";
        #[cfg(not(windows))]
        let script_relative_path = "scripts/exit-one.sh";

        #[cfg(windows)]
        let script_body = "@echo off\r\nexit /b 1\r\n";
        #[cfg(not(windows))]
        let script_body = "#!/bin/sh\nexit 1\n";

        write_test_script(repo, script_relative_path, script_body);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.run_prefix_script(script_relative_path, "origin", "main");

        assert_eq!(result, Some(ScriptResult::Failed));
    }

    #[test]
    fn test_run_prefix_script_non_one_exit_returns_none() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        #[cfg(windows)]
        let script_relative_path = "scripts\\exit-two.cmd";
        #[cfg(not(windows))]
        let script_relative_path = "scripts/exit-two.sh";

        #[cfg(windows)]
        let script_body = "@echo off\r\nexit /b 2\r\n";
        #[cfg(not(windows))]
        let script_body = "#!/bin/sh\nexit 2\n";

        write_test_script(repo, script_relative_path, script_body);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.run_prefix_script(script_relative_path, "origin", "main");

        assert_eq!(result, None);
    }

    /// 最古コミットを含む範囲でも reword できることを検証する
    #[test]
    fn test_reword_commit_oldest_commit() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.reword_commit(2, "rewritten root");
        assert!(result.is_ok());

        let output = Command::new("git")
            .args(["log", "--reverse", "--format=%s"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        let messages = String::from_utf8_lossy(&output.stdout);
        assert_eq!(messages.lines().next(), Some("rewritten root"));
    }

    #[test]
    fn test_amend_commit_preserves_existing_staged_changes() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        run_git_in(repo, &["add", "staged.txt"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        service.amend_commit("amended").unwrap();

        let head_files = git_output_in(repo, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(!head_files.lines().any(|line| line == "staged.txt"));
        let staged_files = git_output_in(repo, &["diff", "--cached", "--name-only"]);
        assert_eq!(staged_files, "staged.txt");
    }

    #[test]
    fn test_reword_head_preserves_existing_staged_changes() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        run_git_in(repo, &["add", "staged.txt"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        service.reword_commit(1, "reworded head").unwrap();

        let message = git_output_in(repo, &["log", "-1", "--format=%s"]);
        assert_eq!(message, "reworded head");
        let head_files = git_output_in(repo, &["show", "--name-only", "--format=", "HEAD"]);
        assert!(!head_files.lines().any(|line| line == "staged.txt"));
        let staged_files = git_output_in(repo, &["diff", "--cached", "--name-only"]);
        assert_eq!(staged_files, "staged.txt");
    }

    #[test]
    fn test_temp_reword_message_file_uses_unique_paths() {
        let first = TempRewordMessageFile::create("first").unwrap();
        let second = TempRewordMessageFile::create("second").unwrap();

        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::read_to_string(first.path()).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(second.path()).unwrap(), "second");
    }

    /// HEAD がマージコミットの場合は `reword_commit(1, ...)` を直接呼んでも
    /// `HasMergeCommits` で拒否されることを確認する。
    /// app.rs 側で事前チェックされる経路だけでなく、ライブラリ層単独でも
    /// CLAUDE.md の保証を維持する境界値テスト。
    #[test]
    fn test_reword_commit_n1_rejects_merge_head() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        // 初期コミット
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        run_git_in(repo, &["add", "a.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);
        let base = git_output_in(repo, &["branch", "--show-current"]);

        // feature ブランチで分岐
        run_git_in(repo, &["checkout", "-b", "feature/merge-head"]);
        std::fs::write(repo.join("feature.txt"), "f\n").unwrap();
        run_git_in(repo, &["add", "feature.txt"]);
        run_git_in(repo, &["commit", "-m", "feature commit"]);

        // base ブランチ側にも変更を加える
        run_git_in(repo, &["checkout", &base]);
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        run_git_in(repo, &["add", "b.txt"]);
        run_git_in(repo, &["commit", "-m", "base commit"]);

        // --no-ff でマージコミットを作成（HEAD がマージコミットになる）
        let merge = Command::new("git")
            .args(["merge", "--no-ff", "feature/merge-head", "-m", "merge"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(merge.status.success(), "merge failed");

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=1 の amend 経路でもマージコミットなら拒否される
        let result = service.reword_commit(1, "renamed merge");
        assert!(matches!(result, Err(AppError::HasMergeCommits)));

        // reword_commit_by_hash 経由でも同様
        let head_hash = git_output_in(repo, &["rev-parse", "HEAD"]);
        let result = service.reword_commit_by_hash(&head_hash, "renamed merge");
        assert!(matches!(result, Err(AppError::HasMergeCommits)));
    }

    /// reword 中に commit-msg hook が失敗しても、rebase が中途状態で残らないことを検証する。
    /// 失敗時に `git rebase --abort` が呼ばれないと、以降の git 操作が
    /// 「rebase in progress」で全て失敗する状態になる。
    #[cfg(unix)]
    #[test]
    fn test_reword_aborts_rebase_when_commit_msg_hook_rejects() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);
        // hooks がデフォルト有効か明示するために core.hooksPath を未設定のままにする
        // （新しい git のデフォルト .git/hooks を利用）

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        // 全てのコミットメッセージを拒否する commit-msg hook を仕込む
        let hook_path = repo.join(".git").join("hooks").join("commit-msg");
        std::fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms).unwrap();

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=2 で rebase ルートに入る。commit-msg hook が拒否するので必ず失敗する。
        let result = service.reword_commit(2, "rewritten root");
        assert!(
            result.is_err(),
            "commit-msg hook 拒否時は reword が失敗するはず"
        );

        // 中途半端な rebase 状態が残っていないことを確認する
        let rebase_merge = repo.join(".git").join("rebase-merge");
        let rebase_apply = repo.join(".git").join("rebase-apply");
        assert!(
            !rebase_merge.exists(),
            ".git/rebase-merge/ が残存している（rebase --abort が呼ばれていない）"
        );
        assert!(
            !rebase_apply.exists(),
            ".git/rebase-apply/ が残存している（rebase --abort が呼ばれていない）"
        );

        // 後続の通常 git 操作（commit など）が実行できる状態であることを確認する
        let status = Command::new("git")
            .args(["status", "--porcelain=v2", "--branch"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(status.status.success());
        let stdout = String::from_utf8_lossy(&status.stdout);
        // rebase 中なら "# branch.head (detached)" のようになるが、ここでは branch 行があるはず
        assert!(
            !stdout.contains("rebase in progress"),
            "git status が rebase 進行中を報告している"
        );
    }

    /// `rebase.autosquash=true` のユーザー設定下でも、reword が範囲内の
    /// `fixup!` コミットを勝手に取り込まないことを確認する。
    /// `--no-autosquash` がないと todo が並べ替えられ、fixup コミットが
    /// 対象コミットへ溶け込んで履歴から消える(reword 以外の履歴改変)。
    #[test]
    fn test_reword_with_autosquash_config_keeps_fixup_commits_intact() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_git_in(repo, &["add", "base.txt"]);
        run_git_in(repo, &["commit", "-m", "base commit"]);

        std::fs::write(repo.join("target.txt"), "target\n").unwrap();
        run_git_in(repo, &["add", "target.txt"]);
        run_git_in(repo, &["commit", "-m", "target commit"]);
        let target_hash = git_output_in(repo, &["rev-parse", "HEAD"]);

        std::fs::write(repo.join("middle.txt"), "middle\n").unwrap();
        run_git_in(repo, &["add", "middle.txt"]);
        run_git_in(repo, &["commit", "-m", "middle commit"]);

        // 対象コミットに対する fixup! コミット(autosquash の並べ替え対象)
        std::fs::write(repo.join("fixup.txt"), "fixup\n").unwrap();
        run_git_in(repo, &["add", "fixup.txt"]);
        run_git_in(repo, &["commit", "-m", "fixup! target commit"]);

        // ユーザー設定で autosquash を有効化した状態を再現
        run_git_in(repo, &["config", "rebase.autosquash", "true"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        service
            .reword_commit_by_hash(&target_hash, "reworded target")
            .unwrap();

        // fixup! コミットが独立したまま残り、対象だけが reword されていること
        let messages = git_output_in(repo, &["log", "--format=%s"]);
        let subjects: Vec<&str> = messages.lines().collect();
        assert_eq!(
            subjects,
            vec![
                "fixup! target commit",
                "middle commit",
                "reworded target",
                "base commit"
            ]
        );
    }

    /// ユーザーの updateRefs 設定で、指定していないブランチまで移動しない。
    /// HEAD の amend、通常の rebase、最古コミットの --root 経路を確認する。
    #[rstest::rstest]
    #[case(1)]
    #[case(2)]
    #[case(4)]
    fn test_reword_with_update_refs_config_preserves_other_branches(#[case] n: usize) {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        let mut other_branches = Vec::new();
        for index in 0..4 {
            let file = format!("file-{index}.txt");
            std::fs::write(repo.join(&file), format!("content {index}\n")).unwrap();
            run_git_in(repo, &["add", &file]);
            run_git_in(repo, &["commit", "-m", &format!("commit {index}")]);
            let branch = format!("saved-{index}");
            run_git_in(repo, &["branch", &branch]);
            other_branches.push((branch, git_output_in(repo, &["rev-parse", "HEAD"])));
        }
        run_git_in(repo, &["config", "rebase.updateRefs", "true"]);
        let branch_before = git_output_in(repo, &["branch", "--show-current"]);
        let tree_before = git_output_in(repo, &["rev-parse", "HEAD^{tree}"]);
        let service = GitService::with_repo_path(repo.to_path_buf());

        service.reword_commit(n, "rewritten target").unwrap();

        for (branch, original_hash) in other_branches {
            assert_eq!(
                git_output_in(repo, &["rev-parse", &branch]),
                original_hash,
                "reword が別ブランチ {branch} を移動した"
            );
        }
        assert_eq!(
            git_output_in(repo, &["branch", "--show-current"]),
            branch_before
        );
        assert_eq!(
            git_output_in(repo, &["rev-parse", "HEAD^{tree}"]),
            tree_before
        );
        let mut expected = (0..4)
            .rev()
            .map(|index| format!("commit {index}"))
            .collect::<Vec<_>>();
        expected[n - 1] = "rewritten target".to_string();
        assert_eq!(
            git_output_in(repo, &["log", "--format=%s"]),
            expected.join("\n")
        );
        assert!(!service.rebase_in_progress());
    }

    #[test]
    fn test_temp_reword_message_file_is_cleaned_up_on_drop() {
        let path = {
            let file = TempRewordMessageFile::create("cleanup").unwrap();
            let path = file.path().to_path_buf();
            assert!(path.exists());
            path
        };

        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_temp_reword_message_file_is_not_readable_by_group_or_others() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempRewordMessageFile::create("secret message").unwrap();
        let mode = std::fs::metadata(tmp.path()).unwrap().permissions().mode();

        assert_eq!(mode & 0o077, 0);
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

    #[rstest::rstest]
    #[case("true")]
    #[case("false")]
    fn test_get_last_commit_diff_on_root_commit(#[case] show_root: &str) {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial commit"]);
        run_git_in(repo, &["config", "log.showRoot", show_root]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let diff = service.get_last_commit_diff().unwrap();
        assert!(diff.contains("diff --git"));
        assert!(diff.contains("+first"));
        // amend / reword / generate-for が使う各 API でルートの差分を保持する。
        assert_eq!(service.get_commit_diff_by_hash("HEAD").unwrap(), diff);
        assert_eq!(
            service
                .get_commit_diffs_by_hashes(&["HEAD".to_string()])
                .unwrap(),
            diff
        );
    }

    // ============================================================
    // extract_file_path_from_diff_header: エスケープ・エッジケース
    // ============================================================

    #[test]
    fn test_extract_file_path_escaped_characters() {
        // エスケープされたバックスラッシュ
        let header = r#"diff --git "a/path\\with\\backslash.txt" "b/path\\with\\backslash.txt""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some(r"path\with\backslash.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_escaped_tab() {
        let header = "diff --git \"a/file\\twith\\ttab.txt\" \"b/file\\twith\\ttab.txt\"";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("file\twith\ttab.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_octal_escaped_utf8() {
        let header =
            r#"diff --git "a/\346\227\245\346\234\254.txt" "b/\346\227\245\346\234\254.txt""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("日本.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_unclosed_quote() {
        // 閉じ引用符がない場合は None
        let header = "diff --git \"a/unclosed b/unclosed";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_empty_quoted_path() {
        // 引用符内が "a/" のみ（パスが空）
        let header = "diff --git \"a/\" \"b/\"";
        let result = GitService::extract_file_path_from_diff_header(header);
        // strip_prefix("a/") 後に空文字列 → None
        assert_eq!(result, Some(String::new()));
    }

    #[test]
    fn test_extract_file_path_trailing_backslash() {
        // 末尾がエスケープ文字で終わる（不完全なエスケープ）
        let header = "diff --git \"a/file\\";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, None);
    }

    // ============================================================
    // summarize_diff: パス抽出失敗時のフォールバック
    // ============================================================

    #[test]
    fn test_filter_binary_diff_unknown_path_fallback() {
        // extract_file_path_from_diff_header が None を返すケース
        let diff = "diff --git \n\
                     Binary files /dev/null and b/something differ";
        let result = GitService::summarize_diff(diff);
        assert!(result.contains("[Binary]"));
        assert!(result.contains("unknown"));
    }

    // ============================================================
    // filter_ignored_files: パス抽出失敗時はスキップしない
    // ============================================================

    #[test]
    fn test_filter_ignored_files_path_extraction_fails() {
        // パス抽出が失敗してもブロックは保持される
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "*.log").unwrap();
        let ignore = builder.build().unwrap();
        let diff = "diff --git \n\
                     some content line";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("some content line"));
    }

    #[test]
    fn test_filter_ignored_files_with_octal_escaped_utf8_path() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "日本.txt").unwrap();
        let ignore = builder.build().unwrap();
        let diff = concat!(
            "diff --git \"a/\\346\\227\\245\\346\\234\\254.txt\" ",
            "\"b/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "index 1234567..89abcde 100644\n",
            "--- \"a/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "+++ \"b/\\346\\227\\245\\346\\234\\254.txt\"\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new\n"
        );

        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("{}\n{IGNORED_CONTENT_NOTICE}", diff.lines().next().unwrap())
        );
    }

    // ============================================================
    // truncate_diff: マルチバイト文字の境界
    // ============================================================

    #[test]
    fn test_truncate_diff_multibyte_characters() {
        // 日本語文字（各3バイト）を含むdiffが文字数で正しく切り詰められる
        let line = "日本語のテスト行です。変更がありました。\n";
        // MAX_DIFF_CHARS (10000) を超えるまで繰り返す
        let chars_per_line = line.chars().count();
        let repeat_count = (MAX_DIFF_CHARS / chars_per_line) + 2;
        let diff: String = line.repeat(repeat_count);
        assert!(diff.chars().count() > MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        // 切り詰めメッセージが含まれる
        assert!(result.contains("diff truncated"));
        // パニックせずに正常に処理される
        // 結果の文字数がMAX_DIFF_CHARS以下（切り詰めメッセージ分を除く）
        let without_msg = result.split("\n\n... (diff truncated").next().unwrap();
        assert!(without_msg.chars().count() <= MAX_DIFF_CHARS);
    }

    // ============================================================
    // decode_quoted_diff_path: エスケープシーケンスのエッジケース
    // ============================================================

    #[test]
    fn test_decode_quoted_path_newline_escape() {
        // \n エスケープが改行文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\nname.txt""#);
        assert_eq!(result, Some("a/file\nname.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_carriage_return_escape() {
        // \r エスケープが復帰文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\rname.txt""#);
        assert_eq!(result, Some("a/file\rname.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_escaped_quote() {
        // \" エスケープがクォート文字にデコードされる
        let result = GitService::decode_quoted_diff_path(r#"a/file\"name.txt""#);
        assert_eq!(result, Some("a/file\"name.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_incomplete_octal() {
        // 8進数エスケープが2桁しかない場合は None
        let result = GitService::decode_quoted_diff_path(r#"a/\34""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_non_octal_after_backslash_digit() {
        // 8進数の2桁目が非8進数文字の場合は None
        let result = GitService::decode_quoted_diff_path(r#"a/\389.txt""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_unknown_escape_passthrough() {
        // 未知のエスケープ文字はそのまま通過する
        let result = GitService::decode_quoted_diff_path(r#"a/file\xname.txt""#);
        assert_eq!(result, Some("a/filexname.txt".to_string()));
    }

    /// Git の `quote.c` が使う文字エスケープ 9 種すべてを復号する。
    ///
    /// `\a` `\b` `\v` `\f` を取りこぼすと未知エスケープ扱いでリテラルの英字になり
    /// (`se\acret/` → `seacret/`)、`.git-sc-ignore` の照合が実在しないパスに対して
    /// 行われて除外が無言で外れる。git 2.55 で実際にこの形が出力されることを確認済み。
    #[rstest]
    #[case(r#"a/bel\ax.txt""#, "a/bel\u{07}x.txt")]
    #[case(r#"a/bs\bx.txt""#, "a/bs\u{08}x.txt")]
    #[case(r#"a/tab\tx.txt""#, "a/tab\tx.txt")]
    #[case(r#"a/nl\nx.txt""#, "a/nl\nx.txt")]
    #[case(r#"a/vt\vx.txt""#, "a/vt\u{0b}x.txt")]
    #[case(r#"a/ff\fx.txt""#, "a/ff\u{0c}x.txt")]
    #[case(r#"a/cr\rx.txt""#, "a/cr\rx.txt")]
    #[case(r#"a/dq\"x.txt""#, "a/dq\"x.txt")]
    #[case(r#"a/bsl\\x.txt""#, "a/bsl\\x.txt")]
    fn test_decode_quoted_path_decodes_all_git_char_escapes(
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            GitService::decode_quoted_diff_path(input),
            Some(expected.to_string()),
            "Git の文字エスケープ {input} が正しく復号されていない"
        );
    }

    #[test]
    fn test_decode_quoted_path_no_closing_quote() {
        // 閉じクォートがない場合は None
        let result = GitService::decode_quoted_diff_path("a/file.txt");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_empty_content() {
        // 空のパス（即座に閉じクォート）は None
        let result = GitService::decode_quoted_diff_path("\"");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_max_octal_value() {
        // 最大8進数値 \377 = 255 (u8最大値)
        let result = GitService::decode_quoted_diff_path(r#"a/\377""#);
        assert!(result.is_some());
        let decoded = result.unwrap();
        // "a/" (2バイト) + 0xFF (1バイト)
        // from_utf8_lossy が 0xFF を U+FFFD に変換する
        assert!(decoded.starts_with("a/"));
    }

    #[test]
    fn test_decode_quoted_path_mixed_escapes() {
        // 複数のエスケープが混在するパス
        let result = GitService::decode_quoted_diff_path(r#"a/\346\227\245\\path\tfile.txt""#);
        assert!(result.is_some());
        let decoded = result.unwrap();
        assert!(decoded.contains("日"));
        assert!(decoded.contains("\\"));
        assert!(decoded.contains("\t"));
    }

    // ============================================================
    // has_merge_commits_in_range: 実Gitリポジトリでの境界テスト
    // ============================================================

    #[test]
    fn test_has_merge_commits_in_range_zero() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=0 は InvalidRewordTarget エラー
        let result = service.has_merge_commits_in_range(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_exceeds_total() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "initial"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n=2 だがコミットは1つだけ → InvalidRewordTarget
        let result = service.has_merge_commits_in_range(2);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_equal_total() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // n == total_commits（最古コミット含む範囲）→ 正常動作
        let result = service.has_merge_commits_in_range(2);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // マージコミットなし
    }

    #[test]
    fn test_has_merge_commits_in_range_no_merge() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "first\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);

        std::fs::write(repo.join("file.txt"), "second\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "second"]);

        std::fs::write(repo.join("file.txt"), "third\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "third"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // 直近1コミットの範囲にマージコミットなし
        let result = service.has_merge_commits_in_range(1);
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    // ============================================================
    // get_commit_diff_by_hash: エッジケーステスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_valid() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test commit"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("HEAD");
        assert!(result.is_ok());
        assert!(result.unwrap().contains("file.txt"));
    }

    #[test]
    fn test_get_commit_diff_by_hash_invalid_in_temp_repo() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("0000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    // ============================================================
    // verify_commit_hash: 境界テスト
    // ============================================================

    #[test]
    fn test_verify_commit_hash_valid_head() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        assert!(service.verify_commit_hash("HEAD").is_ok());
    }

    #[test]
    fn test_verify_commit_hash_invalid() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // 無効なハッシュ形式（16進数ではない文字を含む）
        let result = service.verify_commit_hash("not-a-valid-hash");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::InvalidCommitHash(hash) => {
                assert_eq!(hash, "not-a-valid-hash");
            }
            other => panic!("Expected InvalidCommitHash, got {:?}", other),
        }
    }

    // ============================================================
    // truncate_diff: 切り詰めロジックのテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_short_input_unchanged() {
        let diff = "short diff\n";
        let result = GitService::truncate_diff(diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_exact_limit_unchanged() {
        // ちょうど MAX_DIFF_CHARS 文字の入力はそのまま返る
        let diff: String = "a".repeat(10000);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_over_limit_truncated_at_newline() {
        // MAX_DIFF_CHARS + 1 文字で、改行で切り詰められる
        let mut diff = String::new();
        for i in 0..200 {
            diff.push_str(&format!("line {}\n", i));
        }
        // 10000文字を超えるようにパディング
        while diff.len() <= 10000 {
            diff.push_str("padding line with some content here\n");
        }
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // 切り詰め位置は改行の直前
        let content_before_marker = result.split("\n\n... (diff truncated").next().unwrap();
        assert!(!content_before_marker.ends_with('\n'));
    }

    #[test]
    fn test_truncate_diff_no_newline_in_truncated() {
        // 改行なしで10000文字を超える場合
        let diff: String = "a".repeat(10001);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
    }

    #[test]
    fn test_truncate_diff_multibyte_japanese_chars() {
        // マルチバイト文字（日本語）を含むdiffが正しく切り詰められる
        // "あ\n" は2文字なので、5001回で10002文字 > MAX_DIFF_CHARS
        let mut diff = String::new();
        for _ in 0..5001 {
            diff.push_str("あ\n");
        }
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("... (diff truncated: exceeded 10000 characters)"));
        // パニックしないことが重要
    }

    // ============================================================
    // extract_file_path_from_diff_header: パス抽出テスト（追加）
    // ============================================================

    #[test]
    fn test_extract_file_path_nested_directory() {
        let header = "diff --git a/src/ai/service.rs b/src/ai/service.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/ai/service.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_no_prefix() {
        // "diff --git " のみで後続がない場合
        let result = GitService::extract_file_path_from_diff_header("diff --git ");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_file_path_quoted_with_escape() {
        let header = r#"diff --git "a/src/te\\st.rs" "b/src/te\\st.rs""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/te\\st.rs".to_string()));
    }

    // ============================================================
    // decode_quoted_diff_path: クォートパスのデコードテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_tab_escape() {
        let result = GitService::decode_quoted_diff_path(r#"a/te\tst.rs""#);
        assert_eq!(result, Some("a/te\tst.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_quote_escape() {
        let result = GitService::decode_quoted_diff_path(r#"a/te\"st.rs""#);
        assert_eq!(result, Some("a/te\"st.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_trailing_backslash() {
        // バックスラッシュの後に何もない → None
        let result = GitService::decode_quoted_diff_path("a/test\\");
        assert_eq!(result, None);
    }

    // ============================================================
    // summarize_diff: バイナリファイル除外テスト（追加）
    // ============================================================

    #[test]
    fn test_filter_binary_diff_multiple_binary_files() {
        let diff = "diff --git a/a.png b/a.png\nnew file mode 100644\nBinary files /dev/null and b/a.png differ\ndiff --git a/b.jpg b/b.jpg\nBinary files a/b.jpg and b/b.jpg differ";
        let result = GitService::summarize_diff(diff);
        assert!(result.contains("[Binary] added: a.png"));
        assert!(result.contains("[Binary] modified: b.jpg"));
    }

    // ============================================================
    // filter_ignored_files: ignore パターン除外テスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_no_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_filter_ignored_files_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff =
            "diff --git a/debug.log b/debug.log\n--- a/debug.log\n+++ b/debug.log\n-old\n+new";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert_eq!(
            result,
            format!("diff --git a/debug.log b/debug.log\n{IGNORED_CONTENT_NOTICE}")
        );
    }

    #[test]
    fn test_filter_ignored_files_partial_match() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\ndiff --git a/debug.log b/debug.log\n--- a/debug.log\n+++ b/debug.log\n-x\n+y";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains(&format!(
            "diff --git a/debug.log b/debug.log\n{IGNORED_CONTENT_NOTICE}"
        )));
        assert!(!result.contains("--- a/debug.log"));
        assert!(!result.contains("-x"));
        assert!(!result.contains("+y"));
    }

    // ============================================================
    // decode_quoted_diff_path: 網羅的なデコードテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_ascii_path() {
        // 通常のASCIIパスが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path/to/file.rs""#);
        assert_eq!(result, Some("path/to/file.rs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_japanese_utf8_octal() {
        // 日本語UTF-8のオクタルエスケープが正しくデコードされる（テスト = \343\203\206\343\202\271\343\203\210）
        let result =
            GitService::decode_quoted_diff_path(r#"\343\203\206\343\202\271\343\203\210.txt""#);
        assert_eq!(result, Some("テスト.txt".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_backslash_escape() {
        // バックスラッシュのエスケープが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\\with\\backslash""#);
        assert_eq!(result, Some("path\\with\\backslash".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_tab_and_newline_in_path() {
        // タブと改行のエスケープが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\twith\ttabs""#);
        assert_eq!(result, Some("path\twith\ttabs".to_string()));
    }

    #[test]
    fn test_decode_quoted_path_no_closing_quote_plain() {
        // 閉じクォートがない場合は None を返す
        let result = GitService::decode_quoted_diff_path("no-closing-quote");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_empty_path_only_quote() {
        // 即座に閉じクォートでパスが空の場合は None を返す
        let result = GitService::decode_quoted_diff_path("\"");
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_incomplete_octal_two_digits() {
        // オクタルシーケンスが3桁に満たない場合は None を返す
        let result = GitService::decode_quoted_diff_path(r#"\34""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_invalid_octal_digit() {
        // 8進数の範囲外の数字（8, 9）が含まれる場合は None を返す
        let result = GitService::decode_quoted_diff_path(r#"\389""#);
        assert_eq!(result, None);
    }

    #[test]
    fn test_decode_quoted_path_escaped_quotes_in_path() {
        // パス内のエスケープされたクォートが正しくデコードされる
        let result = GitService::decode_quoted_diff_path(r#"path\"with\"quotes""#);
        assert_eq!(result, Some("path\"with\"quotes".to_string()));
    }

    // ============================================================
    // summarize_diff: リネーム済みテキストファイルのテスト
    // ============================================================

    #[test]
    fn test_filter_binary_diff_renamed_text_file_preserved() {
        // リネームされたテキストファイルはそのまま保持される
        let diff = "diff --git a/old_name.rs b/new_name.rs\n\
                     similarity index 95%\n\
                     rename from old_name.rs\n\
                     rename to new_name.rs\n\
                     --- a/old_name.rs\n\
                     +++ b/new_name.rs\n\
                     @@ -1,3 +1,3 @@\n\
                     -old line\n\
                     +new line";
        let result = GitService::summarize_diff(diff);
        // テキストファイルのリネームはそのまま保持
        assert!(result.contains("rename from old_name.rs"));
        assert!(result.contains("rename to new_name.rs"));
        assert!(result.contains("-old line"));
        assert!(result.contains("+new line"));
    }

    #[test]
    fn test_filter_binary_diff_deleted_binary_file() {
        // 削除されたバイナリファイルはサマリーのみ出力
        let diff = "diff --git a/image.png b/image.png\n\
                     deleted file mode 100644\n\
                     Binary files a/image.png and /dev/null differ";
        let result = GitService::summarize_diff(diff);
        assert_eq!(result, "[Binary] deleted: image.png");
    }

    // ============================================================
    // truncate_diff: 境界値テスト（MAX_DIFF_CHARS + 1）
    // ============================================================

    #[test]
    fn test_truncate_diff_one_char_over_limit() {
        // MAX_DIFF_CHARS + 1 文字の入力で切り詰めが発生する
        let line = "a".repeat(5000);
        let diff = format!("{}\n{}\nx", line, line); // 10002文字（改行含む）
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        assert!(!result.contains("\nx")); // 最後の行は含まれない
    }

    // ============================================================
    // extract_file_path_from_diff_header: 特殊パス
    // ============================================================

    #[test]
    fn test_extract_file_path_with_spaces_unquoted() {
        // スペースを含む非クォートの対称パスを中央分割で正しく抽出できること
        let header = "diff --git a/path with spaces/file.rs b/path with spaces/file.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path with spaces/file.rs".to_string()));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_spaces_unquoted_symmetric() {
        // スペース含み非クォート対称パスで両側とも正しく抽出できること
        let header = "diff --git a/foo bar.txt b/foo bar.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("foo bar.txt".to_string(), "foo bar.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_quoted_before_unquoted_after_with_space() {
        // 先頭がクォート、後半が非クォートでスペースを含むケース
        // Git は非ASCIIパスはクォートするが、英字＋スペースはクォートしないため
        // リネームで非対称になるこのパターンは実運用で発生しうる
        let header = r#"diff --git "a/\346\227\247.txt" b/new name.txt"#;
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("旧.txt".to_string(), "new name.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_asymmetric_spaces() {
        // rename で前後のパスが異なり、かつ両側にスペースを含むケース
        let header = "diff --git a/old file.txt b/generated/new file.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some((
                "old file.txt".to_string(),
                "generated/new file.txt".to_string()
            ))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_symmetric_non_ascii_unquoted() {
        // core.quotePath=false 相当で非ASCIIが非クォートで出力されるケース
        let header = "diff --git a/あ space.txt b/あ space.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("あ space.txt".to_string(), "あ space.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_symmetric_minimal() {
        // `a/x b/x` 最短対称ケース
        let header = "diff --git a/x b/x";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("x".to_string(), "x".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_rename_no_space() {
        // スペースなしのリネーム（両側非クォート非対称）は空白分割経路で処理できる
        let header = "diff --git a/old.txt b/new.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("old.txt".to_string(), "new.txt".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_unquoted_space_before_quoted_after() {
        // 先頭が非クォート(スペース含み)、後半がクォートのケース。
        // Git は a/b 両側を独立にクォートするため、スペースのみの旧パスは非クォート、
        // 非ASCIIの新パスはクォートという混在形が実運用で発生する
        let header = r#"diff --git a/old name.txt "b/new\303\251.txt""#;
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("old name.txt".to_string(), "newé.txt".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_unquoted_no_space_before_quoted_after() {
        // 先頭が非クォート(スペースなし)、後半がクォートの既存動作の回帰確認
        let header = r#"diff --git a/old.txt "b/new\303\251.txt""#;
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("old.txt".to_string(), "newé.txt".to_string()))
        );
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_asymmetric() {
        // 非対称パスは対称分割では抽出しない（呼び出し側のフォールバックに委ねる）
        // "a/old.txt b/new.txt" は奇数長だが a/ 側と b/ 側のパスが一致しない
        assert!(GitService::try_split_symmetric_unquoted_header("a/old.txt b/new.txt").is_none());
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_even_length() {
        // 奇数長でない文字列は対称ケースではない
        assert!(GitService::try_split_symmetric_unquoted_header("a/foo b/fo").is_none());
    }

    #[test]
    fn test_try_split_symmetric_unquoted_header_rejects_without_middle_space() {
        // 中央がスペースでない場合は対称ケースではない
        assert!(GitService::try_split_symmetric_unquoted_header("a/foooob/foooo").is_none());
    }

    #[test]
    fn test_extract_file_path_single_char_filename() {
        // 1文字のファイル名
        let header = "diff --git a/x b/x";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("x".to_string()));
    }

    // ============================================================
    // filter_ignored_files: 複数パターンマッチテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_multiple_patterns_mixed() {
        // 複数のignoreパターンで一部のみマッチする場合
        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new\n\
                     diff --git a/dist/bundle.js b/dist/bundle.js\n\
                     --- a/dist/bundle.js\n\
                     +++ b/dist/bundle.js\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new\n\
                     diff --git a/src/lib.rs b/src/lib.rs\n\
                     --- a/src/lib.rs\n\
                     +++ b/src/lib.rs\n\
                     @@ -1,1 +1,1 @@\n\
                     -old\n\
                     +new";
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "dist/").unwrap();
        let ignore = builder.build().unwrap();

        let result = GitService::filter_ignored_files(diff, &ignore);
        // src/main.rs と src/lib.rs は本文も保持、dist/bundle.js はパスだけ残す。
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(result.contains(&format!(
            "diff --git a/dist/bundle.js b/dist/bundle.js\n{IGNORED_CONTENT_NOTICE}"
        )));
        assert!(!result.contains("--- a/dist/bundle.js"));
    }

    // ============================================================
    // decode_quoted_diff_path: 8進オーバーフロー防御テスト
    // ============================================================

    #[test]
    fn test_decode_quoted_path_octal_overflow_returns_none() {
        // \400 (=256) はu8範囲外なのでNoneを返す
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\400""#), None);
        // \777 (=511) も同様にNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\777""#), None);
    }

    #[test]
    fn test_decode_quoted_path_octal_boundary_377_is_valid() {
        // \377 (=255) はu8の最大値で有効
        let result = GitService::decode_quoted_diff_path(r#"a/\377""#);
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_quoted_path_octal_boundary_400_is_invalid() {
        // \400 (=256) はちょうどオーバーフロー境界
        assert_eq!(GitService::decode_quoted_diff_path(r#"\400""#), None);
    }

    #[test]
    fn test_decode_quoted_path_octal_overflow_mid_path() {
        // パス途中のオーバーフローもNoneを返す
        assert_eq!(
            GitService::decode_quoted_diff_path(r#"a/valid\400rest""#),
            None
        );
    }

    // ============================================================
    // decode_quoted_diff_path: 連続8進シーケンス・特殊エスケープ
    // ============================================================

    #[test]
    fn test_decode_quoted_path_consecutive_octal_sequences() {
        // 連続する8進シーケンス: \343\203\206 = "テ" (UTF-8: E3 83 86)
        let result =
            GitService::decode_quoted_diff_path(r#"a/\343\203\206\343\202\271\343\203\210""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/テスト");
    }

    #[test]
    fn test_decode_quoted_path_tab_escape_in_middle() {
        // \t エスケープがタブに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\twith\ttab""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\twith\ttab");
    }

    #[test]
    fn test_decode_quoted_path_newline_escape_in_middle() {
        // \n エスケープが改行に変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\nwith\nnewline""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\nwith\nnewline");
    }

    #[test]
    fn test_decode_quoted_path_double_backslash_in_middle() {
        // \\ エスケープがバックスラッシュに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\\with\\backslash""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\\with\\backslash");
    }

    #[test]
    fn test_decode_quoted_path_escaped_double_quote_in_middle() {
        // \" エスケープがダブルクォートに変換される
        let result = GitService::decode_quoted_diff_path(r#"a/path\"with\"quote""#);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "a/path\"with\"quote");
    }

    #[test]
    fn test_decode_quoted_path_missing_closing_quote() {
        // 閉じクォートがない場合はNone
        assert_eq!(
            GitService::decode_quoted_diff_path(r#"a/path/without/close"#),
            None
        );
    }

    #[test]
    fn test_decode_quoted_path_immediately_closed_quotes() {
        // クォート内が空の場合はNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"""#), None);
    }

    #[test]
    fn test_decode_quoted_path_one_octal_digit_only() {
        // 8進が1桁の場合（3桁未満）: 次の2バイトが取れないのでNone
        assert_eq!(GitService::decode_quoted_diff_path(r#"a/\3""#), None);
    }

    // ============================================================
    // truncate_diff: マルチバイト文字境界のテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_exact_limit() {
        // ちょうどMAX_DIFF_CHARS文字の場合はそのまま
        let diff: String = "a".repeat(MAX_DIFF_CHARS);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
    }

    #[test]
    fn test_truncate_diff_one_over_limit() {
        // MAX_DIFF_CHARS + 1文字の場合は切り詰められる
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 1);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        // 元のdiffより切り詰めメッセージ分長くなるが、オリジナルの全文字は含まれない
        assert!(!result.contains(&diff));
    }

    #[test]
    fn test_truncate_diff_multibyte_with_newlines() {
        // 日本語 + 改行の混合で、最後の完全な行まで切り詰められる
        let line = "あいうえお\n"; // 6文字（5文字 + 改行）
        let count = MAX_DIFF_CHARS / 6 + 2; // 制限を超える分
        let diff: String = line.repeat(count);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
        // 切り詰め後も改行で終わる行構造が維持される
        let before_truncation_msg = result.split("\n\n...").next().unwrap();
        assert!(before_truncation_msg.ends_with("あいうえお"));
    }

    #[test]
    fn test_truncate_diff_no_newline_in_content() {
        // 改行がない長い文字列の場合、rfind('\n')がNoneでもパニックしない
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("diff truncated"));
    }

    // ============================================================
    // summarize_diff: リネーム・モード変更のエッジケース
    // ============================================================

    #[test]
    fn test_filter_binary_diff_renamed_binary_with_similarity() {
        // similarity index付きバイナリリネーム
        let diff = r#"diff --git a/old.png b/new.png
similarity index 100%
rename from old.png
rename to new.png
Binary files a/old.png and b/new.png differ"#;
        let result = GitService::summarize_diff(diff);
        assert_eq!(result, "[Binary] renamed: old.png -> new.png");
    }

    #[test]
    fn test_filter_binary_diff_executable_mode() {
        // 実行可能モード (100755) のバイナリ
        let diff = r#"diff --git a/script.bin b/script.bin
new file mode 100755
Binary files /dev/null and b/script.bin differ"#;
        let result = GitService::summarize_diff(diff);
        assert_eq!(result, "[Binary] added: script.bin");
    }

    // ============================================================
    // extract_file_path_from_diff_header: 特殊ファイル名
    // ============================================================

    #[test]
    fn test_extract_file_path_spaces_in_name() {
        // クォートされたスペース付きファイル名
        let header = r#"diff --git "a/path with spaces/file.rs" "b/path with spaces/file.rs""#;
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("path with spaces/file.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_basic_unquoted() {
        // 通常のファイル名
        let header = "diff --git a/src/main.rs b/src/main.rs";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("src/main.rs".to_string()));
    }

    #[test]
    fn test_extract_file_path_deeply_nested() {
        // 深いネストのパス
        let header = "diff --git a/a/b/c/d/e/f/g.txt b/a/b/c/d/e/f/g.txt";
        let result = GitService::extract_file_path_from_diff_header(header);
        assert_eq!(result, Some("a/b/c/d/e/f/g.txt".to_string()));
    }

    #[test]
    fn test_extract_file_path_empty_header() {
        // 不正なヘッダー
        let result = GitService::extract_file_path_from_diff_header("diff --git ");
        assert!(result.is_none());
    }

    // ============================================================
    // decode_quoted_diff_path: \r エスケープのテスト
    // ============================================================

    #[test]
    fn test_decode_quoted_diff_path_carriage_return_escape() {
        // \r エスケープシーケンスが正しくデコードされる
        let result = GitService::decode_quoted_diff_path("a/file\\rwith\\rcr\" rest");
        assert_eq!(result, Some("a/file\rwith\rcr".to_string()));
    }

    #[test]
    fn test_decode_quoted_diff_path_unknown_escape() {
        // 未知のエスケープ文字はそのまま出力される
        let result = GitService::decode_quoted_diff_path("a/file\\xname\" rest");
        assert_eq!(result, Some("a/filexname".to_string()));
    }

    #[test]
    fn test_decode_quoted_diff_path_octal_max_valid_377() {
        // 8進 \377 = 255 (u8の最大値) は有効
        let result = GitService::decode_quoted_diff_path("a/\\377\" rest");
        assert!(result.is_some());
    }

    // ============================================================
    // has_staged_changes のテスト（実際のgitリポジトリ内）
    // ============================================================

    #[test]
    fn test_has_staged_changes_in_clean_repo() {
        // クリーンなリポジトリでは false を返す
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);
        // 初期コミットを作成
        std::fs::write(repo.join("file.txt"), "content").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "init"]);

        let git = GitService {
            repo_path: temp.path().to_path_buf(),
        };
        assert!(!git.has_staged_changes());
    }

    #[test]
    fn test_has_staged_changes_with_staged_file() {
        let temp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::fs::write(temp.path().join("file.txt"), "content").unwrap();
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(temp.path())
            .output()
            .unwrap();

        let git = GitService {
            repo_path: temp.path().to_path_buf(),
        };
        assert!(git.has_staged_changes());
    }

    #[test]
    fn test_has_staged_changes_with_untracked_file_is_false() {
        let temp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        std::fs::write(temp.path().join("untracked.txt"), "content").unwrap();

        let git = GitService {
            repo_path: temp.path().to_path_buf(),
        };
        assert!(!git.has_staged_changes());
    }

    // ============================================================
    // truncate_diff: 境界値テスト
    // ============================================================

    #[test]
    fn test_truncate_diff_exact_boundary() {
        // ちょうど MAX_DIFF_CHARS の場合は切り詰めない
        let diff: String = "a".repeat(MAX_DIFF_CHARS);
        let result = GitService::truncate_diff(&diff);
        assert_eq!(result.len(), MAX_DIFF_CHARS);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn test_truncate_diff_one_over_boundary() {
        // MAX_DIFF_CHARS + 1 の場合は切り詰める
        let diff: String = "a".repeat(MAX_DIFF_CHARS + 1);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_truncate_diff_no_newline_falls_to_else_branch() {
        // 改行がない長大な文字列の切り詰め（else 分岐）
        let diff: String = "x".repeat(MAX_DIFF_CHARS + 100);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
        // 改行がないため rfind('\n') は None → そのまま切り詰め
        assert!(result.starts_with("xxxx"));
    }

    #[test]
    fn test_truncate_diff_multibyte_boundary() {
        // マルチバイト文字が境界付近にある場合
        // "あ" は3バイトだが1文字 → chars().take() は文字単位でカット
        let prefix: String = "a".repeat(MAX_DIFF_CHARS - 2);
        let diff = format!("{}\nあいう", prefix);
        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
        // 文字単位でカットされ、バイト列の破損はない
        assert!(result.is_char_boundary(0));
    }

    #[test]
    fn test_truncate_diff_multibyte_byte_over_but_char_within_limit() {
        // バイト長は MAX_DIFF_CHARS を超えるが文字数では収まる場合は切り詰めない
        // "あ" は3バイト/1文字なので、3334個で 10002バイト > MAX_DIFF_CHARS、文字数は3334 ≦ MAX_DIFF_CHARS
        let diff: String = "あ".repeat(3334);
        assert!(diff.len() > MAX_DIFF_CHARS);
        assert!(diff.chars().count() <= MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
        assert!(!result.contains("truncated"));
    }

    #[test]
    fn test_truncate_diff_multibyte_byte_over_and_char_over_limit() {
        // バイト長・文字数とも MAX_DIFF_CHARS を超える場合は切り詰める
        // "あ" 10001 個 = 30003バイト・10001文字
        let diff: String = "あ".repeat(MAX_DIFF_CHARS + 1);
        assert!(diff.len() > MAX_DIFF_CHARS);
        assert!(diff.chars().count() > MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        assert!(result.contains("truncated"));
        // 文字境界を破損していない
        assert!(result.is_char_boundary(0));
        for (idx, _) in result.char_indices() {
            assert!(result.is_char_boundary(idx));
        }
    }

    // ============================================================
    // summarize_diff: 追加エッジケース
    // ============================================================

    #[test]
    fn test_filter_binary_diff_consecutive_binary_add_and_delete() {
        // 追加と削除のバイナリファイルが連続する場合
        let diff = "diff --git a/a.png b/a.png\n\
                     new file mode 100644\n\
                     Binary files /dev/null and b/a.png differ\n\
                     diff --git a/b.jpg b/b.jpg\n\
                     deleted file mode 100644\n\
                     Binary files a/b.jpg and /dev/null differ";
        let result = GitService::summarize_diff(diff);
        assert!(result.contains("[Binary] added: a.png"));
        assert!(result.contains("[Binary] deleted: b.jpg"));
    }

    #[test]
    fn test_filter_binary_diff_binary_then_text() {
        // バイナリファイルの直後にテキストファイルが続く場合
        let diff = "diff --git a/icon.png b/icon.png\n\
                     Binary files a/icon.png and b/icon.png differ\n\
                     diff --git a/main.rs b/main.rs\n\
                     --- a/main.rs\n\
                     +++ b/main.rs\n\
                     @@ -1 +1 @@\n\
                     -old\n\
                     +new";
        let result = GitService::summarize_diff(diff);
        assert!(result.contains("[Binary] modified: icon.png"));
        assert!(result.contains("+new"));
        assert!(result.contains("-old"));
    }

    // ============================================================
    // filter_ignored_files: ディレクトリパターンのテスト
    // ============================================================

    #[test]
    fn test_filter_ignored_files_directory_glob_pattern() {
        // ディレクトリのグロブパターンでフィルタ
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path();
        std::fs::write(root.join(".gitignore"), "generated/**\n").unwrap();

        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        builder.add(root.join(".gitignore"));
        let ignore = builder.build().unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new\n\
                     diff --git a/generated/out.rs b/generated/out.rs\n\
                     --- a/generated/out.rs\n\
                     +++ b/generated/out.rs\n\
                     -x\n\
                     +y";
        let result = GitService::filter_ignored_files(diff, &ignore);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains(&format!(
            "diff --git a/generated/out.rs b/generated/out.rs\n{IGNORED_CONTENT_NOTICE}"
        )));
        assert!(!result.contains("-x"));
        assert!(!result.contains("+y"));
    }

    // ============================================================
    // verify_commit_hash: 非コミットオブジェクトの拒否テスト
    // ============================================================

    #[test]
    fn test_verify_commit_hash_rejects_tree_object() {
        // treeオブジェクトはコミットではないため拒否されること
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // HEAD^{tree} は有効なオブジェクトだが、コミットではない
        let result = service.verify_commit_hash("HEAD^{tree}");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_commit_hash_accepts_valid_short_hash() {
        // 短縮ハッシュもコミットとして検証される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // git rev-parse --short HEAD で短縮ハッシュを取得
        let short_hash = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let short_hash = String::from_utf8_lossy(&short_hash.stdout)
            .trim()
            .to_string();

        assert!(service.verify_commit_hash(&short_hash).is_ok());
    }

    #[test]
    fn test_verify_commit_hash_rejects_empty_string() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.verify_commit_hash("");
        assert!(result.is_err());
    }

    // ============================================================
    // apply_all_filters: フィルタ合成のテスト
    // ============================================================

    #[test]
    fn test_apply_all_filters_ignore_before_binary() {
        // ignoreフィルタがバイナリフィルタより先に適用されることを検証
        // ignoreパターンにマッチするバイナリファイルはパスだけ残す。
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        std::fs::write(repo.join(".git-sc-ignore"), "*.png\n").unwrap();

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new\n\
                     diff --git a/image.png b/image.png\n\
                     Binary files /dev/null and b/image.png differ";

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(diff).unwrap();

        // テキストファイルは含まれる
        assert!(result.contains("src/main.rs"));
        // バイナリ+ignoreのファイルはパスと省略通知だけ残す。
        assert!(result.contains(&format!(
            "diff --git a/image.png b/image.png\n{IGNORED_CONTENT_NOTICE}"
        )));
        assert!(!result.contains("[Binary]"));
        assert!(!result.contains("Binary files"));
    }

    #[test]
    fn test_apply_all_filters_fails_closed_when_ignore_file_is_unreadable() {
        // 回帰テスト: `.git-sc-ignore` が存在するのに読めないとき、以前は None を返して
        // 「除外設定なし」と同じ扱いにしていた。その結果、除外したかったファイル
        // (認証情報・鍵など)の差分が黙って AI プロバイダーへ送られてしまう。
        // 適用できないと分かった時点でエラーにする(fail-closed)。
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        // `.git-sc-ignore` をディレクトリとして作ると読み取りは必ず失敗する。
        // パーミッション 000 は root 実行だと読めてしまい環境依存になるため使わない。
        std::fs::create_dir(repo.join(".git-sc-ignore")).unwrap();

        let diff = "diff --git a/secret.env b/secret.env\n\
                     --- a/secret.env\n\
                     +++ b/secret.env\n\
                     +TOKEN=abc";

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(diff);
        assert!(
            matches!(result, Err(AppError::ConfigError(_))),
            "読めない .git-sc-ignore は ConfigError にすべき。実際: {result:?}"
        );

        // 差分そのものが返らないこと(= AI へ渡らないこと)が本題
        assert!(
            result.is_err(),
            "除外設定を適用できないまま差分を返してはいけない"
        );
    }

    #[test]
    fn test_apply_all_filters_no_ignore_file() {
        // .git-sc-ignoreが無い場合はフィルタなしで動作
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        let diff = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     -old\n\
                     +new";

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(diff).unwrap();
        assert!(result.contains("src/main.rs"));
    }

    #[test]
    fn test_apply_all_filters_truncation_applied_last() {
        // 文字数制限はフィルタリング後に適用される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();
        run_git_in(repo, &["init"]);

        // MAX_DIFF_CHARS を超える入力を生成
        let mut diff = String::from("diff --git a/big.rs b/big.rs\n--- a/big.rs\n+++ b/big.rs\n");
        for i in 0..2000 {
            diff.push_str(&format!("+line {} with some content\n", i));
        }

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };
        let result = service.apply_all_filters(&diff).unwrap();
        assert!(result.contains("diff truncated"));
    }

    // ============================================================
    // TempRewordMessageFile: 基本動作テスト
    // ============================================================

    #[test]
    fn test_temp_reword_message_file_content() {
        // 書き込んだ内容が正しく保存される
        let msg = "feat: テスト用コミットメッセージ";
        let tmp = TempRewordMessageFile::create(msg).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, msg);
    }

    #[test]
    fn test_temp_reword_message_file_unique_paths() {
        // 複数のファイルがそれぞれ異なるパスを持つ
        let tmp1 = TempRewordMessageFile::create("msg1").unwrap();
        let tmp2 = TempRewordMessageFile::create("msg2").unwrap();
        assert_ne!(tmp1.path(), tmp2.path());
    }

    #[test]
    fn test_temp_reword_message_file_drop_cleanup() {
        // Drop後にファイルが削除される
        let path = {
            let tmp = TempRewordMessageFile::create("temp msg").unwrap();
            let p = tmp.path().to_path_buf();
            assert!(p.exists());
            p
        };
        assert!(!path.exists());
    }

    #[test]
    fn test_temp_reword_message_file_multibyte_content() {
        // マルチバイト文字（日本語）を含むメッセージが正しく保存される
        let msg = "feat: 日本語コミットメッセージ 🎉";
        let tmp = TempRewordMessageFile::create(msg).unwrap();
        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content, msg);
    }

    // ============================================================
    // validate_reword_target_hash: 境界テスト
    // ============================================================

    #[test]
    fn test_validate_reword_target_hash_not_in_history() {
        // 別ブランチのコミットはreword対象として拒否される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        // 初期ブランチにコミット
        std::fs::write(repo.join("file.txt"), "v1\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "first"]);
        let primary_branch = git_output_in(repo, &["branch", "--show-current"]);

        // 別ブランチを作成してコミット
        run_git_in(repo, &["checkout", "-b", "other"]);
        std::fs::write(repo.join("other.txt"), "other\n").unwrap();
        run_git_in(repo, &["add", "other.txt"]);
        run_git_in(repo, &["commit", "-m", "other commit"]);

        let other_hash = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let other_hash = String::from_utf8_lossy(&other_hash.stdout)
            .trim()
            .to_string();

        // 初期ブランチに戻る
        run_git_in(repo, &["checkout", &primary_branch]);

        // 初期ブランチに2つ目のコミットを追加（otherとは別の履歴）
        std::fs::write(repo.join("file2.txt"), "v2\n").unwrap();
        run_git_in(repo, &["add", "file2.txt"]);
        run_git_in(repo, &["commit", "-m", "second on primary"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        // otherブランチのコミットは初期ブランチのHEAD履歴外
        // ただしfirst commitが共通祖先であり、otherのコミットはfirstの子
        // 初期ブランチ側にも子があるのでotherは祖先ではない
        let result = service.validate_reword_target_hash(&other_hash);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_merge_commits_in_range_by_hash_detects_target_merge_commit() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        run_git_in(repo, &["add", "base.txt"]);
        run_git_in(repo, &["commit", "-m", "base commit"]);

        let branch_output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(repo)
            .output()
            .unwrap();
        let base_branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();

        run_git_in(repo, &["checkout", "-b", "feature/reword-merge"]);
        std::fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        run_git_in(repo, &["add", "feature.txt"]);
        run_git_in(repo, &["commit", "-m", "feature commit"]);

        run_git_in(repo, &["checkout", &base_branch]);
        std::fs::write(repo.join("main.txt"), "main\n").unwrap();
        run_git_in(repo, &["add", "main.txt"]);
        run_git_in(repo, &["commit", "-m", "main commit"]);
        run_git_in(
            repo,
            &[
                "merge",
                "--no-ff",
                "feature/reword-merge",
                "-m",
                "merge feature",
            ],
        );

        let merge_hash_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        let merge_hash = String::from_utf8_lossy(&merge_hash_output.stdout)
            .trim()
            .to_string();

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        assert!(
            service
                .has_merge_commits_in_range_by_hash(&merge_hash)
                .unwrap()
        );
    }

    /// マージを含む履歴を構築し、(一時ディレクトリ, C0=最古, C1=マージ下の first-parent 祖先) を返す。
    ///
    /// first-parent 鎖: T2 → T1 → MERGE → C1 → C0
    /// feature ブランチ(F1..F5)を C1 から分岐して main に `--no-ff` マージするため、
    /// `C1..HEAD` のトポロジカルなコミット数(=8)は first-parent 深さ(=3)より大きくなる。
    /// これにより「位置算出をトポロジカル数で行うと n が過大になり HEAD~n が無効化する」
    /// という不具合を再現できる。
    fn setup_merge_history_repo() -> (tempfile::TempDir, String, String) {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("c0.txt"), "c0\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "C0"]);
        let c0 = git_output_in(repo, &["rev-parse", "HEAD"]);
        let base_branch = git_output_in(repo, &["branch", "--show-current"]);

        std::fs::write(repo.join("c1.txt"), "c1\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "C1"]);
        let c1 = git_output_in(repo, &["rev-parse", "HEAD"]);

        // C1 から feature ブランチを分岐し、側コミットを5つ積む
        run_git_in(repo, &["checkout", "-b", "feature/fp"]);
        for i in 1..=5 {
            let name = format!("f{i}.txt");
            std::fs::write(repo.join(&name), format!("f{i}\n")).unwrap();
            run_git_in(repo, &["add", "."]);
            run_git_in(repo, &["commit", "-m", &format!("F{i}")]);
        }

        // main(=base_branch, C1 のまま) に戻り、--no-ff で first-parent 上に MERGE を作る
        run_git_in(repo, &["checkout", &base_branch]);
        run_git_in(repo, &["merge", "--no-ff", "feature/fp", "-m", "MERGE"]);

        // マージの上に線形コミットを2つ積む
        std::fs::write(repo.join("t1.txt"), "t1\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "T1"]);
        std::fs::write(repo.join("t2.txt"), "t2\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "T2"]);

        (temp_dir, c0, c1)
    }

    // ============================================================
    // write_tree: index スナップショットの比較
    // ============================================================

    #[test]
    fn test_write_tree_detects_index_change_that_has_staged_changes_misses() {
        // 回帰テスト: コミット直前の再確認が `has_staged_changes()` の有無判定だけだと、
        // AI 生成中に別プロセスが `git add` してもすり抜け、確認していない変更まで
        // コミットに入ってしまう。tree ハッシュなら内容の変化を検出できる。
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        run_git_in(repo, &["add", "."]);

        let service = GitService::with_repo_path(repo.to_path_buf());
        let before = service.write_tree().unwrap();

        // 検査自体は index を変えないので、二度読んでも同じハッシュになる
        assert_eq!(
            service.write_tree().unwrap(),
            before,
            "write_tree は index を変更してはいけない"
        );

        // 別プロセスによる追加ステージを再現する
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        run_git_in(repo, &["add", "."]);

        assert!(
            service.has_staged_changes(),
            "有無判定は変化の前後どちらでも true なので、これだけでは検出できない"
        );
        assert_ne!(
            service.write_tree().unwrap(),
            before,
            "ステージ内容が変わったら tree ハッシュも変わるべき"
        );
    }

    #[test]
    fn test_reword_by_hash_below_merge_returns_has_merge_commits_not_cryptic_error() {
        // 回帰テスト: マージより下の first-parent 祖先 C1 を reword しようとすると、
        // 位置算出をトポロジカル数で行っていた頃は n が過大になり `HEAD~n` が無効化して
        // 不可解な git エラー（fatal: ambiguous argument 'HEAD~n..HEAD'）が露出していた。
        // first-parent で数えることで、範囲内の MERGE を正しく検出し、明確な
        // HasMergeCommits（「マージ越し reword は不可」）を返す。
        let (temp_dir, _c0, c1) = setup_merge_history_repo();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        let result = service.reword_commit_by_hash(&c1, "新しいメッセージ");
        assert!(
            matches!(result, Err(AppError::HasMergeCommits)),
            "C1 と HEAD の間に MERGE があるので HasMergeCommits を返すべき。実際: {result:?}"
        );
    }

    #[test]
    fn test_reword_by_hash_root_below_merge_returns_has_merge_commits() {
        // 最古コミット C0 を対象にしたとき、first-parent 深さ(=5)と位置 n が一致するため
        // 「最古コミット」センチネルに入り、履歴全体のマージ(MERGE)を検出して
        // HasMergeCommits を返す（トポロジカル総数で判定していると n!=総数となり、
        // ここでも HEAD~n 無効化の不可解エラーになっていた）。
        let (temp_dir, c0, _c1) = setup_merge_history_repo();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        let result = service.reword_commit_by_hash(&c0, "新しいメッセージ");
        assert!(
            matches!(result, Err(AppError::HasMergeCommits)),
            "履歴全体に MERGE があるので HasMergeCommits を返すべき。実際: {result:?}"
        );
    }

    #[test]
    fn test_get_commit_position_by_hash_counts_first_parent_depth() {
        // 位置はトポロジカル数ではなく first-parent 深さで算出される。
        // first-parent 鎖: T2(HEAD~0) T1(HEAD~1) MERGE(HEAD~2) C1(HEAD~3) C0(HEAD~4)
        // 位置 n は「対象の親が HEAD~n」になる値なので、C1 は 4、最古の C0 は 5。
        // 旧実装（トポロジカル数）では C1 が 9 を返し、後段の HEAD~9 で破綻していた。
        let (temp_dir, c0, c1) = setup_merge_history_repo();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        assert_eq!(service.get_commit_position_by_hash(&c1).unwrap(), 4);
        assert_eq!(service.get_commit_position_by_hash(&c0).unwrap(), 5);
    }

    // ============================================================
    // rebase 進行中の検出（reword が他人の rebase を壊さないこと）
    // ============================================================

    /// コンフリクトで停止した rebase を持つリポジトリを作る。
    /// 返り値: (temp_dir, reword 対象にできるコミットハッシュ)
    fn setup_repo_with_interrupted_rebase() -> (tempfile::TempDir, String) {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();

        std::fs::write(repo.join("f.txt"), "base\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "base"]);
        let base_branch = git_output_in(repo, &["branch", "--show-current"]);

        // side ブランチで同じファイルを書き換える
        run_git_in(repo, &["checkout", "-b", "side"]);
        std::fs::write(repo.join("f.txt"), "side\n").unwrap();
        run_git_in(repo, &["commit", "-am", "side change"]);

        // 元ブランチでも同じ行を書き換えて、rebase 時に必ずコンフリクトさせる
        run_git_in(repo, &["checkout", &base_branch]);
        std::fs::write(repo.join("f.txt"), "main\n").unwrap();
        run_git_in(repo, &["commit", "-am", "main change"]);
        std::fs::write(repo.join("g.txt"), "g\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "second main change"]);
        let target = git_output_in(repo, &["rev-parse", "HEAD~1"]);

        // ユーザーが rebase を始めてコンフリクトで停止した状態を作る。
        // 失敗が期待値なので run_git_in（成功を assert する）は使えない。
        let output = Command::new("git")
            .args(["rebase", "side"])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "コンフリクトで停止させたいので rebase は失敗するはず"
        );

        (temp_dir, target)
    }

    #[test]
    fn test_rebase_in_progress_detects_interrupted_rebase() {
        let (temp_dir, _target) = setup_repo_with_interrupted_rebase();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        assert!(
            service.rebase_in_progress(),
            "コンフリクトで停止した rebase を検出できるべき"
        );
    }

    #[test]
    fn test_rebase_in_progress_false_on_clean_repo() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("f.txt"), "a\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(repo, &["commit", "-m", "c1"]);

        let service = GitService::with_repo_path(repo.to_path_buf());
        assert!(
            !service.rebase_in_progress(),
            "rebase していないリポジトリで true になってはいけない"
        );
    }

    #[test]
    fn test_reword_refuses_when_another_rebase_is_in_progress() {
        // 回帰テスト: reword は rebase 失敗時に必ず `git rebase --abort` を実行する。
        // 実行前から別の rebase が進行中だと `git rebase -i` の起動自体が失敗するため、
        // その失敗を自分の rebase の失敗と取り違えて abort すると、ユーザーが解決中だった
        // rebase（コンフリクトの途中経過を含む）を破棄してしまう。開始前に弾くこと。
        let (temp_dir, target) = setup_repo_with_interrupted_rebase();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        let result = service.reword_commit(2, "新しいメッセージ");
        assert!(
            matches!(result, Err(AppError::RebaseInProgress)),
            "rebase 進行中は RebaseInProgress で止まるべき。実際: {result:?}"
        );

        // ユーザーの rebase が生き残っていること（abort されていないこと）が本題
        assert!(
            service.rebase_in_progress(),
            "git-sc がユーザーの進行中 rebase を破棄してはいけない"
        );

        // ハッシュ指定の入口からも同じ保証が効く
        let by_hash = service.reword_commit_by_hash(&target, "新しいメッセージ");
        assert!(
            matches!(by_hash, Err(AppError::RebaseInProgress)),
            "reword_commit_by_hash でも RebaseInProgress で止まるべき。実際: {by_hash:?}"
        );
        assert!(
            service.rebase_in_progress(),
            "ハッシュ指定経路でもユーザーの rebase を破棄してはいけない"
        );
    }

    #[test]
    fn test_reword_head_refuses_when_another_rebase_is_in_progress() {
        // n == 1 は amend 経路に分岐するが、rebase 中の HEAD は detached で
        // 意図しないコミットを指すため、こちらも開始前に弾く。
        let (temp_dir, _target) = setup_repo_with_interrupted_rebase();
        let service = GitService::with_repo_path(temp_dir.path().to_path_buf());

        let result = service.reword_commit(1, "新しいメッセージ");
        assert!(
            matches!(result, Err(AppError::RebaseInProgress)),
            "amend 経路でも RebaseInProgress で止まるべき。実際: {result:?}"
        );
        assert!(
            service.rebase_in_progress(),
            "amend 経路でもユーザーの rebase を破棄してはいけない"
        );
    }

    // ============================================================
    // get_commit_diff_by_hash: 非コミットオブジェクト拒否テスト
    // ============================================================

    #[test]
    fn test_get_commit_diff_by_hash_rejects_tree() {
        // treeオブジェクトはdiff取得で拒否される
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let repo = temp_dir.path();

        run_git_in(repo, &["init"]);
        run_git_in(repo, &["config", "user.name", "Test User"]);
        run_git_in(repo, &["config", "user.email", "test@example.com"]);

        std::fs::write(repo.join("file.txt"), "content\n").unwrap();
        run_git_in(repo, &["add", "file.txt"]);
        run_git_in(repo, &["commit", "-m", "test"]);

        let service = GitService {
            repo_path: repo.to_path_buf(),
        };

        let result = service.get_commit_diff_by_hash("HEAD^{tree}");
        assert!(result.is_err());
    }

    // ============================================================
    // extract_file_paths_from_diff_header: ユニットテスト
    // ============================================================

    #[test]
    fn test_extract_file_paths_from_diff_header_standard() {
        let result = GitService::extract_file_paths_from_diff_header(
            "diff --git a/src/main.rs b/src/main.rs",
        );
        assert_eq!(
            result,
            Some(("src/main.rs".to_string(), "src/main.rs".to_string()))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_rename() {
        let result =
            GitService::extract_file_paths_from_diff_header("diff --git a/old.rs b/new.rs");
        assert_eq!(result, Some(("old.rs".to_string(), "new.rs".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_nested_path() {
        let result = GitService::extract_file_paths_from_diff_header(
            "diff --git a/src/deep/nested/file.rs b/src/deep/nested/file.rs",
        );
        assert_eq!(
            result,
            Some((
                "src/deep/nested/file.rs".to_string(),
                "src/deep/nested/file.rs".to_string()
            ))
        );
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_no_a_prefix() {
        // a/ プレフィックスがない場合は None
        let result =
            GitService::extract_file_paths_from_diff_header("diff --git src/main.rs src/main.rs");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_empty() {
        let result = GitService::extract_file_paths_from_diff_header("diff --git ");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_not_diff() {
        let result = GitService::extract_file_paths_from_diff_header("not a diff header");
        assert!(result.is_none());
    }

    // ============================================================
    // take_diff_header_path: ユニットテスト
    // ============================================================

    #[test]
    fn test_take_diff_header_path_unquoted() {
        let (path, rest) = GitService::take_diff_header_path("a/file.rs b/file.rs").unwrap();
        assert_eq!(path, "a/file.rs");
        assert_eq!(rest, " b/file.rs");
    }

    #[test]
    fn test_take_diff_header_path_quoted_utf8() {
        // クオート付きUTF-8パス（日本語ファイル名）
        let (path, _) = GitService::take_diff_header_path(
            "\"a/\\343\\203\\206\\343\\202\\271\\343\\203\\210.rs\" b/test.rs",
        )
        .unwrap();
        assert_eq!(path, "a/テスト.rs");
    }

    #[test]
    fn test_take_diff_header_path_empty_input() {
        assert!(GitService::take_diff_header_path("").is_none());
    }

    #[test]
    fn test_take_diff_header_path_whitespace_only() {
        assert!(GitService::take_diff_header_path("   ").is_none());
    }

    // ============================================================
    // decode_quoted_diff_path: 追加エッジケース
    // ============================================================

    #[test]
    fn test_decode_quoted_diff_path_escaped_backslash() {
        assert_eq!(
            GitService::decode_quoted_diff_path("path\\\\file\""),
            Some("path\\file".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_tab_escape() {
        assert_eq!(
            GitService::decode_quoted_diff_path("tab\\there\""),
            Some("tab\there".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_newline_escape() {
        assert_eq!(
            GitService::decode_quoted_diff_path("new\\nline\""),
            Some("new\nline".to_string())
        );
    }

    #[test]
    fn test_decode_quoted_diff_path_unclosed_quote() {
        // 閉じ引用符がない場合は None
        assert!(GitService::decode_quoted_diff_path("no closing quote").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_empty_content() {
        // 空のクオート内容は None（直後に閉じ引用符）
        assert!(GitService::decode_quoted_diff_path("\"").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_partial_octal() {
        // 不完全な8進シーケンス（2桁のみ）は None
        assert!(GitService::decode_quoted_diff_path("\\34\"").is_none());
    }

    #[test]
    fn test_decode_quoted_diff_path_escaped_double_quote() {
        assert_eq!(
            GitService::decode_quoted_diff_path("a\\\"b\""),
            Some("a\"b".to_string())
        );
    }

    // ============================================================
    // extract_file_paths_from_diff_header: 追加エッジケース
    // ============================================================

    #[test]
    fn test_extract_file_paths_from_diff_header_asymmetric_multiple_b_separators() {
        // パスに ` b/` が含まれる asymmetric rename は最後の ` b/` で分割される。
        // 例: 旧パスが "foo b/bar"、新パスが "baz" の rename 表示。
        let header = "diff --git a/foo b/bar b/baz";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("foo b/bar".to_string(), "baz".to_string())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_asymmetric_empty_after_falls_back() {
        // asymmetric では `a/PATH b/` の空 after を弾くが、その後の通常パスとして
        // 受理されるため、結果として after_path が空文字列となる現状の挙動を保証する。
        let header = "diff --git a/old.txt b/";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(result, Some(("old.txt".to_string(), String::new())));
    }

    #[test]
    fn test_extract_file_paths_from_diff_header_quoted_after_with_spaces() {
        // 前側がクォートされており後側がスペースを含む非クォートの rename ヘッダー。
        let header = "diff --git \"a/old name.txt\" b/new dir/name.txt";
        let result = GitService::extract_file_paths_from_diff_header(header);
        assert_eq!(
            result,
            Some(("old name.txt".to_string(), "new dir/name.txt".to_string()))
        );
    }

    // ============================================================
    // truncate_diff: 早期 return 経路 (バイト長 == MAX_DIFF_CHARS) のテスト
    // ============================================================

    #[test]
    fn test_truncate_diff_byte_len_equals_limit_returns_original() {
        // バイト長がちょうど MAX_DIFF_CHARS の場合は早期 return 経路で全文を返す。
        let diff: String = "a".repeat(MAX_DIFF_CHARS);
        assert_eq!(diff.len(), MAX_DIFF_CHARS);

        let result = GitService::truncate_diff(&diff);
        assert_eq!(result, diff);
        assert!(!result.contains("truncated"));
    }

    // ============================================================
    // rename / copy の移動先パスに ` b/` を含んでも除外が外れない
    // ============================================================

    #[rstest]
    #[case("similarity index 80%\nrename from notes.txt\nrename to config/plan b/notes.txt")]
    #[case("similarity index 80%\ncopy from notes.txt\ncopy to config/plan b/notes.txt")]
    fn test_filter_ignored_files_checks_rename_metadata_when_path_contains_b_separator(
        #[case] metadata: &str,
    ) {
        // 非クォートのヘッダーを最後の ` b/` で切ると `notes.txt b/config/plan` と
        // `notes.txt` になり、どちらも実在しないパスなので `/config/` に一致しない。
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "/config/").unwrap();
        let ignore = builder.build().unwrap();
        let header = "diff --git a/notes.txt b/config/plan b/notes.txt";
        let diff = format!(
            "{header}\n{metadata}\nindex 1111111..2222222 100644\n--- a/notes.txt\n+++ b/config/plan b/notes.txt\t\n@@ -4,0 +5 @@\n+SECRET-LINE"
        );

        assert_eq!(
            GitService::filter_ignored_files(&diff, &ignore),
            format!("{header}\n{IGNORED_CONTENT_NOTICE}")
        );
    }

    #[test]
    fn test_rename_into_ignored_dir_with_b_separator_omits_contents() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        run_git_in(repo, &["config", "diff.renames", "true"]);
        std::fs::write(repo.join("notes.txt"), "line 1\nline 2\nline 3\nline 4\n").unwrap();
        run_git_in(repo, &["add", "notes.txt"]);
        run_git_in(repo, &["commit", "-m", "add notes"]);

        std::fs::create_dir_all(repo.join("config/plan b")).unwrap();
        run_git_in(repo, &["mv", "notes.txt", "config/plan b/notes.txt"]);
        std::fs::write(
            repo.join("config/plan b/notes.txt"),
            "line 1\nline 2\nline 3\nline 4\nSECRET-LINE\n",
        )
        .unwrap();
        run_git_in(repo, &["add", "config/plan b/notes.txt"]);
        std::fs::write(repo.join(".git-sc-ignore"), "/config/\n").unwrap();
        let service = GitService::with_repo_path(repo.to_path_buf());

        let staged = service.get_staged_diff().unwrap();
        assert!(
            staged.contains("diff --git a/notes.txt b/config/plan b/notes.txt\n"),
            "rename として出力される前提が崩れている: {staged}"
        );
        assert!(!staged.contains("SECRET-LINE"), "{staged}");
        assert!(staged.contains(IGNORED_CONTENT_NOTICE), "{staged}");

        run_git_in(repo, &["commit", "-m", "move notes"]);
        let committed = service.get_last_commit_diff().unwrap();
        assert!(!committed.contains("SECRET-LINE"), "{committed}");
        assert!(committed.contains(IGNORED_CONTENT_NOTICE), "{committed}");
    }

    // ============================================================
    // shallow clone の境界のコミットを書き換えない
    // ============================================================

    /// 3 コミットの履歴を `--depth` 付きで clone する。
    /// ローカルパスの clone では `--depth` が無視されるため `file://` で指定する。
    /// 返り値: (clone 元, clone 先の親ディレクトリ, clone 先のリポジトリ)
    #[cfg(unix)]
    fn setup_shallow_clone(depth: usize) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let source = setup_temp_git_repo();
        for i in 1..=3 {
            std::fs::write(source.path().join("f.txt"), format!("{i}\n")).unwrap();
            run_git_in(source.path(), &["add", "f.txt"]);
            run_git_in(source.path(), &["commit", "-m", &format!("c{i}")]);
        }
        let clones = tempfile::TempDir::new().unwrap();
        let url = format!("file://{}", source.path().display());
        let depth = depth.to_string();
        run_git_in(
            clones.path(),
            &["clone", "-q", "--depth", &depth, &url, "shallow"],
        );
        let repo = clones.path().join("shallow");
        run_git_in(&repo, &["config", "user.name", "Test User"]);
        run_git_in(&repo, &["config", "user.email", "test@example.com"]);
        assert_eq!(
            git_output_in(&repo, &["rev-parse", "--is-shallow-repository"]),
            "true"
        );
        (source, clones, repo)
    }

    #[cfg(unix)]
    #[test]
    fn test_reword_refuses_shallow_boundary_but_keeps_head_reword() {
        // 境界のコミットは first-parent の終端に見えるため、以前は最古コミット用の
        // `--root` で作り直され、本来の親を失った新しい root コミットになっていた。
        let (_source, _clones, repo) = setup_shallow_clone(2);
        let service = GitService::with_repo_path(repo.clone());
        let head = git_output_in(&repo, &["rev-parse", "HEAD"]);
        let boundary = git_output_in(&repo, &["rev-parse", "HEAD~1"]);

        let result = service.reword_commit_by_hash(&boundary, "reworded boundary");
        assert!(
            matches!(&result, Err(AppError::InvalidArgument(message)) if message.contains("git fetch --unshallow")),
            "shallow の境界は書き換えずに止めるべき。実際: {result:?}"
        );
        assert_eq!(
            git_output_in(&repo, &["rev-parse", "HEAD"]),
            head,
            "止めたときは履歴を動かしてはいけない"
        );

        // HEAD は境界ではないので、n == 1 の amend 経路はこれまでどおり動き、親も保たれる
        service.reword_commit(1, "reworded head").unwrap();
        assert_eq!(
            git_output_in(&repo, &["log", "-1", "--format=%s"]),
            "reworded head"
        );
        assert_eq!(
            git_output_in(&repo, &["log", "-1", "--format=%P"]),
            boundary
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_reword_and_amend_refuse_head_on_shallow_boundary() {
        // depth 1 では HEAD 自身が境界になる。`git commit --amend` は境界のコミットの
        // 親を引き継げず、parent 行の無いコミットを作る(実測)。
        let (_source, _clones, repo) = setup_shallow_clone(1);
        let service = GitService::with_repo_path(repo.clone());
        let head = git_output_in(&repo, &["rev-parse", "HEAD"]);

        let reword = service.reword_commit(1, "reworded head");
        assert!(
            matches!(&reword, Err(AppError::InvalidArgument(message)) if message.contains("git fetch --unshallow")),
            "HEAD が境界なら reword も止めるべき。実際: {reword:?}"
        );
        let amend = service.amend_commit("amended head");
        assert!(
            matches!(&amend, Err(AppError::InvalidArgument(message)) if message.contains("git fetch --unshallow")),
            "HEAD が境界なら amend も止めるべき。実際: {amend:?}"
        );
        assert_eq!(git_output_in(&repo, &["rev-parse", "HEAD"]), head);
    }

    #[cfg(unix)]
    #[test]
    fn test_shallow_repository_still_rewrites_true_root_of_orphan_branch() {
        // shallow かどうかはリポジトリ全体の状態なので、それだけで止めると、shallow clone の
        // 中で `git switch --orphan` して作った本当の root コミット(親を失う心配が無い)まで
        // 書き換えられなくなる。境界かどうかは書き換える対象のコミットごとに判定する。
        let (_source, _clones, repo) = setup_shallow_clone(2);
        let service = GitService::with_repo_path(repo.clone());
        run_git_in(&repo, &["switch", "-q", "--orphan", "independent"]);
        std::fs::write(repo.join("independent.txt"), "root\n").unwrap();
        run_git_in(&repo, &["add", "independent.txt"]);
        run_git_in(&repo, &["commit", "-q", "-m", "independent root"]);

        service.amend_commit("amended root").unwrap();
        assert_eq!(
            git_output_in(&repo, &["log", "-1", "--format=%s"]),
            "amended root"
        );

        // 2 コミットにして root を reword する(`--root` の経路)
        std::fs::write(repo.join("independent.txt"), "second\n").unwrap();
        run_git_in(&repo, &["add", "independent.txt"]);
        run_git_in(&repo, &["commit", "-q", "-m", "second"]);
        service.reword_commit(2, "reworded root").unwrap();
        assert_eq!(
            git_output_in(&repo, &["log", "-1", "--format=%s", "HEAD~1"]),
            "reworded root"
        );
        assert_eq!(
            git_output_in(&repo, &["log", "-1", "--format=%s"]),
            "second"
        );
    }

    // ============================================================
    // 2 個以上前の reword でも `#` で始まる行を残す
    // ============================================================

    /// コミットメッセージを整形せずにそのまま取り出す
    fn raw_commit_message(repo: &std::path::Path, rev: &str) -> String {
        let output = Command::new("git")
            .args(["cat-file", "commit", rev])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        let object = String::from_utf8(output.stdout).unwrap();
        object
            .split_once("\n\n")
            .map(|(_, message)| message.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn test_reword_older_commit_keeps_hash_lines_like_head_reword() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        for name in ["base", "target", "later"] {
            std::fs::write(repo.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
            run_git_in(repo, &["add", "."]);
            run_git_in(repo, &["commit", "-m", name]);
        }
        let service = GitService::with_repo_path(repo.to_path_buf());

        // 件名だけの場合: 旧方式は件名をコメントとして削り、空メッセージで失敗していた
        service.reword_commit(2, "#123 fix login").unwrap();
        assert_eq!(raw_commit_message(repo, "HEAD~1"), "#123 fix login\n");

        // 本文にも `#` 行がある場合: n == 1(`-m`)と同じ結果になること
        let message = "#123 fix login\n\n# 本文の 1 行目\n本文の 2 行目";
        service.reword_commit(2, message).unwrap();
        service.reword_commit(1, message).unwrap();
        let older = raw_commit_message(repo, "HEAD~1");
        assert_eq!(
            older,
            raw_commit_message(repo, "HEAD"),
            "n == 1 と n >= 2 で結果が食い違ってはいけない"
        );
        assert_eq!(older, format!("{message}\n"));
    }

    #[test]
    fn test_reword_older_commit_leaves_other_commit_messages_byte_identical() {
        // 対象以外のコミットは pick し直すだけなので、メッセージの整形
        // (末尾空白・連続空行・`#` 行の除去)を一切受けてはいけない。
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        for name in ["base", "target"] {
            std::fs::write(repo.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
            run_git_in(repo, &["add", "."]);
            run_git_in(repo, &["commit", "-m", name]);
        }
        let verbatim = "later subject  \n\n\n# not a comment\ntrailing space \n\n";
        let message_file = repo.join(".git").join("git-sc-test-message");
        std::fs::write(&message_file, verbatim).unwrap();
        std::fs::write(repo.join("later.txt"), "later\n").unwrap();
        run_git_in(repo, &["add", "."]);
        run_git_in(
            repo,
            &[
                "commit",
                "--cleanup=verbatim",
                "-F",
                message_file.to_str().unwrap(),
            ],
        );
        assert_eq!(raw_commit_message(repo, "HEAD"), verbatim);

        let service = GitService::with_repo_path(repo.to_path_buf());
        service.reword_commit(2, "reworded target").unwrap();

        assert_eq!(raw_commit_message(repo, "HEAD"), verbatim);
        assert_eq!(raw_commit_message(repo, "HEAD~1"), "reworded target\n");
        assert_eq!(raw_commit_message(repo, "HEAD~2"), "base\n");
    }

    // ============================================================
    // 改行コード(CRLF)だけの変更も AI から見える
    // ============================================================

    #[rstest]
    #[case("")]
    #[case("\n")]
    #[case("a")]
    #[case("a\n")]
    #[case("a\nb")]
    #[case("a\n\nb\n\n")]
    #[case("lone \r inside\n")]
    fn test_split_diff_lines_matches_str_lines_without_crlf(#[case] text: &str) {
        // `\r\n` を含まない入力では `lines()` と同じ区切り方になる(末尾の改行の扱いも同じ)
        assert_eq!(
            GitService::split_diff_lines(text),
            text.lines().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_split_diff_lines_keeps_carriage_return_before_newline() {
        assert_eq!(
            GitService::split_diff_lines("-a\n+a\r\n+b\r"),
            vec!["-a", "+a\r", "+b\r"]
        );
    }

    #[test]
    fn test_summarize_diff_keeps_carriage_returns_of_crlf_lines() {
        let diff = "diff --git a/x.txt b/x.txt\n@@ -1 +1 @@\n-line\n+line\r\n";
        assert_eq!(
            GitService::summarize_diff(diff),
            "diff --git a/x.txt b/x.txt\n@@ -1 +1 @@\n-line\n+line\r"
        );
    }

    #[test]
    fn test_filter_ignored_files_keeps_carriage_returns_of_crlf_lines() {
        let mut builder = GitignoreBuilder::new(".");
        builder.add_line(None, "secret/").unwrap();
        let ignore = builder.build().unwrap();
        let diff = "diff --git a/x.txt b/x.txt\n@@ -1 +1 @@\n-line\n+line\r\n";
        assert_eq!(
            GitService::filter_ignored_files(diff, &ignore),
            "diff --git a/x.txt b/x.txt\n@@ -1 +1 @@\n-line\n+line\r"
        );
    }

    #[test]
    fn test_line_ending_only_change_is_visible_in_staged_diff() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        run_git_in(repo, &["config", "core.autocrlf", "false"]);
        std::fs::write(repo.join("crlf.txt"), "alpha\nbeta\n").unwrap();
        run_git_in(repo, &["add", "crlf.txt"]);
        run_git_in(repo, &["commit", "-m", "lf"]);
        std::fs::write(repo.join("crlf.txt"), "alpha\r\nbeta\r\n").unwrap();
        run_git_in(repo, &["add", "crlf.txt"]);
        let service = GitService::with_repo_path(repo.to_path_buf());

        // `.git-sc-ignore` がある場合は filter_ignored_files も通るため、両方で確かめる
        for with_ignore_file in [false, true] {
            if with_ignore_file {
                std::fs::write(repo.join(".git-sc-ignore"), "unrelated/\n").unwrap();
            }
            let diff = service.get_staged_diff().unwrap();
            assert!(
                diff.contains("-alpha\n-beta\n+alpha\r\n+beta\r"),
                "ignore ファイルあり = {with_ignore_file}: {diff:?}"
            );
        }
    }

    // ============================================================
    // 作業ツリーにリビジョンと同名のファイルがあっても履歴を読める
    // ============================================================

    #[test]
    fn test_history_apis_work_with_file_named_head_in_cwd() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        for i in 1..=3 {
            std::fs::write(repo.join("f.txt"), format!("{i}\n")).unwrap();
            run_git_in(repo, &["add", "f.txt"]);
            run_git_in(repo, &["commit", "-m", &format!("c{i}")]);
        }
        let base = git_output_in(repo, &["rev-parse", "HEAD~2"]);
        // `--` の無いリビジョン指定は作業ツリーのファイル名と照合され、
        // 「リビジョンかファイル名か曖昧」として git が拒否する
        std::fs::write(repo.join("HEAD"), "untracked file named HEAD\n").unwrap();
        let service = GitService::with_repo_path(repo.to_path_buf());

        assert!(service.get_diff_from_base(&base).unwrap().contains("+3"));
        assert!(service.get_last_commit_diff().unwrap().contains("+3"));
        assert_eq!(service.get_commit_message_by_hash("HEAD").unwrap(), "c3");
        assert_eq!(service.get_head_first_parent_count().unwrap(), 3);
        assert!(!service.has_merge_commits_in_range(1).unwrap());
        assert!(!service.has_merge_commits_in_range(3).unwrap());
        service.reword_commit(2, "reworded c2").unwrap();
        service.reword_commit(1, "reworded c3").unwrap();
        assert_eq!(
            git_output_in(repo, &["log", "--format=%s"]),
            "reworded c3\nreworded c2\nc1"
        );
    }

    #[test]
    fn test_commit_apis_work_with_file_named_like_target_commit() {
        // `HEAD` などの名前はコミット ID へ解決してから渡すが、解決後の ID と同じ名前の
        // ファイルがあっても曖昧にならないよう、どの呼び出しも `--` で区切る。
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("f.txt"), "1\n").unwrap();
        run_git_in(repo, &["add", "f.txt"]);
        run_git_in(repo, &["commit", "-m", "c1"]);
        let first = git_output_in(repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("f.txt"), "2\n").unwrap();
        run_git_in(repo, &["commit", "-am", "c2"]);
        let second = git_output_in(repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join(&first), "file named like the first commit\n").unwrap();
        std::fs::write(repo.join(&second), "file named like the second commit\n").unwrap();
        let service = GitService::with_repo_path(repo.to_path_buf());

        assert!(service.get_last_commit_diff().unwrap().contains("+2"));
        assert_eq!(service.get_commit_message_by_hash("HEAD").unwrap(), "c2");
        service.soft_reset_to(&first).unwrap();
        assert_eq!(git_output_in(repo, &["rev-parse", "HEAD"]), first);
    }

    // ============================================================
    // 注釈付きタグを指定しても tagger とタグ本文を AI に渡さない
    // ============================================================

    #[test]
    fn test_commit_lookup_by_annotated_tag_excludes_tag_object() {
        let temp_dir = setup_temp_git_repo();
        let repo = temp_dir.path();
        std::fs::write(repo.join("f.txt"), "tagged change\n").unwrap();
        run_git_in(repo, &["add", "f.txt"]);
        run_git_in(repo, &["commit", "-m", "feat: tagged subject"]);
        run_git_in(repo, &["config", "user.name", "Tagger Person"]);
        run_git_in(repo, &["config", "user.email", "tagger@example.com"]);
        run_git_in(
            repo,
            &["tag", "-a", "v1", "-m", "TAG-BODY-MUST-NOT-REACH-AI"],
        );
        let service = GitService::with_repo_path(repo.to_path_buf());

        let diff = service.get_commit_diff_by_hash("v1").unwrap();
        assert!(diff.starts_with("diff --git "), "{diff}");
        assert!(diff.contains("+tagged change"), "{diff}");
        for leaked in [
            "TAG-BODY-MUST-NOT-REACH-AI",
            "Tagger",
            "tagger@example.com",
            "tag v1",
        ] {
            assert!(!diff.contains(leaked), "{leaked}: {diff}");
        }
        assert_eq!(
            service.get_commit_message_by_hash("v1").unwrap(),
            "feat: tagged subject"
        );
    }
}
