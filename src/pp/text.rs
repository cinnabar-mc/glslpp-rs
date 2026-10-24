use crate::lexer::TextMode;
use crate::token::{Location, Punct, Token as PublicToken, TokenValue};
use alloc::{borrow::ToOwned, format, string::String, vec::Vec};
use unicode_ident::is_xid_continue;

/// One preprocessed text fragment at its original invocation line.
#[derive(Debug)]
pub struct TextToken {
    pub text: String,
    pub line: u32,
    /// Byte offset of the source invocation in the preprocessor input.
    pub source_offset: u32,
    /// Semantic tokens; comments and whitespace have no token value.
    pub value: Option<TokenValue>,
}

/// A preprocessing token with the spelling needed by text consumers.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct SourceToken {
    pub value: TokenValue,
    pub location: Location,
    pub text: String,
    pub trivia: bool,
    pub opaque: bool,
    /// Zero for source text; otherwise identifies one macro replacement.
    pub fragment: u64,
    pub output_location: Location,
}

impl SourceToken {
    /// Builds a generated token without source whitespace.
    pub(crate) fn generated(value: TokenValue, location: Location, text: String) -> Self {
        Self {
            value,
            location,
            text,
            trivia: false,
            opaque: false,
            fragment: 0,
            output_location: location,
        }
    }

    /// Drops text metadata for the ordinary GLSL token API.
    pub(crate) fn into_public(self) -> PublicToken {
        PublicToken {
            value: self.value,
            location: self.location,
        }
    }
}

/// Strips replacement-edge trivia and optionally collapses internal trivia to one space.
pub(super) fn replacement_tokens(tokens: Vec<SourceToken>, collapse: bool) -> Vec<SourceToken> {
    let mut result = Vec::new();
    for mut token in tokens.into_iter().skip_while(|token| token.trivia) {
        if token.trivia && collapse {
            if result
                .last()
                .is_some_and(|token: &SourceToken| token.trivia)
            {
                continue;
            }
            token.text = " ".to_owned();
        }
        result.push(token);
    }
    while result.last().is_some_and(|token| token.trivia) {
        result.pop();
    }
    result
}

/// The token classes that can absorb text at an expansion joint.
enum Class {
    Ident,
    Number,
    Punct(Punct),
    Other,
}

/// The last emitted token, without retaining or re-lexing its spelling.
struct Boundary {
    class: Class,
    fragment: u64,
    end: u32,
}

impl Boundary {
    /// Reports whether adjacent fragments could form a different token or comment.
    fn joins(&self, token: &SourceToken) -> bool {
        let first = match token.text.chars().next() {
            Some(first) => first,
            None => return false,
        };
        match self.class {
            Class::Ident => is_xid_continue(first),
            Class::Number => is_xid_continue(first) || first == '.',
            Class::Punct(punct) => match punct {
                Punct::LeftAngle => matches!(first, '<' | '='),
                Punct::RightAngle => matches!(first, '>' | '='),
                Punct::Plus => matches!(first, '+' | '='),
                Punct::Minus => matches!(first, '-' | '='),
                Punct::Ampersand => matches!(first, '&' | '='),
                Punct::Pipe => matches!(first, '|' | '='),
                Punct::Caret => matches!(first, '^' | '='),
                Punct::Slash => matches!(first, '/' | '*' | '='),
                Punct::Equal
                | Punct::Bang
                | Punct::Star
                | Punct::Percent
                | Punct::LeftShift
                | Punct::RightShift => first == '=',
                Punct::Dot => first.is_ascii_digit(),
                _ => false,
            },
            Class::Other => false,
        }
    }
}

/// Retains token boundaries when writing expanded text.
#[derive(Default)]
pub(crate) struct Output {
    previous: Option<Boundary>,
}

impl Output {
    /// Concatenates properties literally and separates GLSL only at unsafe fragment joints.
    pub(crate) fn render(&mut self, token: SourceToken, mode: TextMode) -> TextToken {
        let separated = mode == TextMode::Glsl
            && self.previous.as_ref().is_some_and(|previous| {
                (previous.fragment != token.fragment || previous.end != token.location.start)
                    && previous.joins(&token)
            });
        if !token.text.is_empty() {
            self.previous = if token.trivia {
                None
            } else {
                Some(Boundary {
                    class: match token.value {
                        TokenValue::Ident(_) if !token.opaque => Class::Ident,
                        TokenValue::Integer(_) | TokenValue::Float(_) => Class::Number,
                        TokenValue::Punct(punct) => Class::Punct(punct),
                        _ => Class::Other,
                    },
                    fragment: token.fragment,
                    end: token.location.end,
                })
            };
        }
        TextToken {
            text: if separated {
                format!(" {}", token.text)
            } else {
                token.text
            },
            line: token.output_location.line,
            source_offset: token.output_location.start,
            value: if token.trivia {
                None
            } else {
                Some(token.value)
            },
        }
    }
}
