fn main() -> std::process::ExitCode {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--stdio"] {
        eprintln!("usage: ts_testhost --stdio (test-only endpoint; no language service)");
        return std::process::ExitCode::FAILURE;
    }
    match ts_testhost::serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("test-host transport failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
