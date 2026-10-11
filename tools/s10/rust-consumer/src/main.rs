//! Independently linked client: compilation enters only through `tsr_embed`.
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

#[path = "../../../phase7/portable/consumer.rs"]
mod consumer;
#[path = "../../../phase7/portable/protocol.rs"]
mod protocol;

#[path = "../../../phase7/portable/assertions.rs"]
pub mod assertions;
#[path = "../../../phase7/portable/compiler_codec.rs"]
pub mod compiler_codec;
#[path = "../../../phase7/portable/compiler_consumer.rs"]
pub mod compiler_consumer;
#[path = "../../../phase7/portable/compiler_protocol.rs"]
pub mod compiler_protocol;
#[path = "../../../phase7/portable/mapper_codec.rs"]
pub mod mapper_codec;
#[path = "../../../phase7/portable/walker/mod.rs"]
pub mod walker;
pub mod compiler_api {
    pub use tsr_embed::content_mapper as mapper;
    pub use tsr_embed::{Program, QueryError, QueryOperation as Operation, TypeRef};
}
pub mod walker_api {
    pub use crate::compiler_protocol::observation_rows;
    pub use tsr_embed::{
        node_builder_flags as builder_flags, node_builder_internal_flags as builder_internal_flags,
        symbol_format_flags, type_flags as tf, type_format_flags as ff, Program,
        QueryOperation as Operation, TypeRef,
    };
}

fn compiler_stdio() -> Result<(), String> {
    struct Channel {
        input: io::BufReader<io::Stdin>,
        output: io::BufWriter<io::Stdout>,
    }
    impl Channel {
        fn send(&mut self, frame: &compiler_protocol::CompilerOutputFrame) -> Result<(), String> {
            serde_json::to_writer(&mut self.output, frame).map_err(|error| error.to_string())?;
            self.output
                .write_all(b"\n")
                .map_err(|error| error.to_string())?;
            self.output.flush().map_err(|error| error.to_string())
        }
        fn receive<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>, String> {
            let mut line = String::new();
            if self
                .input
                .read_line(&mut line)
                .map_err(|error| error.to_string())?
                == 0
            {
                return Ok(None);
            }
            serde_json::from_str(&line)
                .map(Some)
                .map_err(|error| error.to_string())
        }
    }
    let channel = Arc::new(Mutex::new(Channel {
        input: io::BufReader::new(io::stdin()),
        output: io::BufWriter::new(io::stdout()),
    }));
    let callbacks = channel.clone();
    let mut executor = compiler_consumer::CompilerConsumer::new(Arc::new(move |call| {
        let mut channel = callbacks.lock().map_err(|error| error.to_string())?;
        channel.send(&compiler_protocol::CompilerOutputFrame::Callback(call))?;
        channel
            .receive()?
            .ok_or_else(|| "compiler callback reply ended at EOF".to_owned())
    }));
    loop {
        let request = channel
            .lock()
            .map_err(|error| error.to_string())?
            .receive()?;
        let Some(request) = request else {
            return Ok(());
        };
        match executor.execute(request) {
            Ok(response) => channel
                .lock()
                .map_err(|error| error.to_string())?
                .send(&compiler_protocol::CompilerOutputFrame::Response(response))?,
            Err(error) => {
                channel.lock().map_err(|error| error.to_string())?.send(
                    &compiler_protocol::CompilerOutputFrame::Failure(error.clone()),
                )?;
                return Err(error);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments == ["--compiler"] {
        return std::thread::Builder::new()
            .stack_size(tsr_core::workgroup::RESERVED_STACK)
            .spawn(compiler_stdio)?
            .join()
            .map_err(|_| "public compiler consumer panicked")?
            .map_err(Into::into);
    }
    if !arguments.is_empty() {
        return Err("expected no arguments or --compiler".into());
    }
    let input = io::stdin();
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in input.lock().lines() {
        let request: protocol::TranspileRequest = serde_json::from_str(&line?)?;
        // The same reserved stack as native compilation. A panic propagates to
        // a failed child, never a replacement observation from the host.
        let row = std::thread::Builder::new()
            .stack_size(tsr_core::workgroup::RESERVED_STACK)
            .spawn(move || consumer::execute(&request))?
            .join()
            .map_err(|_| "public transpile consumer panicked")??;
        serde_json::to_writer(&mut output, &row)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
