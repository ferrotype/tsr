//! A byte stream split into its read, write and close halves (Go's
//! `io.ReadWriteCloser`, which a reader loop and writers share across
//! goroutines), and the in-memory duplex pipe the in-process content mappers
//! connect through (Go's `net.Pipe`).
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::{Arc, Condvar, Mutex};

/// Closes a stream. Closing unblocks the peer's reads with end of stream.
pub trait Closer: Send + Sync {
    fn close(&self) -> io::Result<()>;
    /// A process's exit code once it has exited (`processExitState`).
    fn exit_code(&self) -> Option<i32> {
        None
    }
}

/// A duplex byte stream.
pub struct Stream {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub closer: Arc<dyn Closer>,
}

#[derive(Default)]
struct PipeState {
    /// Bytes written by end 0 for end 1, and by end 1 for end 0.
    buffers: [VecDeque<u8>; 2],
    closed: [bool; 2],
}

#[derive(Default)]
struct Pipe {
    state: Mutex<PipeState>,
    changed: Condvar,
}

fn closed_pipe() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "io: read/write on closed pipe")
}

struct PipeEnd {
    pipe: Arc<Pipe>,
    end: usize,
}

impl Read for PipeEnd {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut state = self.pipe.state.lock().expect("pipe lock");
        loop {
            if state.closed[self.end] {
                return Err(closed_pipe());
            }
            let incoming = &mut state.buffers[1 - self.end];
            if !incoming.is_empty() {
                let count = out.len().min(incoming.len());
                for (slot, byte) in out.iter_mut().zip(incoming.drain(..count)) {
                    *slot = byte;
                }
                return Ok(count);
            }
            if state.closed[1 - self.end] {
                return Ok(0);
            }
            state = self.pipe.changed.wait(state).expect("pipe lock");
        }
    }
}

impl Write for PipeEnd {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.pipe.state.lock().expect("pipe lock");
        if state.closed[self.end] || state.closed[1 - self.end] {
            return Err(closed_pipe());
        }
        state.buffers[self.end].extend(bytes);
        self.pipe.changed.notify_all();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Closer for PipeEnd {
    fn close(&self) -> io::Result<()> {
        let mut state = self.pipe.state.lock().expect("pipe lock");
        state.closed[self.end] = true;
        self.pipe.changed.notify_all();
        Ok(())
    }
}

/// Two connected in-memory stream ends. What one end writes the other reads;
/// closing an end ends the peer's reads and fails both ends' writes. Unlike
/// `net.Pipe` the pipe buffers, so a write never waits for the peer's read.
pub fn pipe() -> (Stream, Stream) {
    let pipe = Arc::new(Pipe::default());
    let end = |end: usize| Stream {
        reader: Box::new(PipeEnd {
            pipe: pipe.clone(),
            end,
        }),
        writer: Box::new(PipeEnd {
            pipe: pipe.clone(),
            end,
        }),
        closer: Arc::new(PipeEnd {
            pipe: pipe.clone(),
            end,
        }),
    };
    (end(0), end(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_flow_both_ways_and_close_ends_the_peer() {
        let (mut left, mut right) = pipe();
        left.writer.write_all(b"ping").unwrap();
        let mut buffer = [0; 4];
        right.reader.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"ping");
        right.writer.write_all(b"pong").unwrap();
        left.reader.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"pong");
        let reader = std::thread::spawn(move || {
            let mut rest = Vec::new();
            right.reader.read_to_end(&mut rest).unwrap();
            rest
        });
        left.writer.write_all(b"last").unwrap();
        left.closer.close().unwrap();
        assert_eq!(reader.join().unwrap(), b"last");
        assert!(left.writer.write_all(b"x").is_err());
        assert!(left.reader.read(&mut buffer).is_err());
    }
}
