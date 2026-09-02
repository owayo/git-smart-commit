mod ai;
mod ai_usage;
mod app;
mod cli;
mod config;
mod devlog;
mod error;
mod git;
mod init;
mod notify;
mod state;
#[cfg(test)]
mod test_support;

use clap::Parser;
use colored::Colorize;

use app::App;
use cli::{Cli, Command};
use error::AppError;
use init::InitCommand;

/// git-sc が起動した AI CLI の子孫プロセスであることを示す環境変数
///
/// `build_provider_command` が全プロバイダーへ設定し、AI CLI とその子プロセスに継承される。
pub(crate) const NESTED_ENV_KEY: &str = "GIT_SC_NESTED";

fn main() {
    // AI CLI の中から再帰的に起動された場合は何もしない。
    //
    // git-sc は commit メッセージ生成に AI CLI(claude / codex / agy)を起動するが、
    // それらはコーディングエージェントでもあるため、自身の stop hook を発火させる。
    // hook に git-sc が登録されていると「git-sc → claude → stop hook → git-sc」と
    // 再帰し、生成結果を待たずに *その場でコミットされる*(--dry-run 指定時ですら
    // コミットが発生する)。実測 2026-08-27 (JST): `claude -p` を実行しただけで
    // claw-hooks の stop hook が発火し、ステージ済みの変更が意図せずコミットされた。
    //
    // Codex は `--disable hooks` を渡して個別に塞いでいるが、claude にはその手段が無く
    // (`--bare` は hooks を切れる代わりに OAuth 認証を使えなくする、`--settings` の
    // hooks 上書きは既存設定にマージされて無効)、プロバイダー固有のフラグでは塞ぎきれない。
    // 環境変数は AI CLI からその子プロセスまで確実に継承されるため、ここで一度だけ
    // 見て降りるのが最も確実で、プロバイダーが増えても効き続ける。
    //
    // hook から呼ばれる前提なのでエラーではなく正常終了する(非ゼロで抜けると
    // hook 側がフック失敗として扱う)。
    if std::env::var_os(NESTED_ENV_KEY).is_some_and(|v| !v.is_empty()) {
        return;
    }

    let cli = Cli::parse();

    // サブコマンドを先に処理
    if let Some(ref cmd) = cli.command {
        match cmd {
            Command::Init { force } => match InitCommand::execute(*force) {
                Ok(path) => {
                    println!(
                        "{} Created config file at {}",
                        "Success:".green().bold(),
                        path.display()
                    );
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            },
        }
    }

    let app = match App::new(&cli) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    };

    if let Err(e) = app.run(&cli) {
        // Gitリポジトリでない場合は何も表示せず正常終了
        if matches!(e, AppError::NotGitRepository) {
            std::process::exit(0);
        }
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}
