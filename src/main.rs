mod ai;
mod app;
mod cli;
mod config;
mod error;
mod git;
mod init;
mod notify;
mod state;

use clap::Parser;
use colored::Colorize;

use app::App;
use cli::{Cli, Command};
use error::AppError;
use init::InitCommand;

fn main() {
    let cli = Cli::parse();

    // Handle subcommands first
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
