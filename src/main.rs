// FILE: src/main.rs
// VERSION: 0.1.1
// START_MODULE_CONTRACT
//   PURPOSE: Expose a deployable n0wss executable that forwards process arguments into the governed CLI runtime surface and returns a stable exit code.
//   SCOPE: Binary entrypoint, process-signal shutdown handling, argument forwarding, top-level error rendering, and process exit status selection.
//   DEPENDS: std, tokio, tokio-util, src/cli/mod.rs
//   LINKS: M-CLI-BIN, M-CLI, V-M-CLI-BIN, DF-LIVE-DEPLOY
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   main - forward argv into the library CLI runtime entrypoint and convert result into an OS exit code
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added async runtime execution and signal-aware shutdown so the binary can keep live server and client modes running.
// END_CHANGE_SUMMARY

use std::process::ExitCode;
use tokio_util::sync::CancellationToken;

// START_CONTRACT: main
//   PURPOSE: Run the governed CLI runtime from the process entrypoint and surface a stable shell exit code.
//   INPUTS: { none - consumes process arguments from std::env }
//   OUTPUTS: { ExitCode - success on coordinated runtime shutdown, failure on bootstrap or runtime error }
//   SIDE_EFFECTS: [reads process args, waits on process termination signals, writes one-line error to stderr on failure]
//   LINKS: [M-CLI-BIN, M-CLI, V-M-CLI-BIN]
// END_CONTRACT: main
#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    // START_BLOCK_FORWARD_TO_LIBRARY_CLI
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal(signal_cancel).await;
    });

    match n0wss::cli::run_until_shutdown_from(std::env::args_os(), cancel).await {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("n0wss runtime failed: {error}");
            ExitCode::FAILURE
        }
    }
    // END_BLOCK_FORWARD_TO_LIBRARY_CLI
}

async fn wait_for_shutdown_signal(cancel: CancellationToken) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                cancel.cancel();
                return;
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }

    cancel.cancel();
}
