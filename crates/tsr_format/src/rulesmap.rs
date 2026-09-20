//! The rules map (`format/rulesmap.go`): every rule bucketed by the pair of
//! token kinds it applies to, in a fixed priority order, and the selection of
//! the rules that apply to one pair in one context.

use crate::{
    context::FormattingContext,
    rule::{action, Rule},
    rules::get_all_rules,
    Error,
};
use std::sync::OnceLock;
use tsr_ast::SyntaxKind as K;

const MASK_BIT_SIZE: u32 = 5;
const MASK: u32 = 0b11111;
const MAP_ROW_LENGTH: usize = K::LastToken as usize + 1;

pub(crate) struct RulesMap {
    rules: Vec<Rule>,
    /// Indices into `rules`, per bucket, in priority order.
    buckets: Vec<Vec<u16>>,
}

impl RulesMap {
    pub(crate) fn bucket(&self, index: usize) -> impl Iterator<Item = &Rule> {
        self.buckets[index]
            .iter()
            .map(|&rule| &self.rules[rule as usize])
    }

    pub(crate) fn buckets(&self) -> usize {
        self.buckets.len()
    }
}

// port: tsc/internal/format/rulesmap.go:getRules
pub(crate) fn get_rules(
    context: &mut FormattingContext<'_, '_, '_>,
) -> Result<Vec<&'static Rule>, Error> {
    let index = get_rule_bucket_index(
        context.current_token_span.kind,
        context.next_token_span.kind,
    )?;
    let mut rules = Vec::new();
    let mut mask = action::NONE;
    'rules: for rule in get_rules_map().bucket(index) {
        let accepted = !get_rule_action_exclusion(mask);
        if rule.action & accepted != 0 {
            for predicate in &rule.context {
                if !predicate.test(context)? {
                    continue 'rules;
                }
            }
            rules.push(rule);
            mask |= rule.action;
        }
    }
    Ok(rules)
}

// port: tsc/internal/format/rulesmap.go:getRuleBucketIndex
pub(crate) fn get_rule_bucket_index(row: K, column: K) -> Result<usize, Error> {
    if !(row as u16 <= K::LastKeyword as u16 && column as u16 <= K::LastKeyword as u16) {
        return Err(Error::Assertion(
            "Debug failure. False expression: Must compute formatting context from tokens".into(),
        ));
    }
    Ok(row as usize * MAP_ROW_LENGTH + column as usize)
}

/// For a rule action, the other actions that cannot apply at the same position.
// port: tsc/internal/format/rulesmap.go:getRuleActionExclusion
fn get_rule_action_exclusion(rule_action: u32) -> u32 {
    let mut mask = action::NONE;
    if rule_action & action::STOP_PROCESSING_SPACE_ACTIONS != 0 {
        mask |= action::MODIFY_SPACE_ACTION;
    }
    if rule_action & action::STOP_PROCESSING_TOKEN_ACTIONS != 0 {
        mask |= action::MODIFY_TOKEN_ACTION;
    }
    if rule_action & action::MODIFY_SPACE_ACTION != 0 {
        mask |= action::MODIFY_SPACE_ACTION;
    }
    if rule_action & action::MODIFY_TOKEN_ACTION != 0 {
        mask |= action::MODIFY_TOKEN_ACTION;
    }
    mask
}

/// Upstream's `getRulesMap`, a `sync.OnceValue` over `buildRulesMap`.
pub(crate) fn get_rules_map() -> &'static RulesMap {
    static MAP: OnceLock<RulesMap> = OnceLock::new();
    MAP.get_or_init(build_rules_map)
}

// port: tsc/internal/format/rulesmap.go:buildRulesMap
fn build_rules_map() -> RulesMap {
    let specs = get_all_rules();
    let mut buckets: Vec<Vec<u16>> = vec![Vec::new(); MAP_ROW_LENGTH * MAP_ROW_LENGTH];
    // Used only while the buckets are built.
    let mut construction_state = vec![0u32; buckets.len()];
    let mut rules = Vec::with_capacity(specs.len());
    for (index, spec) in specs.into_iter().enumerate() {
        let specific = spec.left.is_specific && spec.right.is_specific;
        for &left in &spec.left.tokens {
            for &right in &spec.right.tokens {
                let bucket = left as usize * MAP_ROW_LENGTH + right as usize;
                add_rule(
                    &mut buckets[bucket],
                    index as u16,
                    &spec.rule,
                    specific,
                    &mut construction_state[bucket],
                );
            }
        }
        rules.push(spec.rule);
    }
    RulesMap { rules, buckets }
}

/// The sections of a bucket, in order: stop rules for specific tokens, stop
/// rules for any token, context rules for specific tokens, context rules for
/// any token, then the rules without a context, specific before any. The
/// construction state counts the rules of each section in five bits, which
/// gives the index at which to insert the next one.
#[derive(Clone, Copy)]
enum RulesPosition {
    StopRulesSpecific = 0,
    StopRulesAny = MASK_BIT_SIZE as isize,
    ContextRulesSpecific = MASK_BIT_SIZE as isize * 2,
    ContextRulesAny = MASK_BIT_SIZE as isize * 3,
    NoContextRulesSpecific = MASK_BIT_SIZE as isize * 4,
    NoContextRulesAny = MASK_BIT_SIZE as isize * 5,
}

// port: tsc/internal/format/rulesmap.go:addRule
fn add_rule(bucket: &mut Vec<u16>, index: u16, rule: &Rule, specific: bool, state: &mut u32) {
    let position = if rule.action & action::STOP_ACTION != 0 {
        if specific {
            RulesPosition::StopRulesSpecific
        } else {
            RulesPosition::StopRulesAny
        }
    } else if !rule.context.is_empty() {
        if specific {
            RulesPosition::ContextRulesSpecific
        } else {
            RulesPosition::ContextRulesAny
        }
    } else if specific {
        RulesPosition::NoContextRulesSpecific
    } else {
        RulesPosition::NoContextRulesAny
    };
    bucket.insert(get_rule_insertion_index(*state, position), index);
    *state = increase_insertion_index(*state, position);
}

// port: tsc/internal/format/rulesmap.go:getRuleInsertionIndex
fn get_rule_insertion_index(mut bitmap: u32, position: RulesPosition) -> usize {
    let mut index = 0;
    let mut pos = 0;
    while pos <= position as u32 {
        index += bitmap & MASK;
        bitmap >>= MASK_BIT_SIZE;
        pos += MASK_BIT_SIZE;
    }
    index as usize
}

// port: tsc/internal/format/rulesmap.go:increaseInsertionIndex
fn increase_insertion_index(bitmap: u32, position: RulesPosition) -> u32 {
    let position = position as u32;
    let value = ((bitmap >> position) & MASK) + 1;
    assert!(
        value & MASK == value,
        "Adding more rules into the sub-bucket than allowed. Maximum allowed is 32 rules."
    );
    (bitmap & !(MASK << position)) | (value << position)
}
