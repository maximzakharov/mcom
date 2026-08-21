mod ansi;
mod cli;
mod input;
mod logfile;
mod logo;
mod ports;
mod scrollback;
mod serial;
mod session;
mod term;
mod timestamps;
mod tui;

use clap::Parser;

use crate::cli::Cli;

fn main() {
    let cli = Cli::parse();

    if cli.list {
        ports::print_list();
        return;
    }

    if let Err(e) = session::run(cli) {
        // The terminal is already restored by TermGuard's Drop at this point.
        eprintln!("mcom: {e:#}");
        std::process::exit(1);
    }
}
