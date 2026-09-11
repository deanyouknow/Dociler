use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

use dociler_core::diagnostics::Diagnostics;

const HELP: &str = "Dociler — local-first document assistant (development build)

Usage: dociler [COMMAND]

Commands:
  doctor        Show basic workspace and platform diagnostics
  help          Show this help

Options:
  -h, --help    Show this help
  -V, --version Show the build version

This build contains the CLI foundation. Chat, document reading, model loading,
and the API server are not available yet. See IMPLEMENTATION_PLAN.md.
";

enum Command {
    Help,
    Version,
    Doctor,
}

fn parse(args: &[OsString]) -> Option<Command> {
    match args {
        [] => Some(Command::Help),
        [arg] if arg == "help" || arg == "--help" || arg == "-h" => Some(Command::Help),
        [arg] if arg == "--version" || arg == "-V" => Some(Command::Version),
        [arg] if arg == "doctor" => Some(Command::Doctor),
        [command, flag] if command == "doctor" && (flag == "--help" || flag == "-h") => {
            Some(Command::Help)
        }
        _ => None,
    }
}

fn execute(command: Command, output: &mut impl Write) -> io::Result<()> {
    match command {
        Command::Help => write!(output, "{HELP}"),
        Command::Version => writeln!(output, "dociler {}", env!("CARGO_PKG_VERSION")),
        Command::Doctor => {
            let report = Diagnostics::inspect(&std::env::current_dir()?)?;
            writeln!(
                output,
                "Dociler {} — basic diagnostics",
                env!("CARGO_PKG_VERSION")
            )?;
            // Debug formatting escapes terminal controls in an untrusted path.
            writeln!(output, "Workspace: {:?}", report.workspace)?;
            writeln!(
                output,
                "Platform: {} ({})",
                report.operating_system, report.architecture
            )?;
            writeln!(output, "Workspace access: read-only")?;
            writeln!(output, "Session storage: none")?;
            writeln!(
                output,
                "Chat, document parsing, and inference: not implemented"
            )?;
            writeln!(output, "Memory/GPU readiness: not assessed")?;
            writeln!(output, "API server: not implemented (no listening ports)")
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(command) = parse(&args) else {
        // Do not echo arbitrary arguments: they may contain credentials or controls.
        let _ = writeln!(
            io::stderr().lock(),
            "dociler: unsupported arguments; use 'dociler --help'."
        );
        return ExitCode::from(2);
    };

    match execute(command, &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(_) => {
            let _ = writeln!(
                io::stderr().lock(),
                "dociler: command failed; check workspace access and output destination."
            );
            ExitCode::FAILURE
        }
    }
}
