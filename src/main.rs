// FILE: src/main.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Expose a deployable n0wss executable that forwards process arguments into the governed CLI bootstrap surface and returns a stable exit code.
//   SCOPE: Binary entrypoint, argument forwarding, top-level error rendering, and process exit status selection.
//   DEPENDS: std, src/cli/mod.rs
//   LINKS: M-CLI-BIN, M-CLI, V-M-CLI-BIN, DF-LIVE-DEPLOY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   main - forward argv into the library CLI entrypoint and convert result into an OS exit code
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.0 - Added a thin binary wrapper so release builds produce a deployable n0wss executable for live staging.
// END_CHANGE_SUMMARY

use std::process::ExitCode;

// START_CONTRACT: main
//   PURPOSE: Run the governed CLI bootstrap from the process entrypoint and surface a stable shell exit code.
//   INPUTS: { none - consumes process arguments from std::env }
//   OUTPUTS: { ExitCode - success on initialized startup, failure on bootstrap error }
//   SIDE_EFFECTS: [reads process args, writes one-line error to stderr on failure]
//   LINKS: [M-CLI-BIN, M-CLI, V-M-CLI-BIN]
// END_CONTRACT: main
fn main() -> ExitCode {
    // START_BLOCK_FORWARD_TO_LIBRARY_CLI
    match n0wss::cli::run_from(std::env::args_os()) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("n0wss startup failed: {error}");
            ExitCode::FAILURE
        }
    }
    // END_BLOCK_FORWARD_TO_LIBRARY_CLI
}
