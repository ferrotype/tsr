#![allow(dead_code)]
struct Checker;
impl Checker {
    fn check_source_element_worker(&mut self, bad: bool) {
        self.bad_before(bad);
        self.bad_between(bad);
        self.checked_first(bad);
        self.alternatives(bad);
        self.match_alternatives(bad);
        self.nil_guard(None);
        self.unrelated_nil(None, None);
        self.loop_return(bad);
        self.nested_closure(bad);
        self.leaf(bad);
        self.let_some_guard(None);
        self.let_some_ref_guard(None);
        self.unrelated_let_some(None, None);
        let _ = self.custom_err(bad);
        let _ = self.real_err(bad);
        let _ = self.aliased_err(bad);
        self.fake_none(bad);
        self.precise_allowance(bad);
    }
    fn check_expression<T>(&mut self, _: T) {}
    fn check_source_element(&mut self, _: Option<u8>) {}
    fn bad_before(&mut self, bad: bool) {
        if bad {
            return;
        } // expect: bad_before
        self.check_expression(Some(1));
    }
    fn bad_between(&mut self, bad: bool) {
        self.check_expression(Some(1));
        if bad {
            return;
        } // expect: bad_between
        self.check_source_element(Some(2));
    }
    fn checked_first(&mut self, bad: bool) {
        self.check_expression(Some(1));
        if bad {
            return;
        }
    }
    fn alternatives(&mut self, bad: bool) {
        if bad {
            return;
        } else {
            self.check_expression(Some(1));
        }
    }
    fn match_alternatives(&mut self, bad: bool) {
        match bad {
            true => return,
            false => self.check_source_element(Some(1)),
        }
    }
    fn nil_guard(&mut self, child: Option<u8>) {
        if child.is_none() {
            return;
        }
        self.check_expression(child);
    }
    fn unrelated_nil(&mut self, child: Option<u8>, other: Option<u8>) {
        if other.is_none() {
            return;
        } // expect: unrelated_nil
        self.check_expression(child);
    }
    fn loop_return(&mut self, bad: bool) {
        loop {
            self.check_expression(Some(1));
            if bad {
                return;
            } // expect: loop_return
        }
    }
    fn nested_closure(&mut self, bad: bool) {
        let closure = || {
            if bad {
                return;
            }
        };
        closure();
        self.check_expression(Some(1));
    }
    fn leaf(&mut self, bad: bool) {
        if bad {
            return;
        }
    }
    fn let_some_guard(&mut self, child: Option<u8>) {
        let Some(child) = child else {
            return;
        };
        self.check_expression(child);
    }
    fn let_some_ref_guard(&mut self, child: Option<&u8>) {
        let Some(&child) = child else {
            return;
        };
        self.check_expression(child);
    }
    fn unrelated_let_some(&mut self, child: Option<u8>, other: Option<u8>) {
        let Some(_other) = other else {
            return;
        }; // expect: unrelated_let_some
        self.check_expression(child);
    }
    fn custom_err(&mut self, bad: bool) -> u8 {
        if bad {
            return Err(());
        } // expect: custom_err
        self.check_expression(Some(1));
        1
    }
    fn real_err(&mut self, bad: bool) -> Result<(), ()> {
        if bad {
            return std::result::Result::Err(());
        }
        self.check_expression(Some(1));
        Ok(())
    }
    fn aliased_err(&mut self, bad: bool) -> Result<(), ()> {
        use std::result::Result::Err as Failure;
        if bad {
            return Failure(());
        }
        self.check_expression(Some(1));
        Ok(())
    }
    fn fake_none(&mut self, bad: bool) {
        let child = FakeOption(bad);
        if child.is_none() {
            return;
        } // expect: fake_none
        self.check_expression(child);
    }
    fn precise_allowance(&mut self, bad: bool) {
        if bad {
            // checkchildren-allow: deliberate fixture branch; other return remains checked
            return;
        }
        if bad {
            return;
        } // expect: precise_allowance
        self.check_expression(Some(1));
    }
    // Not dispatched: the pinned analyzer does not examine this method.
    fn unrelated_method(&mut self, bad: bool) {
        if bad {
            return;
        }
        self.check_expression(Some(1));
    }
}
#[allow(non_snake_case)]
fn Err(_: ()) -> u8 {
    0
}
struct FakeOption(bool);
impl FakeOption {
    fn is_none(&self) -> bool {
        self.0
    }
}
fn main() {}
