use std::io::{self, Write};

use colored::Colorize;

use crate::ai::AiService;
use crate::cli::Cli;
use crate::config::Config;
use crate::error::AppError;
use crate::git::GitService;

/// アプリケーションのメインオーケストレーター
pub struct App {
    git: GitService,
    ai: AiService,
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
        })
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

        // diffを取得（デフォルトはステージ済み、-uフラグでアンステージも含む）
        let staged_diff = self.git.get_staged_diff()?;
        let (diff, needs_staging) = if !staged_diff.trim().is_empty() {
            (staged_diff, false)
        } else if cli.include_unstaged {
            // -uフラグ: アンステージの変更にフォールバック
            let unstaged_diff = self.git.get_unstaged_diff()?;
            if unstaged_diff.trim().is_empty() {
                return Err(AppError::NoChanges);
            }
            println!(
                "{}",
                "No staged changes. Using unstaged changes for message generation.".yellow()
            );
            (unstaged_diff, true)
        } else {
            // デフォルト: ステージ済みのみ
            return Err(AppError::NoStagedChanges);
        };

        // フォーマット検出用に直近のコミットを取得
        let recent_commits = self.git.get_recent_commits(2)?;

        // 参照用に直近のコミットを表示
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

        // コミットメッセージを生成（AIがフォーマットを決定）
        println!("{}", "Generating commit message...".cyan());
        let message = self.ai.generate_commit_message(&diff, &recent_commits)?;

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
            // アンステージだった場合はステージング
            if needs_staging {
                println!("{}", "Staging changes...".cyan());
                self.git.stage_all()?;
            }
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

        // フォーマット検出用に直近のコミットを取得（amendするコミットはスキップ）
        let recent_commits = self.git.get_recent_commits(3)?;
        let recent_commits: Vec<String> = recent_commits.into_iter().skip(1).collect();

        // 参照用に直近のコミットを表示
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

        // コミットメッセージを生成
        println!("{}", "Generating commit message...".cyan());
        let message = self.ai.generate_commit_message(&diff, &recent_commits)?;

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
