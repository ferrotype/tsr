//! Private entry only. Production tsrust does not depend on this crate.
use std::{io, sync::mpsc, thread};
use tsr_testhost::{
    framing,
    project_host::{Connection, Input},
};
fn main() -> std::process::ExitCode {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--stdio"] {
        eprintln!("usage: phase5_testserver --stdio");
        return std::process::ExitCode::FAILURE;
    }
    let (output, receive) = mpsc::channel();
    let (connection, input) = Connection::new(output);
    let write_failure = input.clone();
    // The process owns stdin. It may block while the router finishes shutdown;
    // it owns no session roots and is terminated with this private process.
    thread::spawn(move || {
        let mut reader = io::stdin().lock();
        loop {
            match framing::read(&mut reader) {
                Ok(Some(bytes)) => {
                    if input.send(Input::Message(bytes)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = input.send(Input::End(Ok(())));
                    break;
                }
                Err(error) => {
                    let _ = input.send(Input::End(Err(error)));
                    break;
                }
            }
        }
    });
    let writer = thread::spawn(move || -> io::Result<()> {
        let mut writer = io::stdout().lock();
        for message in receive {
            if let Err(error) = framing::write(&mut writer, message.get().as_bytes()) {
                let _ = write_failure.send(Input::End(Err(io::Error::new(
                    error.kind(),
                    error.to_string(),
                ))));
                return Err(error);
            }
        }
        Ok(())
    });
    let result = connection.run();
    let written = writer.join().expect("output writer panicked");
    match result.and(written) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("project test host failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
