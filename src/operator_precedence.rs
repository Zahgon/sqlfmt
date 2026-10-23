//! Operator precedence tiers used when merging runs of operators.
//!
//! Ported from sqlfmt/operator_precedence.py.

use std::sync::OnceLock;

use regex::Regex;

use crate::error::Fatal;
use crate::node::{Arena, NodeId};
use crate::pyre;
use crate::tokens::TokenType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OperatorPrecedence {
    DoubleColon = 0,
    As = 1,
    SquareBrackets = 2,
    OtherTight = 3,
    Exponent = 4,
    Multiplication = 5,
    Addition = 6,
    Other = 7,
    Membership = 8,
    Comparators = 9,
    Presence = 10,
    BoolNot = 11,
    BoolAnd = 12,
    BoolOr = 13,
    On = 14,
}

impl OperatorPrecedence {
    /// The tiers at which runs of operators are merged, loosest last.
    pub(crate) fn tiers() -> Vec<OperatorPrecedence> {
        use OperatorPrecedence::*;
        vec![OtherTight, Exponent, Multiplication, Other, Comparators, BoolNot, On]
    }

    pub(crate) fn from_node(arena: &Arena, id: NodeId) -> Result<OperatorPrecedence, Fatal> {
        if !arena.is_operator(id) {
            return Err(Fatal::new(format!(
                "AssertionError: Internal error! {} is not an operator. Please open an issue",
                arena.get(id).text()
            )));
        }
        let node = arena.get(id);
        Ok(match node.token.ttype {
            TokenType::DoubleColon => OperatorPrecedence::DoubleColon,
            TokenType::BracketOpen => OperatorPrecedence::SquareBrackets,
            TokenType::On => OperatorPrecedence::On,
            TokenType::Star => OperatorPrecedence::Multiplication,
            TokenType::BooleanOperator => Self::from_boolean(arena, id),
            TokenType::Operator => Self::from_operator(&node.value),
            TokenType::WordOperator => Self::from_word_operator(&node.value),
            _ => OperatorPrecedence::Other,
        })
    }

    fn from_operator(value: &str) -> OperatorPrecedence {
        use OperatorPrecedence::*;
        match value {
            "+" | "-" => Addition,
            "/" | "%" | "%%" => Multiplication,
            "^" => Exponent,
            "=" | "==" | "!=" | "<>" | "<=" | ">=" | ">" | "<" => Comparators,
            "~" | "~*" | "!~" | "!~*" | "~~" | "~~*" | "!~~" | "!~~*" => Membership,
            _ => Other,
        }
    }

    fn from_word_operator(value: &str) -> OperatorPrecedence {
        static TABLE: OnceLock<Vec<(OperatorPrecedence, Vec<Regex>)>> = OnceLock::new();
        let table = TABLE.get_or_init(|| {
            let compile = |patterns: &[&str]| -> Vec<Regex> {
                patterns.iter().map(|p| pyre::compile_plain(&format!("{p}$"), "i")).collect()
            };
            vec![
                (OperatorPrecedence::As, compile(&[r"as"])),
                (
                    OperatorPrecedence::OtherTight,
                    compile(&[r"exclude", r"replace", r"except", r"over", r"within\s+group", r"filter", r"using"]),
                ),
                (
                    OperatorPrecedence::Membership,
                    compile(&[
                        r"(not\s+)?between",
                        r"(not\s+)?in",
                        r"(not\s+)?i?like(\s+any)?",
                        r"(not\s+)?similar\s+to",
                        r"(not\s+)?rlike",
                        r"(not\s+)?regexp",
                    ]),
                ),
                (OperatorPrecedence::Presence, compile(&[r"is(\s+not)?", r"isnull", r"notnull"])),
            ]
        });
        for (precedence, programs) in table {
            if programs.iter().any(|p| p.is_match(value)) {
                return *precedence;
            }
        }
        OperatorPrecedence::Other
    }

    fn from_boolean(arena: &Arena, id: NodeId) -> OperatorPrecedence {
        if arena.is_the_and_after_the_between_operator(id) {
            return OperatorPrecedence::OtherTight;
        }
        match arena.get(id).value.as_str() {
            "or" => OperatorPrecedence::BoolOr,
            "not" => OperatorPrecedence::BoolNot,
            _ => OperatorPrecedence::BoolAnd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_ordered() {
        let tiers = OperatorPrecedence::tiers();
        assert_eq!(tiers.first(), Some(&OperatorPrecedence::OtherTight));
        assert_eq!(tiers.last(), Some(&OperatorPrecedence::On));
        assert!(tiers.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn word_operators_have_tiers() {
        assert_eq!(OperatorPrecedence::from_word_operator("as"), OperatorPrecedence::As);
        assert_eq!(OperatorPrecedence::from_word_operator("not  in"), OperatorPrecedence::Membership);
        assert_eq!(OperatorPrecedence::from_word_operator("global not in"), OperatorPrecedence::Other);
        assert_eq!(OperatorPrecedence::from_word_operator("is not"), OperatorPrecedence::Presence);
    }
}
