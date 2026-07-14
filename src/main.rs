use std::io::ErrorKind;
use std::process::ExitCode;

use clap::Parser;

use lode::cli::{Cli, Command};
use lode::commands;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        // A cancelled prompt (Esc / Ctrl-C) is a normal exit, not a failure: report it cleanly
        // rather than dumping an error, using the conventional 130 (128 + SIGINT) code.
        Err(err) if is_cancel(&err) => {
            eprintln!("Cancelled — nothing was written.");
            ExitCode::from(130)
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(command: Command) -> anyhow::Result<()> {
    match command {
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

/// Whether an error is a user cancellation from an interactive prompt (Esc / Ctrl-C), which cliclack
/// surfaces as an `io::Error` of kind `Interrupted` somewhere in the chain.
fn is_cancel(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == ErrorKind::Interrupted)
    })
}

#[cfg(test)]
mod tests {
    use super::is_cancel;
    use std::io::{Error, ErrorKind};

    #[test]
    fn detects_an_interrupted_io_error_anywhere_in_the_chain() {
        // cliclack surfaces Esc/Ctrl-C as an Interrupted io error; it may be wrapped in context by
        // the time it reaches main, so the whole chain is searched.
        let err = anyhow::Error::new(Error::new(ErrorKind::Interrupted, "cancelled"))
            .context("while prompting for the loader");
        assert!(is_cancel(&err));
    }

    #[test]
    fn is_false_for_ordinary_errors() {
        assert!(!is_cancel(&anyhow::anyhow!("no lode.json found")));
        let not_found = anyhow::Error::new(Error::new(ErrorKind::NotFound, "missing"));
        assert!(!is_cancel(&not_found));
    }
}
