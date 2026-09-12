use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;

#[test]
fn nested_gates_and_checker_acquisition_are_rejected_before_waiting() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let other_generation = Generation::new(&counters);
    let checker = CheckerIdentity::new(generation.clone(), &counters);
    let sibling = CheckerIdentity::new(generation.clone(), &counters);
    let foreign = CheckerIdentity::new(other_generation.clone(), &counters);
    let gate = generation.enter().unwrap();
    assert_eq!(gate.validate_checker(&checker), Ok(()));
    assert_eq!(gate.validate_checker(&sibling), Ok(()));
    assert_eq!(gate.validate_checker(&foreign), Err(Error::WrongOwner));
    assert!(matches!(generation.enter(), Err(Error::Reentry)));
    assert!(matches!(other_generation.enter(), Err(Error::Reentry)));
    assert!(matches!(checker.lease(), Err(Error::Reentry)));
    assert!(matches!(foreign.lease(), Err(Error::Reentry)));
    drop(gate);
    assert!(checker.lease().is_ok());
    assert!(other_generation.enter().is_ok());
}

#[test]
fn retirement_waits_for_the_committing_gate() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let checker = CheckerIdentity::new(generation.clone(), &counters);
    let order = Arc::new(Mutex::new(Vec::new()));
    let gate = generation.enter().unwrap();
    let (attempted, attempted_rx) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    let retiring = {
        let generation = generation.clone();
        let order = order.clone();
        std::thread::spawn(move || {
            observe_next_retirement_contention(attempted);
            generation.retire();
            order.lock().unwrap().push("retired");
            finished.send(()).unwrap();
        })
    };
    attempted_rx.recv().unwrap();
    assert!(matches!(
        finished_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(gate.validate_checker(&checker), Ok(()));
    order.lock().unwrap().push("committed");
    drop(gate);
    finished_rx.recv().unwrap();
    retiring.join().unwrap();
    assert_eq!(*order.lock().unwrap(), ["committed", "retired"]);
    assert!(matches!(generation.enter(), Err(Error::Retired)));
    assert!(matches!(checker.lease(), Err(Error::Retired)));
}

#[test]
fn retirement_while_holding_the_same_gate_closes_without_relocking() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let checker = CheckerIdentity::new(generation.clone(), &counters);
    let gate = generation.enter().unwrap();
    generation.retire();
    assert_eq!(gate.validate_checker(&checker), Err(Error::Retired));
    drop(gate);
    assert!(matches!(generation.enter(), Err(Error::Retired)));
}

#[test]
fn panic_retires_regardless_of_gate_and_lease_drop_order() {
    for lease_first in [false, true] {
        let counters = Counters::new();
        let generation = Generation::new(&counters);
        let checker = CheckerIdentity::new(generation.clone(), &counters);
        let sibling = CheckerIdentity::new(generation.clone(), &counters);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let lease = checker.lease().unwrap();
            let gate = generation.enter().unwrap();
            if lease_first {
                // Tuple field order, not local declaration order, forces the
                // lease destructor to retire while its gate is still held.
                let _held = (lease, gate);
                panic!("lease first");
            } else {
                let _held = (gate, lease);
                panic!("gate first");
            }
        }));
        assert!(result.is_err());
        assert_eq!(generation.validate(), Err(Error::Retired));
        assert!(matches!(generation.enter(), Err(Error::Retired)));
        assert!(matches!(checker.lease(), Err(Error::Retired)));
        assert!(matches!(sibling.lease(), Err(Error::Retired)));
    }
}

#[test]
fn a_poisoned_gate_never_recovers_as_active() {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let poisoned = catch_unwind(AssertUnwindSafe(|| {
        let _raw_gate = generation.0.gate.lock().unwrap();
        panic!("poison without running GenerationGuard::drop");
    }));
    assert!(poisoned.is_err());
    assert_eq!(generation.validate(), Ok(()));
    assert!(matches!(generation.enter(), Err(Error::Retired)));
    generation.retire();
    assert_eq!(generation.validate(), Err(Error::Retired));
}
