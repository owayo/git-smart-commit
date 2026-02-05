use std::io::{self, Write};

use colored::Colorize;
use regex::Regex;

use crate::ai::AiService;
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

/// prefix_type が有効かどうかを検証
fn is_valid_prefix_type(prefix_type: &str) -> bool {
    VALID_PREFIX_TYPES.contains(&prefix_type)
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

        Ok(Self {
            git: GitService::new(),
            ai,
            prefix_scripts: config.prefix_scripts.clone(),
            prefix_rules: config.prefix_rules.clone(),
            prefix_type: config.prefix_type.clone(),
            auto_push: config.auto_push,
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
        println!("  models.gemini: {}", config.models.gemini);
        println!("  models.codex: {}", config.models.codex);
        println!("  models.claude: {}", config.models.claude);
        println!("  prefix_type: {:?}", config.prefix_type);
        println!("  auto_push: {:?}", config.auto_push);
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
    fn get_prefix_mode(&self) -> PrefixMode {
        self.get_prefix_mode_internal(false)
    }

    /// サイレントモードでプレフィックスモードを判定（進捗出力なし）
    fn get_prefix_mode_silent(&self) -> PrefixMode {
        self.get_prefix_mode_internal(true)
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
            if let Ok(re) = Regex::new(&script_config.url_pattern) {
                if re.is_match(&remote_url) {
                    if !silent {
                        println!(
                            "{}",
                            format!("Running prefix script for {}...", script_config.url_pattern)
                                .cyan()
                        );
                    }
                    if let Some(branch_name) = &branch {
                        let result = self.git.run_prefix_script(
                            &script_config.script,
                            &remote_url,
                            branch_name,
                        );

                        // スクリプト実行結果を出力
                        if !silent {
                            match &result {
                                Some(ScriptResult::Prefix(prefix)) => {
                                    println!(
                                        "{}",
                                        format!("  → prefix: {:?}", prefix.trim()).green()
                                    );
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
                            return PrefixMode::Script(r);
                        }
                    }
                }
            }
        }

        // 2. プレフィックスルールをチェック（正規表現マッチ）
        for rule_config in &self.prefix_rules {
            if let Ok(re) = Regex::new(&rule_config.url_pattern) {
                if re.is_match(&remote_url) {
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
        // Conventional Commits形式（type: message）の場合、typeを削除してprefixに置き換え
        if let Some(colon_pos) = message.find(':') {
            let body = message[colon_pos + 1..].trim_start();
            format!("{}{}", prefix, body)
        } else {
            // コロンがない場合はそのまま結合
            format!("{}{}", prefix, message)
        }
    }

    /// コミットメッセージから型プレフィックスを削除（本文のみ取得）
    fn strip_type_prefix(&self, message: &str) -> String {
        if let Some(colon_pos) = message.find(':') {
            message[colon_pos + 1..].trim_start().to_string()
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
    ) {
        let prompt = AiService::build_prompt(
            diff,
            recent_commits,
            self.ai.language(),
            prefix_type,
            with_body,
        );
        println!();
        println!("{}", "=== DEBUG: AI Prompt ===".yellow().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", prompt);
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", "=== END DEBUG ===".yellow().bold());
        println!();
    }

    /// デバッグモード時にPrefixModeに基づいてプロンプトを表示
    fn debug_print_for_prefix_mode(
        &self,
        diff: &str,
        recent_commits: &[String],
        prefix_mode: &PrefixMode,
        is_squash: bool,
        with_body: bool,
    ) {
        let (prefix_type, commits) =
            Self::get_debug_params_for_prefix_mode(prefix_mode, recent_commits, is_squash);
        self.print_debug_prompt(diff, commits, prefix_type, with_body);
    }

    /// メインワークフローを実行
    pub fn run(&self, cli: &Cli) -> Result<(), AppError> {
        // Gitリポジトリかどうかを確認
        self.git.verify_repository()?;

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

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
            return self.run_generate_for(cli);
        }

        // --rewordモードは別処理
        if cli.reword.is_some() {
            return self.run_reword(cli);
        }

        // --amendモードは別処理
        if cli.amend {
            return self.run_amend(cli);
        }

        // --squashモードは別処理
        if cli.squash.is_some() {
            return self.run_squash(cli);
        }

        // --allフラグがあれば全変更をステージング
        if cli.stage_all {
            println!("{}", "Staging all changes...".cyan());
            self.git.stage_all()?;
        }

        // ステージ済みのdiffを取得
        let staged_diff = self.git.get_staged_diff()?;
        let diff = if !staged_diff.trim().is_empty() {
            staged_diff
        } else if cli.stage_all {
            // --allフラグ指定時で変更がない場合は正常終了
            println!("{}", "変更がありません。".cyan());
            return Ok(());
        } else {
            // デフォルト: ステージ済みのみ
            return Err(AppError::NoStagedChanges);
        };

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode();

        // フォーマット検出用に直近のコミットを取得（Autoモードの場合のみ表示）
        let recent_commits = self.git.get_recent_commits(5)?;

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            if recent_commits.is_empty() {
                println!(
                    "{} {}",
                    "No recent commits found.".cyan(),
                    "Using Conventional Commits format.".yellow()
                );
            } else {
                println!("{}", "Recent commits (for format reference):".cyan());
                for commit in &recent_commits {
                    println!("  {}", commit.dimmed());
                }
            }
        }

        // コミットメッセージを生成
        println!("{}", "Generating commit message...".cyan());

        // デバッグモード: プロンプトを表示
        if cli.debug {
            self.debug_print_for_prefix_mode(
                &diff,
                &recent_commits,
                &prefix_mode,
                false,
                cli.with_body,
            );
        }

        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: プレフィックスなしで生成（後でスクリプトのプレフィックスを適用）
                self.ai
                    .generate_commit_message(&diff, &[], Some("plain"), cli.with_body)?
            }
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai.generate_commit_message(
                    &diff,
                    &recent_commits,
                    Some(prefix_type),
                    cli.with_body,
                )?
            }
            PrefixMode::Auto => {
                // 自動判定モード: 過去コミットから推論
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None, cli.with_body)?
            }
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                    println!("{}", format!("Applied prefix: {}", prefix.trim()).cyan());
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                    println!("{}", "No prefix applied (script returned empty).".cyan());
                }
                ScriptResult::Failed => {
                    // AI生成のメッセージをそのまま使用
                    println!("{}", "Using AI-generated format.".cyan());
                }
            }
        }

        // 生成されたメッセージを表示
        println!();
        println!("{}", "Generated commit message:".green().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", message);
        println!("{}", "─".repeat(50).dimmed());
        println!();

        // ドライランモードの処理
        if cli.dry_run {
            println!("{}", "Dry run mode - no commit was made.".yellow());
            return Ok(());
        }

        // 確認してコミット
        if cli.auto_confirm || self.confirm_commit()? {
            self.git.commit(&message)?;
            println!("{}", "✓ Commit created successfully!".green().bold());

            // auto-push が有効な場合は push も実行
            if self.git.is_auto_push_enabled(self.auto_push) {
                self.git.push()?;
                println!("{}", "✓ Pushed to remote successfully!".green().bold());
            }
        } else {
            println!("{}", "Commit cancelled.".yellow());
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// amendワークフローを実行
    fn run_amend(&self, cli: &Cli) -> Result<(), AppError> {
        println!(
            "{}",
            "Amend mode: regenerating message for last commit...".cyan()
        );

        // 直前のコミットのdiffを取得
        let diff = self.git.get_last_commit_diff()?;
        if diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode();

        // フォーマット検出用に直近のコミットを取得（amendするコミットはスキップ）
        let recent_commits = self.git.get_recent_commits(6)?;
        let recent_commits: Vec<String> = recent_commits.into_iter().skip(1).collect();

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            if recent_commits.is_empty() {
                println!(
                    "{} {}",
                    "No recent commits found.".cyan(),
                    "Using Conventional Commits format.".yellow()
                );
            } else {
                println!("{}", "Recent commits (for format reference):".cyan());
                for commit in &recent_commits {
                    println!("  {}", commit.dimmed());
                }
            }
        }

        // コミットメッセージを生成
        println!("{}", "Generating commit message...".cyan());

        // デバッグモード: プロンプトを表示
        if cli.debug {
            self.debug_print_for_prefix_mode(
                &diff,
                &recent_commits,
                &prefix_mode,
                false,
                cli.with_body,
            );
        }

        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: プレフィックスなしで生成（後でスクリプトのプレフィックスを適用）
                self.ai
                    .generate_commit_message(&diff, &[], Some("plain"), cli.with_body)?
            }
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai.generate_commit_message(
                    &diff,
                    &recent_commits,
                    Some(prefix_type),
                    cli.with_body,
                )?
            }
            PrefixMode::Auto => {
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None, cli.with_body)?
            }
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                    println!("{}", format!("Applied prefix: {}", prefix.trim()).cyan());
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                    println!("{}", "No prefix applied (script returned empty).".cyan());
                }
                ScriptResult::Failed => {
                    // AI生成のメッセージをそのまま使用
                    println!("{}", "Using AI-generated format.".cyan());
                }
            }
        }

        // 生成されたメッセージを表示
        println!();
        println!("{}", "Generated commit message:".green().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", message);
        println!("{}", "─".repeat(50).dimmed());
        println!();

        // ドライランモードの処理
        if cli.dry_run {
            println!("{}", "Dry run mode - commit was not amended.".yellow());
            return Ok(());
        }

        // 確認してamend
        if cli.auto_confirm || self.confirm_amend()? {
            self.git.amend_commit(&message)?;
            println!("{}", "✓ Commit amended successfully!".green().bold());
        } else {
            println!("{}", "Amend cancelled.".yellow());
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// squashワークフローを実行
    fn run_squash(&self, cli: &Cli) -> Result<(), AppError> {
        // ベースブランチを取得（必須）
        let base_branch = cli.squash.as_ref().ok_or(AppError::NoBaseBranch)?;

        // ベースブランチの存在確認
        if !self.git.branch_exists(base_branch) {
            return Err(AppError::GitError(format!(
                "Base branch '{}' does not exist",
                base_branch
            )));
        }

        println!("{}", "Squash mode: combining commits into one...".cyan());

        // 現在のブランチを取得
        let current_branch = self
            .git
            .get_current_branch()
            .ok_or_else(|| AppError::GitError("Failed to get current branch".to_string()))?;

        // ベースブランチ上にいる場合はエラー
        if current_branch == *base_branch {
            return Err(AppError::OnBaseBranch);
        }

        println!(
            "{}",
            format!(
                "Base branch: {} → Current branch: {}",
                base_branch, current_branch
            )
            .cyan()
        );

        // merge-baseを取得
        let merge_base = self.git.get_merge_base(base_branch, "HEAD")?;

        // コミット数を確認
        let commit_count = self.git.count_commits_from_base(&merge_base)?;
        if commit_count == 0 {
            return Err(AppError::NoCommitsToSquash);
        }

        println!("{}", format!("Commits to squash: {}", commit_count).cyan());

        // ベースからの差分を取得
        let diff = self.git.get_diff_from_base(&merge_base)?;
        if diff.trim().is_empty() {
            return Err(AppError::NoChanges);
        }

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode();

        // コミットメッセージを生成（差分のみから、過去コミットは参照しない）
        println!("{}", "Generating commit message...".cyan());

        // デバッグモード: プロンプトを表示
        if cli.debug {
            self.debug_print_for_prefix_mode(&diff, &[], &prefix_mode, true, cli.with_body);
        }

        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: プレフィックスなしで生成
                self.ai
                    .generate_commit_message(&diff, &[], Some("plain"), cli.with_body)?
            }
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai
                    .generate_commit_message(&diff, &[], Some(prefix_type), cli.with_body)?
            }
            PrefixMode::Auto => {
                // 自動判定モード: Conventional Commits形式で生成
                self.ai
                    .generate_commit_message(&diff, &[], Some("conventional"), cli.with_body)?
            }
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                    println!("{}", format!("Applied prefix: {}", prefix.trim()).cyan());
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                    println!("{}", "No prefix applied (script returned empty).".cyan());
                }
                ScriptResult::Failed => {
                    println!("{}", "Using AI-generated format.".cyan());
                }
            }
        }

        // 生成されたメッセージを表示
        println!();
        println!("{}", "Generated commit message:".green().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", message);
        println!("{}", "─".repeat(50).dimmed());
        println!();

        // ドライランモードの処理
        if cli.dry_run {
            println!("{}", "Dry run mode - no squash was performed.".yellow());
            return Ok(());
        }

        // 確認してsquash実行
        if cli.auto_confirm || self.confirm_squash(commit_count)? {
            // soft resetしてコミット
            self.git.soft_reset_to(&merge_base)?;
            self.git.commit(&message)?;
            println!(
                "{}",
                format!("✓ {} commits squashed successfully!", commit_count)
                    .green()
                    .bold()
            );

            // auto-push が有効な場合は push も実行
            if self.git.is_auto_push_enabled(self.auto_push) {
                self.git.push()?;
                println!("{}", "✓ Pushed to remote successfully!".green().bold());
            }
        } else {
            println!("{}", "Squash cancelled.".yellow());
            return Err(AppError::UserCancelled);
        }

        Ok(())
    }

    /// generate-forワークフローを実行（標準出力にメッセージのみ出力）
    fn run_generate_for(&self, cli: &Cli) -> Result<(), AppError> {
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

        // プレフィックスモードを判定（サイレントモード）
        let prefix_mode = self.get_prefix_mode_silent();

        // フォーマット検出用に直近のコミットを取得
        let recent_commits = self.git.get_recent_commits(5)?;

        // デバッグモード: プロンプトを標準エラー出力に表示（標準出力はメッセージのみ）
        if cli.debug {
            eprintln!();
            let (prefix_type, commits) =
                Self::get_debug_params_for_prefix_mode(&prefix_mode, &recent_commits, false);
            let prompt = AiService::build_prompt(
                &combined_diff,
                commits,
                self.ai.language(),
                prefix_type,
                cli.with_body,
            );
            eprintln!("{}", "=== DEBUG: AI Prompt ===".yellow().bold());
            eprintln!("{}", "─".repeat(50).dimmed());
            eprintln!("{}", prompt);
            eprintln!("{}", "─".repeat(50).dimmed());
            eprintln!("{}", "=== END DEBUG ===".yellow().bold());
            eprintln!();
        }

        // コミットメッセージを生成（サイレントモード）
        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => self.ai.generate_commit_message_silent(
                &combined_diff,
                &[],
                Some("plain"),
                cli.with_body,
            )?,
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai.generate_commit_message_silent(
                    &combined_diff,
                    &recent_commits,
                    Some(prefix_type),
                    cli.with_body,
                )?
            }
            PrefixMode::Auto => self.ai.generate_commit_message_silent(
                &combined_diff,
                &recent_commits,
                None,
                cli.with_body,
            )?,
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                }
                ScriptResult::Failed => {
                    // AI生成のメッセージをそのまま使用
                }
            }
        }

        // 標準出力にメッセージのみを出力（余計な装飾なし）
        println!("{}", message);

        Ok(())
    }

    /// rewordワークフローを実行
    fn run_reword(&self, cli: &Cli) -> Result<(), AppError> {
        let hash = cli
            .reword
            .as_ref()
            .ok_or(AppError::InvalidRewordTarget)?
            .clone();

        // 短いハッシュを取得して表示用に使用
        let short_hash = if hash.len() > 7 { &hash[..7] } else { &hash };

        println!(
            "{}",
            format!(
                "Reword mode: regenerating message for commit {}...",
                short_hash
            )
            .cyan()
        );

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
        println!("{}", "Current commit message:".cyan());
        println!("  {}", current_message.dimmed());

        // プレフィックスモードを判定
        let prefix_mode = self.get_prefix_mode();

        // フォーマット検出用に直近のコミットを取得（対象コミットより新しいものを除く）
        let recent_commits = self.git.get_recent_commits(5 + n)?;
        let recent_commits: Vec<String> = recent_commits.into_iter().skip(n).collect();

        // Autoモードの場合のみ参照用に直近のコミットを表示
        if matches!(prefix_mode, PrefixMode::Auto) {
            if recent_commits.is_empty() {
                println!(
                    "{} {}",
                    "No recent commits found.".cyan(),
                    "Using Conventional Commits format.".yellow()
                );
            } else {
                println!("{}", "Recent commits (for format reference):".cyan());
                for commit in &recent_commits {
                    println!("  {}", commit.dimmed());
                }
            }
        }

        // コミットメッセージを生成
        println!("{}", "Generating commit message...".cyan());

        // デバッグモード: プロンプトを表示
        if cli.debug {
            self.debug_print_for_prefix_mode(
                &diff,
                &recent_commits,
                &prefix_mode,
                false,
                cli.with_body,
            );
        }

        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: プレフィックスなしで生成
                self.ai
                    .generate_commit_message(&diff, &[], Some("plain"), cli.with_body)?
            }
            PrefixMode::Rule(prefix_type) | PrefixMode::Config(prefix_type) => {
                // ルール/設定モード: 指定されたprefix_typeで生成
                self.ai.generate_commit_message(
                    &diff,
                    &recent_commits,
                    Some(prefix_type),
                    cli.with_body,
                )?
            }
            PrefixMode::Auto => {
                // 自動判定モード: 過去コミットから推論
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None, cli.with_body)?
            }
        };

        // スクリプトモードの場合はメッセージを加工
        if let PrefixMode::Script(result) = prefix_mode {
            match result {
                ScriptResult::Prefix(prefix) => {
                    message = self.apply_prefix(&message, &prefix);
                    println!("{}", format!("Applied prefix: {}", prefix.trim()).cyan());
                }
                ScriptResult::Empty => {
                    message = self.strip_type_prefix(&message);
                    println!("{}", "No prefix applied (script returned empty).".cyan());
                }
                ScriptResult::Failed => {
                    println!("{}", "Using AI-generated format.".cyan());
                }
            }
        }

        // 生成されたメッセージを表示
        println!();
        println!("{}", "Generated commit message:".green().bold());
        println!("{}", "─".repeat(50).dimmed());
        println!("{}", message);
        println!("{}", "─".repeat(50).dimmed());
        println!();

        // ドライランモードの処理
        if cli.dry_run {
            println!("{}", "Dry run mode - commit was not reworded.".yellow());
            return Ok(());
        }

        // 確認してreword実行
        if cli.auto_confirm || self.confirm_reword(short_hash)? {
            self.git.reword_commit_by_hash(&hash, &message)?;
            println!(
                "{}",
                format!("✓ Commit {} reworded successfully!", short_hash)
                    .green()
                    .bold()
            );
            println!(
                "{}",
                "Note: You may need to force push (git push --force) if already pushed.".yellow()
            );
        } else {
            println!("{}", "Reword cancelled.".yellow());
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
            if let Some(colon_pos) = message.find(':') {
                let body = message[colon_pos + 1..].trim_start();
                format!("{}{}", prefix, body)
            } else {
                format!("{}{}", prefix, message)
            }
        }

        /// strip_type_prefixのテスト用ラッパー
        fn strip_type_prefix(message: &str) -> String {
            if let Some(colon_pos) = message.find(':') {
                message[colon_pos + 1..].trim_start().to_string()
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

    // ============================================================
    // strip_type_prefix のテスト
    // ============================================================

    #[rstest]
    #[case("feat: add new feature", "add new feature")]
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
}
