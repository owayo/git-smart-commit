mod ai;
mod app;
mod cli;
mod config;
mod error;
mod git;
mod state;

use clap::Parser;
use colored::Colorize;

use app::App;
use cli::Cli;
use error::AppError;

fn main() {
    let cli = Cli::parse();

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
