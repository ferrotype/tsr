//! `core.Stack`.
//!
//! Ports of `tsc/internal/core/stack.go`, witnessed by the `core` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).

/// Go's `Stack[T]`.
#[derive(Clone, Debug, Default)]
pub struct Stack<T> {
    data: Vec<T>,
}

impl<T> Stack<T> {
    /// port: tsc/internal/core/stack.go:Stack.Push
    pub fn push(&mut self, item: T) {
        self.data.push(item);
    }

    /// Go's `Peek`: the top item, panicking on an empty stack.
    pub fn peek(&self) -> &T {
        &self.data[self.peek_index()]
    }

    /// The index `Peek` reads, carrying its empty check.
    /// port: tsc/internal/core/stack.go:Stack.Peek
    fn peek_index(&self) -> usize {
        let l = self.data.len();
        if l == 0 {
            panic!("stack is empty");
        }
        l - 1
    }

    /// Go's `Pop`: the top item, removed, panicking on an empty stack.
    pub fn pop(&mut self) -> T {
        let index = self.pop_index();
        self.data.remove(index)
    }

    /// The index `Pop` removes, carrying its empty check.
    /// port: tsc/internal/core/stack.go:Stack.Pop
    fn pop_index(&self) -> usize {
        let l = self.data.len();
        if l == 0 {
            panic!("stack is empty");
        }
        l - 1
    }

    /// port: tsc/internal/core/stack.go:Stack.Len
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}
