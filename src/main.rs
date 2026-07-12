use anyhow::Result;
use clap::Parser;

use lode::cli::{Cli, Command};
use lode::commands;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => commands::init::run(args),
        Command::Import(args) => commands::import::run(args),
        Command::Add(args) => commands::add::run(args),
        Command::Del(args) => commands::del::run(args),
        Command::Pin(args) => commands::pin::pin(args),
        Command::Unpin(args) => commands::pin::unpin(args),
        Command::Update(args) => commands::update::run(args),
        Command::List(args) => commands::list::run(args),
        Command::Refresh => commands::refresh::run(),
        Command::Install(args) => commands::install::run(args),
        Command::Get(args) => commands::get::run(args),
        Command::Publish(args) => commands::publish::run(args),
        Command::Export(args) => commands::export::run(args),
        Command::Bundle(args) => commands::bundle::run(args),
        Command::Verify(args) => commands::verify::run(args),
        Command::Why(args) => commands::why::run(args),
        Command::Config(args) => commands::config::run(args),
    }
}
