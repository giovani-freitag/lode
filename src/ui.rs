//! Terminal UX helpers shared across commands: interactivity detection and clack-style spinners
//! for network-bound work. Kept in one place so every command frames prompts and progress the
//! same way, and so the non-interactive contract (no prompts under `--yes` or when piped) is
//! decided once rather than re-derived per command.

use std::io::IsTerminal;

use anyhow::Result;

/// Whether interactive prompts should run. A prompt is only shown when the user did not pass
/// `--yes` *and* stdin is a real terminal — piping or redirecting input must never block on a
/// widget, mirroring how `npm`/`cargo` degrade to non-interactive behavior.
pub fn is_interactive(yes: bool) -> bool {
    !yes && std::io::stdin().is_terminal()
}

/// Whether progress spinners should animate. Off whenever stderr isn't a terminal (CI, pipes,
/// redirects) so captured output stays plain text instead of carrying spinner control sequences.
fn spinners_enabled() -> bool {
    std::io::stderr().is_terminal()
}

/// Run a network-bound step under a clack-style spinner, stopping it on success and switching it to
/// the error state on failure — so a spinner is never left dangling and the underlying error still
/// propagates cleanly. When spinners are disabled (non-TTY) the step simply runs, keeping piped
/// output untouched.
pub fn spin<T>(active: &str, done: &str, step: impl FnOnce() -> Result<T>) -> Result<T> {
    if !spinners_enabled() {
        return step();
    }
    let spinner = cliclack::spinner();
    spinner.start(active);
    match step() {
        Ok(value) => {
            spinner.stop(done);
            Ok(value)
        }
        Err(err) => {
            spinner.error(format!("{active} failed"));
            Err(err)
        }
    }
}
