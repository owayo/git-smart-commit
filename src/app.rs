use std::io::{self, Write};

use colored::Colorize;
use regex::Regex;

use crate::ai::{AiProvider, AiService};
use crate::cli::Cli;
use crate::config::{Config, PrefixRuleConfig, PrefixScriptConfig};
use crate::error::AppError;
use crate::git::{GitService, ScriptResult};

/// プレフィックス判定結果
pub enum PrefixMode {
    /// スクリプトによるプレフィックス
    Script(ScriptResult),
    /// ルールによるプレフィックスタイプ指定
    Rule(String),
    /// 設定ファイルによるプレフィックスタイプ指定
    Config(String),
    /// 自動判定（過去コミットから推論）
    Auto,
}

/// 有効な prefix_type 値
const VALID_PREFIX_TYPES: &[&str] = &["conventional", "bracket", "colon", "emoji", "plain", "none"];
const CONVENTIONAL_TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// prefix_type が有効かどうかを検証
fn is_valid_prefix_type(prefix_type: &str) -> bool {
    VALID_PREFIX_TYPES.contains(&prefix_type)
}

/// Conventional Commits の type プレフィックスを検出し、本文を返す
fn extract_conventional_body(message: &str) -> Option<&str> {
    let first_line_end = message.find('\n').unwrap_or(message.len());
    let first_line = &message[..first_line_end];
    let colon_pos = first_line.find(':')?;
    let mut header = &first_line[..colon_pos];

    // Breaking change マーカー `!` を許可
    header = header.strip_suffix('!').unwrap_or(header);

    let commit_type = if let Some((ty, scope)) = header.split_once('(') {
        if !scope.ends_with(')') || scope.len() <= 1 {
            return None;
        }
        ty
    } else {
        header
    };

    let is_conventional = CONVENTIONAL_TYPES
        .iter()
        .any(|known| known.eq_ignore_ascii_case(commit_type));
    if !is_conventional {
        return None;
    }

    Some(message[colon_pos + 1..].trim_start())
}

/// ScriptResult をチェックし、有効な prefix_type 名ならば PrefixMode::Rule に変換する。
/// それ以外は PrefixMode::Script として返す。
fn resolve_script_result(result: ScriptResult) -> PrefixMode {
    if let ScriptResult::Prefix(ref s) = result {
        let trimmed = s.trim();
        if is_valid_prefix_type(trimmed) {
            return PrefixMode::Rule(trimmed.to_string());
        }
    }
    PrefixMode::Script(result)
}

/// アプリケーションのメインオーケストレーター
pub struct App {
    git: GitService,
    ai: AiService,
    prefix_scripts: Vec<PrefixScriptConfig>,
    prefix_rules: Vec<PrefixRuleConfig>,
    /// 設定ファイルで指定された prefix_type
    prefix_type: Option<String>,
    /// 設定ファイルで指定された auto_push
    auto_push: Option<bool>,
    /// NanoBuddy連携の有効/無効
    nano_buddy: bool,
}

impl App {
    /// 新しいAppインスタンスを作成
    pub fn new(cli: &Cli) -> Result<Self, AppError> {
        let config = Config::load()?;

        // デバッグモード: 設定ファイル情報を表示
        if cli.debug {
            Self::print_config_debug(&config)?;
        }

        let mut ai = AiService::from_config(&config);

        // デバッグモードを設定
        if cli.debug {
            ai.set_debug(true);
        }

        // CLIで言語が指定されていれば上書き
        if let Some(ref lang) = cli.language {
            ai.set_language(lang.clone());
        }

        // CLIでプロバイダーが指定されていれば上書き
        if let Some(ref provider_name) = cli.provider {
            let provider = AiProvider::from_str(provider_name).ok_or_else(|| {
                AppError::InvalidArgument(format!(
                    "Unknown provider '{}'. Valid providers: gemini, codex, claude, opencode, apple-intelligence",
                    provider_name
                ))
            })?;
            ai.set_provider_override(provider);
        }

        Ok(Self {
            git: GitService::new(),
            ai,
            prefix_scripts: config.prefix_scripts.clone(),
            prefix_rules: config.prefix_rules.clone(),
            prefix_type: config.prefix_type.clone(),
            auto_push: config.auto_push,
            nano_buddy: cfg!(target_os = "macos") && config.nano_buddy,
        })
    }

    /// デバッグモード: 設定ファイル情報を表示
    fn print_config_debug(config: &Config) -> Result<(), AppError> {
        println!();
        println!("{}", "=== DEBUG: Config Settings ===".yellow().bold());
        println!("{}", "─".repeat(50).dimmed());

        // グローバル設定ファイルパス
        if let Ok(global_path) = Config::global_config_path() {
            if global_path.exists() {
                println!(
                    "  Global config: {}",
                    global_path.display().to_string().cyan()
                );
            } else {
                println!(
                    "  Global config: {} (not found)",
                    global_path.display().to_string().dimmed()
                );
            }
        }

        // プロジェクト設定ファイルパス
        if let Ok(Some(project_path)) = Config::project_config_path() {
            println!(
                "  Project config: {}",
                project_path.display().to_string().cyan()
            );
        } else {
            println!("  Project config: {}", "(not found)".dimmed());
        }

        println!("{}", "─".repeat(50).dimmed());
        println!("{}", "Effective settings:".yellow());
        println!("  providers: {:?}", config.providers);
        println!("  language: {}", config.language);
        println!("  models.opencode: {}", config.models.opencode);
        println!("  models.gemini: {}", config.models.gemini);
        println!("  models.codex: {}", config.models.codex);
        println!(
            "  codex_reasoning_effort: {}",
            if config.codex_reasoning_effort.is_empty() {
                "(omitted)".to_string()
            } else {
                config.codex_reasoning_effort.clone()
            }
        );
        println!("  models.claude: {}", config.models.claude);
        println!("  prefix_type: {:?}", config.prefix_type);
        println!("  auto_push: {:?}", config.auto_push);
        println!("  nano_buddy: {}", config.nano_buddy);
        println!("  prefix_scripts: {} rule(s)", config.prefix_scripts.len());
        println!("  prefix_rules: {} rule(s)", config.prefix_rules.len());
        println!(
            "  provider_cooldown_minutes: {}",
            config.provider_cooldown_minutes
        );
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", "=== END DEBUG ===".yellow().bold());
        println!();

        Ok(())
    }

    /// プレフィックスモードを判定
    ///
    /// 優先順位:
    /// 1. prefix_scripts: url_patternの正規表現にマッチすればスクリプト実行
    /// 2. prefix_rules: url_patternの正規表現にマッチすればそのprefix_typeを使用
    /// 3. Auto: 上記に該当しなければ過去コミットから自動判定
    fn get_prefix_mode(&self, quiet: bool) -> PrefixMode {
        self.get_prefix_mode_internal(quiet)
    }

    /// 内部実装: プレフィックスモード判定
    fn get_prefix_mode_internal(&self, silent: bool) -> PrefixMode {
        // リモートURLとブランチ名を取得
        let remote_url = match self.git.get_remote_url() {
            Some(url) => url,
            None => return PrefixMode::Auto,
        };
        let branch = self.git.get_current_branch();

        // 1. プレフィックススクリプトをチェック（最優先、正規表現マッチ）
        for script_config in &self.prefix_scripts {
            if let Ok(re) = Regex::new(&script_config.url_pattern)
                && re.is_match(&remote_url)
            {
                if !silent {
                    println!(
                        "{}",
                        format!("Running prefix script for {}...", script_config.url_pattern)
                            .cyan()
                    );
                }
                if let Some(branch_name) = &branch {
                    let result =
                        self.git
                            .run_prefix_script(&script_config.script, &remote_url, branch_name);

                    // スクリプト実行結果を出力
                    if !silent {
                        match &result {
                            Some(ScriptResult::Prefix(prefix)) => {
                                println!("{}", format!("  → prefix: {:?}", prefix.trim()).green());
                            }
                            Some(ScriptResult::Empty) => {
                                println!(
                                    "{}",
                                    "  → prefix: (empty, no prefix will be added)".yellow()
                                );
                            }
                            Some(ScriptResult::Failed) => {
                                println!(
                                    "{}",
                                    "  → script exited with non-zero status (exit 1), using AI-generated message".yellow()
                                );
                            }
                            None => {
                                println!("{}", "  → script execution failed".red());
                            }
                        }
                    }

                    if let Some(r) = result {
                        let mode = resolve_script_result(r);
                        if !silent && let PrefixMode::Rule(ref pt) = mode {
                            println!(
                                "{}",
                                format!("  → interpreted as prefix_type: {}", pt).cyan()
                            );
                        }
                        return mode;
                    }
                }
            }
        }

        // 2. プレフィックスルールをチェック（正規表現マッチ）
        for rule_config in &self.prefix_rules {
            if let Ok(re) = Regex::new(&rule_config.url_pattern)
                && re.is_match(&remote_url)
            {
                if !silent {
                    println!(
                        "{}",
                        format!(
                            "Using prefix rule for {}: {}",
                            rule_config.url_pattern, rule_config.prefix_type
                        )
                        .cyan()
                    );
                }
                return PrefixMode::Rule(rule_config.prefix_type.clone());
            }
        }

        // 3. 設定ファイルの prefix_type をチェック
        if let Some(ref prefix_type) = self.prefix_type {
            if is_valid_prefix_type(prefix_type) {
                if !silent {
                    println!(
                        "{}",
                        format!("Using config prefix_type: {}", prefix_type).cyan()
                    );
                }
                return PrefixMode::Config(prefix_type.clone());
            } else {
                // 無効な prefix_type の場合は警告を出力
                eprintln!(
                    "{}",
                    format!(
                        "警告: 無効な prefix_type '{}' が設定されています。有効な値: {:?}",
                        prefix_type, VALID_PREFIX_TYPES
                    )
                    .yellow()
                );
            }
        }

        // 4. 該当なし: 自動判定モード
        PrefixMode::Auto
    }

    /// コミットメッセージにプレフィックスを適用
    fn apply_prefix(&self, message: &str, prefix: &str) -> String {
        // Conventional Commits形式（type: message）の場合のみ type を削除して置き換える
        if let Some(body) = extract_conventional_body(message) {
            format!("{}{}", prefix, body)
        } else {
            // Conventional Commits 形式でない場合はそのまま結合
            format!("{}{}", prefix, message)
        }
    }

    /// コミットメッセージから型プレフィックスを削除（本文のみ取得）
    fn strip_type_prefix(&self, message: &str) -> String {
        if let Some(body) = extract_conventional_body(message) {
            body.to_string()
        } else {
            message.to_string()
        }
    }

    /// PrefixModeからデバッグ用のパラメータを抽出
    fn get_debug_params_for_prefix_mode<'a>(
        prefix_mode: &'a PrefixMode,
        recent_commits: &'a [String],
        is_squash: bool,
    ) -> (Option<&'a str>, &'a [String]) {
        let prefix_type = match prefix_mode {
            PrefixMode::Script(_) => Some("plain"),
            PrefixMode::Rule(pt) => Some(pt.as_str()),
            PrefixMode::Config(pt) => Some(pt.as_str()),
            PrefixMode::Auto => {
                if is_squash {
                    Some("conventional")
                } else {
                    None
                }
            }
        };
        let commits = match prefix_mode {
            PrefixMode::Script(_) => &[][..],
            _ => {
                if is_squash {
                    &[][..]
                } else {
                    recent_commits
                }
            }
        };
        (prefix_type, commits)
    }

    /// デバッグモード時にプロンプトを表示
    fn print_debug_prompt(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_type: Option<&str>,
        with_body: bool,
        agent_context: Option<&str>,
        use_stderr: bool,
    ) {
        let prompt = AiService::build_prompt(
            diff,
            recent_commits,
            self.ai.language(),
            prefix_type,
            with_body,
            agent_context,
        );
        macro_rules! debug_out {
            ($($arg:tt)*) => {
                if use_stderr { eprintln!($($arg)*) } else { println!($($arg)*) }
            };
        }
        debug_out!();
        debug_out!("{}", "=== DEBUG: AI Prompt ===".yellow().bold());
        debug_out!("{}", "─".repeat(50).dimmed());
        debug_out!("{}", prompt);
        debug_out!("{}", "─".repeat(50).dimmed());
        debug_out!("{}", "=== END DEBUG ===".yellow().bold());
        debug_out!();
    }

    fn print_recent_commits_for_auto(&self, cli: &Cli, recent_commits: &[String]) {
        if cli.quiet {
            return;
        }

        if recent_commits.is_empty() {
            println!(
                "{} {}",
                "No recent commits found.".cyan(),
                "Using Conventional Commits format.".yellow()
            );
        } else {
            println!("{}", "Recent commits (for format reference):".cyan());
            for commit in recent_commits {
                println!("  {}", commit.dimmed());
            }
        }
    }

    fn print_generated_message(&self, cli: &Cli, message: &str, provider_name: &str) {
        if cli.quiet {
            // quietモードでもプロバイダー名とコミットメッセージは出力する
            println!("[{}] {}", provider_name, message);
            return;
        }

        println!();
        println!("{}", "Generated commit message:".green().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", message);
        println!("{}", "─".repeat(50).dimmed());
        println!();
    }

    /// PrefixModeに基づいてメッセージを生成し、スクリプトプレフィックスを適用する共通メソッド
    ///
    /// # 引数
    /// - `cli`: CLIオプション
    /// - `diff`: 差分テキスト
    /// - `recent_commits`: フォーマット参照用の直近コミット
    /// - `prefix_mode`: プレフィックス判定結果
    /// - `is_squash`: squashモードかどうか（Autoモード時にconventionalを強制する）
    /// - `agent_context`: エージェントコンテキスト
    /// - `silent`: trueの場合、進捗メッセージを抑制しデバッグ出力をstderrに出力
    #[allow(clippy::too_many_arguments)]
    fn generate_with_prefix(
        &self,
        cli: &Cli,
        diff: &str,
        recent_commits: &[String],
        prefix_mode: PrefixMode,
        is_squash: bool,
        agent_context: Option<&str>,
        silent: bool,
    ) -> Result<(String, &'static str), AppError> {
        let quiet = silent || cli.quiet;

        // デバッグモード: プロンプトを表示
        if cli.debug {
            let (prefix_type, commits) =
                Self::get_debug_params_for_prefix_mode(&prefix_mode, recent_commits, is_squash);
            self.print_debug_prompt(
                diff,
                commits,
                prefix_type,
                cli.with_body,
                agent_context,
                silent,
            );
        }

        // PrefixModeに基づいてメッセージを生成
        let (mut message, provider_name) = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: プレフィックスなしで生成（後でスクリプトのプレフィックスを適用）
                self.ai.generate_commit_message(
                    diff,
                    &[],
                    Some("plain"),
                    cli.with_body,
                    quiet,
                    agent_context,
                )?
            }
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai.generate_commit_message(
                    diff,
                    recent_commits,
                    Some(prefix_type),
                    cli.with_body,
                    quiet,
                    agent_context,
                )?
            }
            PrefixMode::Auto => {
                if is_squash {
                    // squashモード: Conventional Commits形式で生成
                    self.ai.generate_commit_message(
                        diff,
                        &[],
                        Some("conventional"),
                        cli.with_body,
                        quiet,
                        agent_context,
                    )?
                } else {
                    // 通常モード: 過去コミットから推論
                    self.ai.generate_commit_message(
                        diff,
                        recent_commits,
                        None,
                        cli.with_body,
                        quiet,
                        agent_context,
                    )?
                }
            }
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                    if !quiet {
                        println!("{}", format!("Applied prefix: {}", prefix.trim()).cyan());
                    }
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                    if !quiet {
                        println!("{}", "No prefix applied (script returned empty).".cyan());
                    }
                }
                ScriptResult::Failed => {
                    // AI生成のメッセージをそのまま使用
                    if !quiet {
                        println!("{}", "Using AI-generated format.".cyan());
                    }
                }
            }
        }

        Ok((message, provider_name))
    }

    /// メインワークフローを実行
    pub fn run(&self, cli: &Cli) -> Result<(), AppError> {
        // claw-hooks stop hook から渡されるエージェントコンテキスト
        let agent_context = std::env::var("CLAW_HOOKS_AGENT_MESSAGE").ok();

        // Gitリポジトリかどうかを確認
        self.git.verify_repository()?;

        // --generate-forモードは別処理（排他チェック付き）
        if cli.generate_for.is_some() {
            // 排他チェック
            if cli.reword.is_some() {
                return Err(AppError::ConflictingOptions("reword".to_string()));
            }
            if cli.amend {
                return Err(AppError::ConflictingOptions("amend".to_string()));
            }
            if cli.squash.is_some() {
                return Err(AppError::ConflictingOptions("squash".to_string()));
            }
            return self.run_generate_for(cli, agent_context.as_deref());
        }

        // --rewordモードは別処理
        if cli.reword.is_some() {
            return self.run_reword(cli, agent_context.as_deref());
        }

        // --amendモードは別処理
        if cli.amend {
            return self.run_amend(cli, agent_context.as_deref());
        }

        // --squashモードは別処理
        if cli.squash.is_some() {
            return self.run_squash(cli, agent_context.as_deref());
        }

        // --allフラグがあれば全変更をステージング
        if cli.stage_all {
            if !cli.quiet {
                println!("{}", "Staging all changes...".cyan());
            }
            self.git.stage_all()?;
        }

        // ステージ済みのdiffを取得
        let staged_diff = self.git.get_staged_diff()?;
        let diff = if !staged_diff.trim().is_empty() {
            staged_diff
        } else if cli.stage_all {
            // --allフラグ指定時で変更がない場合は正常終了
            if !cli.quiet {
                println!("{}", "変更がありません。".cyan());
            }
            return Ok(());
        } else {
            // デフォルト: ステージ済みのみ
            return Err(AppError::NoStagedChanges);
        };

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode(cli.quiet);

        // フォーマット検出用に直近のコミットを取得（Autoモードの場合のみ表示）
        let recent_commits = self.git.get_recent_commits(5)?;

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            self.print_recent_commits_for_auto(cli, &recent_commits);
        }

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // コミットメッセージを生成
        if !cli.quiet {
            println!("{}", "Generating commit message...".cyan());
        }

        let agent_ctx = agent_context.as_deref();

        let (message, provider_name) = self.generate_with_prefix(
            cli,
            &diff,
            &recent_commits,
            prefix_mode,
            false,
            agent_ctx,
            false,
        )?;

        // 生成されたメッセージを表示
        self.print_generated_message(cli, &message, provider_name);

        // ドライランモードの処理
        if cli.dry_run {
            if !cli.quiet {
                println!("{}", "Dry run mode - no commit was made.".yellow());
            }
            return Ok(());
        }

        // 確認してコミット
        if cli.auto_confirm || self.confirm_commit()? {
            // コミット直前にステージ済み変更を再確認（race condition 防止）
            if !self.git.has_staged_changes() {
                if !cli.quiet {
                    println!(
                        "{}",
                        "ステージ済みの変更がありません。コミットをスキップしました。".yellow()
                    );
                }
                return Ok(());
            }
            self.git.commit(&message)?;
            if cli.quiet {
                println!("Committed");
            } else {
                println!("{}", "✓ Commit created successfully!".green().bold());
            }
            if self.nano_buddy {
                crate::notify::notify_commit_message(&message);
            }

            // auto-push が有効な場合は push も実行
            if self.git.is_auto_push_enabled(self.auto_push) {
                self.git.push()?;
                if cli.quiet {
                    println!("Pushed");
                } else {
                    println!("{}", "✓ Pushed to remote successfully!".green().bold());
                }
            }
        } else {
            if !cli.quiet {
                println!("{}", "Commit cancelled.".yellow());
            }
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// amendワークフローを実行
    fn run_amend(&self, cli: &Cli, agent_context: Option<&str>) -> Result<(), AppError> {
        if !cli.quiet {
            println!(
                "{}",
                "Amend mode: regenerating message for last commit...".cyan()
            );
        }

        // 直前のコミットのdiffを取得
        let diff = self.git.get_last_commit_diff()?;
        if diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode(cli.quiet);

        // フォーマット検出用に直近のコミットを取得（amendするコミットはスキップ）
        let recent_commits = self.git.get_recent_commits(6)?;
        let recent_commits: Vec<String> = recent_commits.into_iter().skip(1).collect();

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            self.print_recent_commits_for_auto(cli, &recent_commits);
        }

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // コミットメッセージを生成
        if !cli.quiet {
            println!("{}", "Generating commit message...".cyan());
        }

        let (message, provider_name) = self.generate_with_prefix(
            cli,
            &diff,
            &recent_commits,
            prefix_mode,
            false,
            agent_context,
            false,
        )?;

        // 生成されたメッセージを表示
        self.print_generated_message(cli, &message, provider_name);

        // ドライランモードの処理
        if cli.dry_run {
            if !cli.quiet {
                println!("{}", "Dry run mode - commit was not amended.".yellow());
            }
            return Ok(());
        }

        // 確認してamend
        if cli.auto_confirm || self.confirm_amend()? {
            self.git.amend_commit(&message)?;
            if cli.quiet {
                println!("Amended");
            } else {
                println!("{}", "✓ Commit amended successfully!".green().bold());
            }
            if self.nano_buddy {
                crate::notify::notify_commit_message(&message);
            }
        } else {
            if !cli.quiet {
                println!("{}", "Amend cancelled.".yellow());
            }
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// squashワークフローを実行
    fn run_squash(&self, cli: &Cli, agent_context: Option<&str>) -> Result<(), AppError> {
        // ベースブランチを取得（必須）
        let base_branch = cli.squash.as_ref().ok_or(AppError::NoBaseBranch)?;

        // ベースブランチの存在確認
        if !self.git.branch_exists(base_branch) {
            return Err(AppError::GitError(format!(
                "Base branch '{}' does not exist",
                base_branch
            )));
        }

        if !cli.quiet {
            println!("{}", "Squash mode: combining commits into one...".cyan());
        }

        // 現在のブランチを取得
        let current_branch = self
            .git
            .get_current_branch()
            .ok_or_else(|| AppError::GitError("Failed to get current branch".to_string()))?;

        // ベースブランチ上にいる場合はエラー
        if current_branch == *base_branch {
            return Err(AppError::OnBaseBranch);
        }

        if !cli.quiet {
            println!(
                "{}",
                format!(
                    "Base branch: {} → Current branch: {}",
                    base_branch, current_branch
                )
                .cyan()
            );
        }

        // merge-baseを取得
        let merge_base = self.git.get_merge_base(base_branch, "HEAD")?;

        // コミット数を確認
        let commit_count = self.git.count_commits_from_base(&merge_base)?;
        if commit_count == 0 {
            return Err(AppError::NoCommitsToSquash);
        }

        if !cli.quiet {
            println!("{}", format!("Commits to squash: {}", commit_count).cyan());
        }

        // ベースからの差分を取得
        let diff = self.git.get_diff_from_base(&merge_base)?;
        if diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode(cli.quiet);

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // コミットメッセージを生成（差分のみから、過去コミットは参照しない）
        if !cli.quiet {
            println!("{}", "Generating commit message...".cyan());
        }

        let (message, provider_name) =
            self.generate_with_prefix(cli, &diff, &[], prefix_mode, true, agent_context, false)?;

        // 生成されたメッセージを表示
        self.print_generated_message(cli, &message, provider_name);

        // ドライランモードの処理
        if cli.dry_run {
            if !cli.quiet {
                println!("{}", "Dry run mode - no squash was performed.".yellow());
            }
            return Ok(());
        }

        // 確認してsquash実行
        if cli.auto_confirm || self.confirm_squash(commit_count)? {
            // soft resetしてコミット
            self.git.soft_reset_to(&merge_base)?;
            self.git.commit(&message)?;
            if cli.quiet {
                println!("Squashed");
            } else {
                println!(
                    "{}",
                    format!("✓ {} commits squashed successfully!", commit_count)
                        .green()
                        .bold()
                );
            }
            if self.nano_buddy {
                crate::notify::notify_commit_message(&message);
            }

            // auto-push が有効な場合は push も実行
            if self.git.is_auto_push_enabled(self.auto_push) {
                self.git.push()?;
                if cli.quiet {
                    println!("Pushed");
                } else {
                    println!("{}", "✓ Pushed to remote successfully!".green().bold());
                }
            }
        } else {
            if !cli.quiet {
                println!("{}", "Squash cancelled.".yellow());
            }
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// generate-forワークフローを実行（標準出力にメッセージのみ出力）
    fn run_generate_for(&self, cli: &Cli, agent_context: Option<&str>) -> Result<(), AppError> {
        let hashes = cli
            .generate_for
            .as_ref()
            .ok_or_else(|| AppError::InvalidCommitHash("(empty)".to_string()))?;

        if hashes.is_empty() {
            return Err(AppError::InvalidCommitHash("(empty)".to_string()));
        }

        // 各コミットのdiffを取得して結合
        let mut combined_diff = String::new();
        for hash in hashes {
            let diff = self.git.get_commit_diff_by_hash(hash)?;
            if !diff.trim().is_empty() {
                if !combined_diff.is_empty() {
                    combined_diff.push('\n');
                }
                combined_diff.push_str(&diff);
            }
        }

        if combined_diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // プレフィックスモードを判定（サイレントモード）
        let prefix_mode = self.get_prefix_mode(true);

        // フォーマット検出用に直近のコミットを取得
        let recent_commits = self.git.get_recent_commits(5)?;

        // コミットメッセージを生成（サイレントモード: 進捗抑制、デバッグ出力はstderr）
        let (message, _provider_name) = self.generate_with_prefix(
            cli,
            &combined_diff,
            &recent_commits,
            prefix_mode,
            false,
            agent_context,
            true,
        )?;

        // 標準出力にメッセージのみを出力（余計な装飾なし）
        println!("{}", message);

        Ok(())
    }

    /// rewordワークフローを実行
    fn run_reword(&self, cli: &Cli, agent_context: Option<&str>) -> Result<(), AppError> {
        let hash = cli
            .reword
            .as_ref()
            .ok_or(AppError::InvalidRewordTarget)?
            .clone();

        // 短いハッシュを取得して表示用に使用
        let short_hash = if hash.len() > 7 { &hash[..7] } else { &hash };

        if !cli.quiet {
            println!(
                "{}",
                format!(
                    "Reword mode: regenerating message for commit {}...",
                    short_hash
                )
                .cyan()
            );
        }

        // マージコミットが含まれていないか確認
        if self.git.has_merge_commits_in_range_by_hash(&hash)? {
            return Err(AppError::HasMergeCommits);
        }

        // ハッシュの位置を取得（recent_commits のスキップ用）
        let n = self.git.get_commit_position_by_hash(&hash)?;

        // 対象コミットのdiffを取得
        let diff = self.git.get_commit_diff_by_hash(&hash)?;
        if diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // 現在のコミットメッセージを表示
        let current_message = self.git.get_commit_message_by_hash(&hash)?;
        if !cli.quiet {
            println!("{}", "Current commit message:".cyan());
            println!("  {}", current_message.dimmed());
        }

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode(cli.quiet);

        // フォーマット検出用に直近のコミットを取得（対象コミットより新しいものを除く）
        let recent_commits = self.git.get_recent_commits(5 + n)?;
        let recent_commits: Vec<String> = recent_commits.into_iter().skip(n).collect();

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            self.print_recent_commits_for_auto(cli, &recent_commits);
        }

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // コミットメッセージを生成
        if !cli.quiet {
            println!("{}", "Generating commit message...".cyan());
        }

        let (message, provider_name) = self.generate_with_prefix(
            cli,
            &diff,
            &recent_commits,
            prefix_mode,
            false,
            agent_context,
            false,
        )?;

        // 生成されたメッセージを表示
        self.print_generated_message(cli, &message, provider_name);

        // ドライランモードの処理
        if cli.dry_run {
            if !cli.quiet {
                println!("{}", "Dry run mode - commit was not reworded.".yellow());
            }
            return Ok(());
        }

        // 確認してreword実行
        if cli.auto_confirm || self.confirm_reword(short_hash)? {
            self.git.reword_commit_by_hash(&hash, &message)?;
            if cli.quiet {
                println!("Reworded");
            } else {
                println!(
                    "{}",
                    format!("✓ Commit {} reworded successfully!", short_hash)
                        .green()
                        .bold()
                );
            }
            if self.nano_buddy {
                crate::notify::notify_commit_message(&message);
            }
            if !cli.quiet {
                println!(
                    "{}",
                    "Note: You may need to force push (git push --force) if already pushed."
                        .yellow()
                );
            }
        } else {
            if !cli.quiet {
                println!("{}", "Reword cancelled.".yellow());
            }
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// コミット確認プロンプトを表示
    fn confirm_commit(&self) -> Result<bool, AppError> {
        self.confirm_prompt("Create this commit? [Y/n] ")
    }

    /// amend確認プロンプトを表示
    fn confirm_amend(&self) -> Result<bool, AppError> {
        self.confirm_prompt("Amend this commit? [Y/n] ")
    }

    /// squash確認プロンプトを表示
    fn confirm_squash(&self, count: usize) -> Result<bool, AppError> {
        self.confirm_prompt(&format!("Squash {} commits? [Y/n] ", count))
    }

    /// reword確認プロンプトを表示
    fn confirm_reword(&self, hash: &str) -> Result<bool, AppError> {
        self.confirm_prompt(&format!("Reword commit {}? [Y/n] ", hash))
    }

    /// 汎用確認プロンプト
    fn confirm_prompt(&self, prompt: &str) -> Result<bool, AppError> {
        print!("{}", prompt.cyan());
        io::stdout()
            .flush()
            .map_err(|e| AppError::GitError(e.to_string()))?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| AppError::GitError(e.to_string()))?;

        let input = input.trim().to_lowercase();
        Ok(input.is_empty() || input == "y" || input == "yes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    /// テスト用のAppヘルパー構造体（純粋関数のテスト用）
    struct TestHelper;

    impl TestHelper {
        /// apply_prefixのテスト用ラッパー
        fn apply_prefix(message: &str, prefix: &str) -> String {
            if let Some(body) = extract_conventional_body(message) {
                format!("{}{}", prefix, body)
            } else {
                format!("{}{}", prefix, message)
            }
        }

        /// strip_type_prefixのテスト用ラッパー
        fn strip_type_prefix(message: &str) -> String {
            if let Some(body) = extract_conventional_body(message) {
                body.to_string()
            } else {
                message.to_string()
            }
        }
    }

    // ============================================================
    // apply_prefix のテスト
    // ============================================================

    #[rstest]
    #[case("feat: add new feature", "TICKET-123 ", "TICKET-123 add new feature")]
    #[case("feat!: breaking change", "TICKET-123 ", "TICKET-123 breaking change")]
    #[case("fix: bug fix", "[BUG] ", "[BUG] bug fix")]
    #[case("docs: update readme", "📝 ", "📝 update readme")]
    fn test_apply_prefix_with_conventional_commits(
        #[case] message: &str,
        #[case] prefix: &str,
        #[case] expected: &str,
    ) {
        let result = TestHelper::apply_prefix(message, prefix);
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case("add new feature", "TICKET-123 ", "TICKET-123 add new feature")]
    #[case("simple message", "[PREFIX] ", "[PREFIX] simple message")]
    fn test_apply_prefix_without_colon(
        #[case] message: &str,
        #[case] prefix: &str,
        #[case] expected: &str,
    ) {
        let result = TestHelper::apply_prefix(message, prefix);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_apply_prefix_with_scope() {
        let result = TestHelper::apply_prefix("feat(auth): implement login", "PROJ-001 ");
        assert_eq!(result, "PROJ-001 implement login");
    }

    #[test]
    fn test_apply_prefix_preserves_message_body() {
        let result = TestHelper::apply_prefix(
            "refactor: improve code structure with better patterns",
            "🔧 ",
        );
        assert_eq!(result, "🔧 improve code structure with better patterns");
    }

    #[test]
    fn test_apply_prefix_with_empty_prefix() {
        let result = TestHelper::apply_prefix("feat: new feature", "");
        assert_eq!(result, "new feature");
    }

    #[test]
    fn test_apply_prefix_with_multiline_message() {
        let message = "feat: add feature\n\nThis is a detailed description.";
        let result = TestHelper::apply_prefix(message, "TICKET-1 ");
        assert_eq!(
            result,
            "TICKET-1 add feature\n\nThis is a detailed description."
        );
    }

    #[test]
    fn test_apply_prefix_non_conventional_colon_message() {
        let message = "Refactor module: improve parser";
        let result = TestHelper::apply_prefix(message, "TICKET-1 ");
        assert_eq!(result, "TICKET-1 Refactor module: improve parser");
    }

    // ============================================================
    // strip_type_prefix のテスト
    // ============================================================

    #[rstest]
    #[case("feat: add new feature", "add new feature")]
    #[case("feat!: breaking change", "breaking change")]
    #[case("fix: bug fix", "bug fix")]
    #[case("docs: update readme", "update readme")]
    #[case("refactor: improve code", "improve code")]
    #[case("test: add unit tests", "add unit tests")]
    #[case("chore: update deps", "update deps")]
    fn test_strip_type_prefix_conventional_commits(#[case] message: &str, #[case] expected: &str) {
        let result = TestHelper::strip_type_prefix(message);
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case("feat(auth): implement login", "implement login")]
    #[case("feat(auth)!: remove legacy auth flow", "remove legacy auth flow")]
    #[case("fix(api): resolve rate limiting", "resolve rate limiting")]
    fn test_strip_type_prefix_with_scope(#[case] message: &str, #[case] expected: &str) {
        let result = TestHelper::strip_type_prefix(message);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_strip_type_prefix_no_colon() {
        let result = TestHelper::strip_type_prefix("simple message without colon");
        assert_eq!(result, "simple message without colon");
    }

    #[test]
    fn test_strip_type_prefix_extra_whitespace() {
        let result = TestHelper::strip_type_prefix("feat:   extra whitespace");
        assert_eq!(result, "extra whitespace");
    }

    #[test]
    fn test_strip_type_prefix_colon_in_body() {
        // 最初のコロンのみを処理
        let result = TestHelper::strip_type_prefix("feat: update config: new settings");
        assert_eq!(result, "update config: new settings");
    }

    #[test]
    fn test_strip_type_prefix_empty_body() {
        let result = TestHelper::strip_type_prefix("feat:");
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_type_prefix_non_conventional_colon_message() {
        let message = "Refactor module: improve parser";
        let result = TestHelper::strip_type_prefix(message);
        assert_eq!(result, message);
    }

    // ============================================================
    // PrefixMode のテスト
    // ============================================================

    #[test]
    fn test_prefix_mode_variants() {
        // PrefixModeの各バリアントが正しく作成できることを確認
        let _script = PrefixMode::Script(ScriptResult::Prefix("PREFIX ".to_string()));
        let _empty = PrefixMode::Script(ScriptResult::Empty);
        let _failed = PrefixMode::Script(ScriptResult::Failed);
        let _rule = PrefixMode::Rule("conventional".to_string());
        let _config = PrefixMode::Config("bracket".to_string());
        let _auto = PrefixMode::Auto;
    }

    // ============================================================
    // is_valid_prefix_type のテスト
    // ============================================================

    #[rstest]
    #[case("conventional", true)]
    #[case("bracket", true)]
    #[case("colon", true)]
    #[case("emoji", true)]
    #[case("plain", true)]
    #[case("none", true)]
    #[case("invalid", false)]
    #[case("CONVENTIONAL", false)] // 大文字小文字を区別
    #[case("", false)]
    fn test_is_valid_prefix_type(#[case] prefix_type: &str, #[case] expected: bool) {
        assert_eq!(is_valid_prefix_type(prefix_type), expected);
    }

    // ============================================================
    // get_debug_params_for_prefix_mode のテスト
    // ============================================================

    #[test]
    fn test_debug_params_script_mode() {
        let prefix_mode = PrefixMode::Script(ScriptResult::Prefix("PREFIX ".to_string()));
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("plain"));
        assert!(commits.is_empty());
    }

    #[test]
    fn test_debug_params_rule_mode() {
        let prefix_mode = PrefixMode::Rule("bracket".to_string());
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("bracket"));
        assert_eq!(commits.len(), 1);
    }

    #[test]
    fn test_debug_params_config_mode() {
        let prefix_mode = PrefixMode::Config("emoji".to_string());
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("emoji"));
        assert_eq!(commits.len(), 1);
    }

    #[test]
    fn test_debug_params_auto_mode_normal() {
        let prefix_mode = PrefixMode::Auto;
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, None);
        assert_eq!(commits.len(), 1);
    }

    #[test]
    fn test_debug_params_auto_mode_squash() {
        let prefix_mode = PrefixMode::Auto;
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, true);
        assert_eq!(prefix_type, Some("conventional"));
        assert!(commits.is_empty());
    }

    #[test]
    fn test_debug_params_script_failed() {
        let prefix_mode = PrefixMode::Script(ScriptResult::Failed);
        let recent_commits = vec!["fix: bug".to_string(), "feat: add".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("plain"));
        assert!(commits.is_empty());
    }

    #[test]
    fn test_debug_params_script_empty() {
        let prefix_mode = PrefixMode::Script(ScriptResult::Empty);
        let recent_commits = vec![];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("plain"));
        assert!(commits.is_empty());
    }

    // ============================================================
    // VALID_PREFIX_TYPES 定数のテスト
    // ============================================================

    #[test]
    fn test_valid_prefix_types_count() {
        assert_eq!(VALID_PREFIX_TYPES.len(), 6);
    }

    #[test]
    fn test_valid_prefix_types_contains_all() {
        assert!(VALID_PREFIX_TYPES.contains(&"conventional"));
        assert!(VALID_PREFIX_TYPES.contains(&"bracket"));
        assert!(VALID_PREFIX_TYPES.contains(&"colon"));
        assert!(VALID_PREFIX_TYPES.contains(&"emoji"));
        assert!(VALID_PREFIX_TYPES.contains(&"plain"));
        assert!(VALID_PREFIX_TYPES.contains(&"none"));
    }

    // ============================================================
    // resolve_script_result のテスト
    // (スクリプトのPrefix結果をルールモードとして解釈する機能)
    // ============================================================

    #[rstest]
    #[case("conventional")]
    #[case("bracket")]
    #[case("colon")]
    #[case("emoji")]
    #[case("plain")]
    #[case("none")]
    fn test_resolve_script_result_valid_prefix_type_returns_rule(#[case] prefix_type: &str) {
        let result = ScriptResult::Prefix(prefix_type.to_string());
        let mode = resolve_script_result(result);
        match mode {
            PrefixMode::Rule(pt) => assert_eq!(pt, prefix_type),
            _ => panic!("Expected PrefixMode::Rule, got something else"),
        }
    }

    #[rstest]
    #[case("conventional\n")]
    #[case("  bracket  ")]
    #[case("\temoji\t")]
    fn test_resolve_script_result_trims_whitespace(#[case] raw_value: &str) {
        let result = ScriptResult::Prefix(raw_value.to_string());
        let mode = resolve_script_result(result);
        match mode {
            PrefixMode::Rule(pt) => assert_eq!(pt, raw_value.trim()),
            _ => panic!("Expected PrefixMode::Rule after trimming, got something else"),
        }
    }

    #[rstest]
    #[case("TICKET-123 ")]
    #[case("[BUG] ")]
    #[case("feat: ")]
    #[case("some random prefix")]
    #[case("CONVENTIONAL")] // 大文字は無効
    #[case("")]
    fn test_resolve_script_result_non_prefix_type_returns_script(#[case] prefix_value: &str) {
        let result = ScriptResult::Prefix(prefix_value.to_string());
        let mode = resolve_script_result(result);
        match mode {
            PrefixMode::Script(ScriptResult::Prefix(s)) => assert_eq!(s, prefix_value),
            _ => panic!("Expected PrefixMode::Script(Prefix), got something else"),
        }
    }

    #[test]
    fn test_resolve_script_result_empty_returns_script() {
        let result = ScriptResult::Empty;
        let mode = resolve_script_result(result);
        assert!(matches!(mode, PrefixMode::Script(ScriptResult::Empty)));
    }

    #[test]
    fn test_resolve_script_result_failed_returns_script() {
        let result = ScriptResult::Failed;
        let mode = resolve_script_result(result);
        assert!(matches!(mode, PrefixMode::Script(ScriptResult::Failed)));
    }

    #[test]
    fn test_extract_conventional_body_basic() {
        assert_eq!(
            extract_conventional_body("feat: add feature"),
            Some("add feature")
        );
        assert_eq!(extract_conventional_body("fix: bug fix"), Some("bug fix"));
    }

    #[test]
    fn test_extract_conventional_body_with_scope() {
        assert_eq!(
            extract_conventional_body("feat(ui): add button"),
            Some("add button")
        );
        assert_eq!(
            extract_conventional_body("fix(core): resolve crash"),
            Some("resolve crash")
        );
    }

    #[test]
    fn test_extract_conventional_body_breaking_change() {
        assert_eq!(
            extract_conventional_body("feat!: breaking change"),
            Some("breaking change")
        );
        assert_eq!(
            extract_conventional_body("feat(api)!: remove endpoint"),
            Some("remove endpoint")
        );
    }

    #[test]
    fn test_extract_conventional_body_non_conventional() {
        assert_eq!(extract_conventional_body("Update README"), None);
        assert_eq!(extract_conventional_body("random: not conventional"), None);
    }

    #[test]
    fn test_extract_conventional_body_invalid_scope() {
        // 空スコープ
        assert_eq!(extract_conventional_body("feat(): add feature"), None);
        // 閉じ括弧なし
        assert_eq!(extract_conventional_body("feat(ui: add feature"), None);
    }

    #[test]
    fn test_extract_conventional_body_with_multiline() {
        let msg = "feat: add feature\n\nDetailed body";
        assert_eq!(
            extract_conventional_body(msg),
            Some("add feature\n\nDetailed body")
        );
    }

    #[test]
    fn test_extract_conventional_body_case_insensitive() {
        assert_eq!(
            extract_conventional_body("FEAT: uppercase"),
            Some("uppercase")
        );
        assert_eq!(
            extract_conventional_body("Fix: capitalized"),
            Some("capitalized")
        );
    }

    #[test]
    fn test_extract_conventional_body_no_colon() {
        assert_eq!(extract_conventional_body("feat add feature"), None);
    }

    #[test]
    fn test_extract_conventional_body_empty() {
        assert_eq!(extract_conventional_body(""), None);
    }

    // ============================================================
    // extract_conventional_body: 追加エッジケース
    // ============================================================

    #[test]
    fn test_extract_conventional_body_all_types() {
        // 全 conventional type が認識される
        let types = [
            "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore",
            "revert",
        ];
        for ty in types {
            let msg = format!("{}: description", ty);
            assert_eq!(
                extract_conventional_body(&msg),
                Some("description"),
                "type '{}' should be recognized",
                ty
            );
        }
    }

    #[test]
    fn test_extract_conventional_body_colon_space_only() {
        // "feat: " はコロン後がスペースのみ → trim_start で空文字列
        assert_eq!(extract_conventional_body("feat: "), Some(""));
    }

    #[test]
    fn test_extract_conventional_body_no_space_after_colon() {
        // "feat:no-space" はコロン後にスペースなし → trim_start で "no-space"
        assert_eq!(extract_conventional_body("feat:no-space"), Some("no-space"));
    }

    #[test]
    fn test_extract_conventional_body_unknown_type() {
        // conventional type でないものは None
        assert_eq!(extract_conventional_body("unknown_type: some text"), None);
    }

    #[test]
    fn test_extract_conventional_body_scope_with_breaking() {
        // マルチ文字スコープ + breaking change
        assert_eq!(
            extract_conventional_body("feat(auth-module)!: major change"),
            Some("major change")
        );
    }

    #[test]
    fn test_extract_conventional_body_open_paren_no_close() {
        // 開き括弧のみで閉じ括弧なし → None
        assert_eq!(extract_conventional_body("feat(: invalid scope"), None);
    }

    // ============================================================
    // apply_prefix: breaking change + scope の組み合わせ
    // ============================================================

    #[test]
    fn test_apply_prefix_breaking_change_with_scope() {
        // スコープ付きbreaking change のプレフィックス置換
        let result = TestHelper::apply_prefix("feat(api)!: remove legacy endpoint", "BREAKING ");
        assert_eq!(result, "BREAKING remove legacy endpoint");
    }

    #[test]
    fn test_apply_prefix_multiline_body_preserves_all_lines() {
        // 複数行の本文が全て保持される
        let message = "fix(db): resolve connection leak\n\n- Close idle connections\n- Add timeout\n- Update pool config";
        let result = TestHelper::apply_prefix(message, "[DB] ");
        assert_eq!(
            result,
            "[DB] resolve connection leak\n\n- Close idle connections\n- Add timeout\n- Update pool config"
        );
    }

    // ============================================================
    // strip_type_prefix: 複雑なケース
    // ============================================================

    #[test]
    fn test_strip_type_prefix_breaking_with_scope_and_multiline() {
        // スコープ付きbreaking change + 複数行の本文
        let message = "feat(auth)!: rewrite token validation\n\nMigrate from JWT to Paseto";
        let result = TestHelper::strip_type_prefix(message);
        assert_eq!(
            result,
            "rewrite token validation\n\nMigrate from JWT to Paseto"
        );
    }

    #[test]
    fn test_strip_type_prefix_only_colon() {
        // タイプ部分がConventional Commitsに合致しない
        let result = TestHelper::strip_type_prefix(":");
        assert_eq!(result, ":");
    }

    // ============================================================
    // get_debug_params_for_prefix_mode: 空のrecent_commits
    // ============================================================

    #[test]
    fn test_debug_params_rule_mode_empty_commits() {
        // ルールモードでrecent_commitsが空の場合
        let prefix_mode = PrefixMode::Rule("conventional".to_string());
        let recent_commits: Vec<String> = vec![];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
        assert_eq!(prefix_type, Some("conventional"));
        assert!(commits.is_empty());
    }

    #[test]
    fn test_debug_params_config_mode_squash() {
        // Configモード + squashフラグ: commitsは空になる
        let prefix_mode = PrefixMode::Config("bracket".to_string());
        let recent_commits = vec!["feat: test".to_string()];
        let (prefix_type, commits) =
            App::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, true);
        assert_eq!(prefix_type, Some("bracket"));
        assert!(commits.is_empty());
    }

    // ============================================================
    // extract_conventional_body: 追加境界ケース
    // ============================================================

    #[test]
    fn test_extract_conventional_body_nested_parentheses() {
        // ネストされた括弧: split_once('(') で "a(b))" がスコープとして扱われ、
        // ends_with(')') が true かつ len > 1 なので有効と判定される
        assert_eq!(
            extract_conventional_body("feat(a(b)): nested"),
            Some("nested")
        );
    }

    #[test]
    fn test_extract_conventional_body_empty_after_colon_no_space() {
        // "feat:" のようにコロン後が空の場合
        assert_eq!(extract_conventional_body("feat:"), Some(""));
    }

    #[test]
    fn test_extract_conventional_body_revert_type() {
        // revert タイプも認識される
        assert_eq!(
            extract_conventional_body("revert: undo previous change"),
            Some("undo previous change")
        );
    }

    // ============================================================
    // extract_conventional_body: 数値スコープ・特殊パターン
    // ============================================================

    #[test]
    fn test_extract_conventional_body_numeric_scope() {
        // 数値のみのスコープも有効
        assert_eq!(
            extract_conventional_body("feat(123): add feature"),
            Some("add feature")
        );
    }

    #[test]
    fn test_extract_conventional_body_scope_with_dash() {
        // ハイフンを含むスコープ
        assert_eq!(
            extract_conventional_body("fix(my-module): resolve issue"),
            Some("resolve issue")
        );
    }

    #[test]
    fn test_extract_conventional_body_scope_missing_close_paren() {
        // 閉じ括弧がない場合はNone
        assert_eq!(extract_conventional_body("feat(scope: message"), None);
    }

    #[test]
    fn test_extract_conventional_body_breaking_change_with_scope_and_multiline() {
        // breaking change + スコープ + 複数行
        let msg = "feat(api)!: remove deprecated endpoint\n\n- Remove /v1/users\n- Update docs";
        assert_eq!(
            extract_conventional_body(msg),
            Some("remove deprecated endpoint\n\n- Remove /v1/users\n- Update docs")
        );
    }

    #[test]
    fn test_extract_conventional_body_multiple_colons_in_body() {
        // 本文にコロンが含まれる場合、最初のコロンのみ分割に使用
        assert_eq!(
            extract_conventional_body("fix: resolve config: parsing error"),
            Some("resolve config: parsing error")
        );
    }

    // ============================================================
    // apply_prefix: Unicode・空メッセージのエッジケース
    // ============================================================

    #[test]
    fn test_apply_prefix_unicode_prefix() {
        // 日本語プレフィックス
        let result = TestHelper::apply_prefix("feat: add feature", "【機能】");
        assert_eq!(result, "【機能】add feature");
    }

    #[test]
    fn test_apply_prefix_empty_message() {
        // 空メッセージへのプレフィックス適用
        let result = TestHelper::apply_prefix("", "PREFIX ");
        assert_eq!(result, "PREFIX ");
    }

    #[test]
    fn test_strip_type_prefix_multiline_preserves_body() {
        // 複数行メッセージのstrip: 本文が保持される
        let msg = "feat: title\n\n- detail 1\n- detail 2";
        let result = TestHelper::strip_type_prefix(msg);
        assert_eq!(result, "title\n\n- detail 1\n- detail 2");
    }

    // ============================================================
    // resolve_script_result のテスト
    // ============================================================

    #[test]
    fn test_resolve_script_result_prefix_type_name() {
        // スクリプトが有効な prefix_type 名を返した場合、PrefixMode::Rule に変換
        let result = resolve_script_result(ScriptResult::Prefix("conventional".to_string()));
        assert!(matches!(result, PrefixMode::Rule(ref s) if s == "conventional"));
    }

    #[test]
    fn test_resolve_script_result_prefix_type_with_whitespace() {
        // 前後に空白がある prefix_type 名もトリムして認識
        let result = resolve_script_result(ScriptResult::Prefix("  plain  ".to_string()));
        assert!(matches!(result, PrefixMode::Rule(ref s) if s == "plain"));
    }

    #[test]
    fn test_resolve_script_result_normal_prefix() {
        // 有効な prefix_type 名でない場合、PrefixMode::Script のまま
        let result = resolve_script_result(ScriptResult::Prefix("TICKET-123 ".to_string()));
        assert!(matches!(
            result,
            PrefixMode::Script(ScriptResult::Prefix(_))
        ));
    }

    #[test]
    fn test_resolve_script_result_empty() {
        // Empty はそのまま PrefixMode::Script(Empty) として返る
        let result = resolve_script_result(ScriptResult::Empty);
        assert!(matches!(result, PrefixMode::Script(ScriptResult::Empty)));
    }

    #[test]
    fn test_resolve_script_result_failed() {
        // Failed はそのまま PrefixMode::Script(Failed) として返る
        let result = resolve_script_result(ScriptResult::Failed);
        assert!(matches!(result, PrefixMode::Script(ScriptResult::Failed)));
    }

    // ============================================================
    // extract_conventional_body: 追加のエッジケース
    // ============================================================

    #[test]
    fn test_extract_conventional_body_scope_only_paren() {
        // 空のスコープ feat() は無効
        assert_eq!(extract_conventional_body("feat(): add feature"), None);
    }

    #[test]
    fn test_extract_conventional_body_unclosed_scope() {
        // スコープの閉じ括弧がない場合は無効
        assert_eq!(extract_conventional_body("feat(scope: add feature"), None);
    }

    #[test]
    fn test_extract_conventional_body_colon_only() {
        // コロンだけの場合
        assert_eq!(extract_conventional_body(":"), None);
    }

    // ============================================================
    // extract_conventional_body: 追加エッジケース
    // ============================================================

    #[test]
    fn test_extract_conventional_body_whitespace_only_scope() {
        // スコープが空白のみの場合 → 有効なスコープとして扱われる
        assert_eq!(
            extract_conventional_body("feat( ): add feature"),
            Some("add feature")
        );
    }

    #[test]
    fn test_extract_conventional_body_multibyte_scope() {
        // マルチバイト文字のスコープ
        assert_eq!(
            extract_conventional_body("feat(認証): ログイン修正"),
            Some("ログイン修正")
        );
    }

    #[test]
    fn test_extract_conventional_body_triple_colons_in_body() {
        // 本文にコロンが3つ含まれる場合
        assert_eq!(
            extract_conventional_body("fix: resolve config: parsing: error"),
            Some("resolve config: parsing: error")
        );
    }

    #[test]
    fn test_extract_conventional_body_body_with_many_newlines() {
        // 本文に複数の空行がある場合
        let msg = "feat: add feature\n\nline1\n\nline2\n\nline3";
        assert_eq!(
            extract_conventional_body(msg),
            Some("add feature\n\nline1\n\nline2\n\nline3")
        );
    }

    // ============================================================
    // resolve_script_result: 追加エッジケース
    // ============================================================

    #[test]
    fn test_resolve_script_result_all_valid_types() {
        // 全ての有効な prefix_type が Rule に変換される
        for ty in VALID_PREFIX_TYPES {
            let result = resolve_script_result(ScriptResult::Prefix(ty.to_string()));
            assert!(
                matches!(result, PrefixMode::Rule(_)),
                "'{}' は Rule に変換されるべき",
                ty
            );
        }
    }

    #[test]
    fn test_resolve_script_result_prefix_with_newline() {
        // 改行を含むプレフィックス → trim されても有効な prefix_type なら Rule
        let result = resolve_script_result(ScriptResult::Prefix("conventional\n".to_string()));
        assert!(matches!(result, PrefixMode::Rule(_)));
    }

    #[test]
    fn test_resolve_script_result_empty_string() {
        // 空文字列のプレフィックスは Script のまま
        let result = resolve_script_result(ScriptResult::Prefix(String::new()));
        assert!(matches!(result, PrefixMode::Script(_)));
    }

    // ============================================================
    // strip_type_prefix: 追加エッジケース
    // ============================================================

    #[test]
    fn test_strip_type_prefix_non_conventional() {
        // Conventional Commits 以外のメッセージはそのまま返る
        assert_eq!(
            TestHelper::strip_type_prefix("Update README"),
            "Update README"
        );
    }

    #[test]
    fn test_strip_type_prefix_with_scope_and_breaking() {
        // スコープ + breaking change マーカー付き
        assert_eq!(
            TestHelper::strip_type_prefix("feat(api)!: remove endpoint"),
            "remove endpoint"
        );
    }

    #[test]
    fn test_strip_type_prefix_multiline() {
        // 複数行メッセージのプレフィックス除去
        let msg = "feat: add feature\n\n- detail 1\n- detail 2";
        assert_eq!(
            TestHelper::strip_type_prefix(msg),
            "add feature\n\n- detail 1\n- detail 2"
        );
    }

    // ============================================================
    // apply_prefix: 追加エッジケース
    // ============================================================

    #[test]
    fn test_apply_prefix_to_non_conventional() {
        // Conventional Commits 以外のメッセージにプレフィックスを付加
        assert_eq!(
            TestHelper::apply_prefix("Update README", "TICKET-123 "),
            "TICKET-123 Update README"
        );
    }

    #[test]
    fn test_apply_prefix_empty_prefix() {
        // 空のプレフィックスの場合
        assert_eq!(
            TestHelper::apply_prefix("feat: add feature", ""),
            "add feature"
        );
    }

    #[test]
    fn test_apply_prefix_multiline_message() {
        // 複数行のメッセージにプレフィックスを付加
        let result = TestHelper::apply_prefix("feat: add feature\n\nbody text", "PROJ-1 ");
        assert_eq!(result, "PROJ-1 add feature\n\nbody text");
    }
}
