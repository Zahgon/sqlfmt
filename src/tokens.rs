//! Tokens: the basic elements of SQL syntax that matter for formatting.
//!
//! Ported from sqlfmt/tokens.py.

use std::fmt;

/// The kinds of token sqlfmt distinguishes. Only a small number of token types matter,
/// the ones that correspond to query structure and therefore formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TokenType {
    FmtOff,
    FmtOn,
    Data,
    JinjaStatement,
    JinjaExpression,
    JinjaBlockStart,
    JinjaBlockEnd,
    JinjaBlockKeyword,
    QuotedName,
    Comment,
    Semicolon,
    StatementStart,
    StatementEnd,
    Star,
    Number,
    BracketOpen,
    BracketClose,
    DoubleColon,
    Colon,
    Operator,
    WordOperator,
    On,
    BooleanOperator,
    Comma,
    Dot,
    Newline,
    UntermKeyword,
    SetOperator,
    Name,
}

impl TokenType {
    /// The enum member's name, as the safety check reports it.
    pub(crate) fn name(self) -> &'static str {
        use TokenType::*;
        match self {
            FmtOff => "FMT_OFF",
            FmtOn => "FMT_ON",
            Data => "DATA",
            JinjaStatement => "JINJA_STATEMENT",
            JinjaExpression => "JINJA_EXPRESSION",
            JinjaBlockStart => "JINJA_BLOCK_START",
            JinjaBlockEnd => "JINJA_BLOCK_END",
            JinjaBlockKeyword => "JINJA_BLOCK_KEYWORD",
            QuotedName => "QUOTED_NAME",
            Comment => "COMMENT",
            Semicolon => "SEMICOLON",
            StatementStart => "STATEMENT_START",
            StatementEnd => "STATEMENT_END",
            Star => "STAR",
            Number => "NUMBER",
            BracketOpen => "BRACKET_OPEN",
            BracketClose => "BRACKET_CLOSE",
            DoubleColon => "DOUBLE_COLON",
            Colon => "COLON",
            Operator => "OPERATOR",
            WordOperator => "WORD_OPERATOR",
            On => "ON",
            BooleanOperator => "BOOLEAN_OPERATOR",
            Comma => "COMMA",
            Dot => "DOT",
            Newline => "NEWLINE",
            UntermKeyword => "UNTERM_KEYWORD",
            SetOperator => "SET_OPERATOR",
            Name => "NAME",
        }
    }

    pub(crate) fn is_jinja_statement(self) -> bool {
        matches!(
            self,
            TokenType::JinjaStatement
                | TokenType::JinjaBlockStart
                | TokenType::JinjaBlockKeyword
                | TokenType::JinjaBlockEnd
        )
    }

    pub(crate) fn is_jinja(self) -> bool {
        self.is_jinja_statement() || self == TokenType::JinjaExpression
    }

    pub(crate) fn is_opening_bracket(self) -> bool {
        matches!(self, TokenType::BracketOpen | TokenType::StatementStart)
    }

    pub(crate) fn divides_queries(self) -> bool {
        matches!(self, TokenType::Semicolon | TokenType::SetOperator)
    }

    pub(crate) fn does_not_set_prev_sql_context(self) -> bool {
        self.is_jinja_statement() || self == TokenType::Newline
    }

    pub(crate) fn is_always_operator(self) -> bool {
        matches!(
            self,
            TokenType::Operator
                | TokenType::WordOperator
                | TokenType::On
                | TokenType::BooleanOperator
                | TokenType::DoubleColon
                | TokenType::Semicolon
        )
    }

    pub(crate) fn is_never_preceded_by_space(self) -> bool {
        matches!(
            self,
            TokenType::BracketClose
                | TokenType::Colon
                | TokenType::DoubleColon
                | TokenType::Comma
                | TokenType::Dot
                | TokenType::Newline
        )
    }

    pub(crate) fn is_preceded_by_space_except_after_open_bracket(self) -> bool {
        matches!(
            self,
            TokenType::UntermKeyword
                | TokenType::StatementStart
                | TokenType::StatementEnd
                | TokenType::WordOperator
                | TokenType::BooleanOperator
                | TokenType::On
        )
    }

    pub(crate) fn is_possible_name(self) -> bool {
        matches!(self, TokenType::QuotedName | TokenType::Name | TokenType::Star | TokenType::JinjaExpression)
    }

    pub(crate) fn is_always_lowercased(self) -> bool {
        matches!(
            self,
            TokenType::UntermKeyword
                | TokenType::BracketOpen
                | TokenType::StatementStart
                | TokenType::StatementEnd
                | TokenType::WordOperator
                | TokenType::On
                | TokenType::BooleanOperator
                | TokenType::SetOperator
                | TokenType::Number
        )
    }

    pub(crate) fn is_equivalent_in_output(self) -> bool {
        !matches!(self, TokenType::Newline | TokenType::Comment)
    }
}

impl fmt::Display for TokenType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenType.{}", self.name())
    }
}

/// A lexed syntactic element. Tokens reference their position in the source query
/// (`spos` is the start of the match including the prefix, `epos` its end; both are
/// byte offsets) and keep the whitespace that preceded them as `prefix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) ttype: TokenType,
    pub(crate) prefix: String,
    pub(crate) token: String,
    pub(crate) spos: usize,
    pub(crate) epos: usize,
}

impl Token {
    pub(crate) fn new(ttype: TokenType, prefix: &str, token: &str, spos: usize, epos: usize) -> Token {
        Token { ttype, prefix: prefix.to_string(), token: token.to_string(), spos, epos }
    }

    /// Builds a token from a regex match: the whole match starts at `pos`, the token
    /// text is capture group 1 and the prefix is the text in between.
    pub(crate) fn from_match(source: &str, caps: &fancy_regex::Captures<'_, str>, ttype: TokenType) -> Token {
        let whole = caps.get(0).expect("match has a group 0");
        let pos = whole.start();
        let (spos, epos) = caps.get(1).map_or((whole.end(), whole.end()), |g| (g.start(), g.end()));
        Token::new(ttype, &source[pos..spos], &source[spos..epos], pos, epos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_type_properties() {
        assert!(TokenType::JinjaBlockKeyword.is_jinja_statement());
        assert!(TokenType::JinjaExpression.is_jinja());
        assert!(!TokenType::JinjaExpression.is_jinja_statement());
        assert!(TokenType::StatementStart.is_opening_bracket());
        assert!(TokenType::SetOperator.divides_queries());
        assert!(!TokenType::Comment.is_equivalent_in_output());
        assert_eq!(TokenType::Name.to_string(), "TokenType.NAME");
    }
}
