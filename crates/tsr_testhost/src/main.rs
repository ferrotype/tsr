fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--api") {
        return std::process::ExitCode::from(
            u8::try_from(tsr_testhost::api_witness::run(&args)).unwrap_or(1),
        );
    }
    if args != ["--stdio"] {
        eprintln!(
            "usage: tsr_testhost --stdio (test-only endpoint; no language service) | --api [api flags]"
        );
        return std::process::ExitCode::FAILURE;
    }
    match tsr_testhost::serve(&mut std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("test-host transport failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
