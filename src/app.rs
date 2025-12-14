use std::io::{self, Write};

use colored::Colorize;

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
    /// 自動判定（過去コミットから推論）
    Auto,
}

/// アプリケーションのメインオーケストレーター
pub struct App {
    git: GitService,
    ai: AiService,
    prefix_scripts: Vec<PrefixScriptConfig>,
    prefix_rules: Vec<PrefixRuleConfig>,
}

impl App {
    /// 新しいAppインスタンスを作成
    pub fn new(cli: &Cli) -> Result<Self, AppError> {
        let config = Config::load()?;
        let mut ai = AiService::from_config(&config);

        // CLIで言語が指定されていれば上書き
        if let Some(ref lang) = cli.language {
            ai.set_language(lang.clone());
        }

        Ok(Self {
            git: GitService::new(),
            ai,
            prefix_scripts: config.prefix_scripts,
            prefix_rules: config.prefix_rules,
        })
    }

    /// プレフィックスモードを判定
    ///
    /// 優先順位:
    /// 1. prefix_scripts: host_patternにマッチすればスクリプト実行
    /// 2. prefix_rules: url_patternに前方一致すればそのprefix_typeを使用
    /// 3. Auto: 上記に該当しなければ過去コミットから自動判定
    fn get_prefix_mode(&self) -> PrefixMode {
        // リモートURLとブランチ名を取得
        let remote_url = match self.git.get_remote_url() {
            Some(url) => url,
            None => return PrefixMode::Auto,
        };
        let branch = self.git.get_current_branch();

        // 1. プレフィックススクリプトをチェック（最優先）
        for script_config in &self.prefix_scripts {
            if remote_url.contains(&script_config.host_pattern) {
                println!(
                    "{}",
                    format!(
                        "Running prefix script for {}...",
                        script_config.host_pattern
                    )
                    .cyan()
                );
                if let Some(branch_name) = &branch {
                    if let Some(result) =
                        self.git
                            .run_prefix_script(&script_config.script, &remote_url, branch_name)
                    {
                        return PrefixMode::Script(result);
                    }
                }
            }
        }

        // 2. プレフィックスルールをチェック（URL前方一致）
        for rule_config in &self.prefix_rules {
            if remote_url.starts_with(&rule_config.url_pattern) {
                println!(
                    "{}",
                    format!(
                        "Using prefix rule for {}: {}",
                        rule_config.url_pattern, rule_config.prefix_type
                    )
                    .cyan()
                );
                return PrefixMode::Rule(rule_config.prefix_type.clone());
            }
        }

        // 3. 該当なし: 自動判定モード
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

    /// メインワークフローを実行
    pub fn run(&self, cli: &Cli) -> Result<(), AppError> {
        // Gitリポジトリかどうかを確認
        self.git.verify_repository()?;

        // AI CLIがインストールされているか確認
        self.ai.verify_installation()?;

        // --amendモードは別処理
        if cli.amend {
            return self.run_amend(cli);
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
        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                // スクリプトモード: 自動判定で生成
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None)?
            }
            PrefixMode::Rule(prefix_type) => {
                // ルールモード: 指定されたprefix_typeで生成
                self.ai
                    .generate_commit_message(&diff, &recent_commits, Some(prefix_type))?
            }
            PrefixMode::Auto => {
                // 自動判定モード: 過去コミットから推論
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None)?
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
        let mut message = match &prefix_mode {
            PrefixMode::Script(_) => {
                self.ai
                    .generate_commit_message(&diff, &recent_commits, None)?
            }
            PrefixMode::Rule(prefix_type) => {
                self.ai
                    .generate_commit_message(&diff, &recent_commits, Some(prefix_type))?
            }
            PrefixMode::Auto => self
                .ai
                .generate_commit_message(&diff, &recent_commits, None)?,
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

    /// コミット確認プロンプトを表示
    fn confirm_commit(&self) -> Result<bool, AppError> {
        self.confirm_prompt("Create this commit? [Y/n] ")
    }

    /// amend確認プロンプトを表示
    fn confirm_amend(&self) -> Result<bool, AppError> {
        self.confirm_prompt("Amend this commit? [Y/n] ")
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
