mod ai;
mod app;
mod cli;
mod config;
mod error;
mod git;

use clap::Parser;
use colored::Colorize;

use app::App;
use cli::Cli;

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
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}
