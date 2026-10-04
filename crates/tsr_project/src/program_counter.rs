use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tsr_compiler::Program;

#[derive(Default)]
pub struct ProgramCounter {
    refs: Mutex<HashMap<usize, i32>>,
}
impl ProgramCounter {
    // port: tsc/internal/project/programcounter.go:programCounter.Ref
    pub fn retain(self: &Arc<Self>, program: Arc<Program>) -> ProgramReference {
        let key = Arc::as_ptr(&program) as usize;
        let mut refs = self.refs.lock().expect("program counter");
        let count = refs.entry(key).or_default();
        *count = count
            .checked_add(1)
            .expect("program reference count overflow");
        ProgramReference {
            counter: self.clone(),
            program,
        }
    }
    pub fn len(&self) -> usize {
        self.refs.lock().expect("program counter").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
pub struct ProgramReference {
    counter: Arc<ProgramCounter>,
    program: Arc<Program>,
}
impl Drop for ProgramReference {
    // port: tsc/internal/project/programcounter.go:programCounter.Deref
    fn drop(&mut self) {
        let key = Arc::as_ptr(&self.program) as usize;
        let mut refs = self.counter.refs.lock().expect("program counter");
        let count = refs.get_mut(&key).expect("live program reference");
        *count -= 1;
        assert!(*count >= 0, "program reference count went below zero");
        if *count == 0 {
            refs.remove(&key);
        }
        // Program's own cache leases release at the last program reference;
        // this count tracks snapshot membership, independent of escaped roots.
    }
}
