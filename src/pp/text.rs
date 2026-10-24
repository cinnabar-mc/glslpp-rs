use crate::lexer::{Lexer, TextMode, TokenValue as LexicalValue};
use crate::token::{Location, Token as PublicToken, TokenValue};
use alloc::{borrow::ToOwned, format, string::String, vec::Vec};

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

/// The spelling and value of the last token emitted without trailing trivia.
struct Boundary {
    text: String,
    value: TokenValue,
}

impl Boundary {
    /// Re-lexes a joint, including splicing, to check that its token values survive.
    fn joins(&self, token: &SourceToken) -> bool {
        let joint = format!("{}{}", self.text, token.text);
        let actual = Lexer::new(&joint)
            .filter(|item| !matches!(item, Ok(token) if token.value == LexicalValue::NewLine))
            .map(|item| {
                item.and_then(super::convert_lexer_token)
                    .map(|token| token.value)
            });
        let expected = core::iter::once(&self.value)
            .chain((!token.trivia).then_some(&token.value))
            .cloned()
            .map(Ok);
        !actual.eq(expected)
    }
}

/// Retains token boundaries when writing expanded text.
#[derive(Default)]
pub(crate) struct Output {
    previous: Option<Boundary>,
}

impl Output {
    /// Concatenates properties literally and checks every adjacent GLSL token joint.
    pub(crate) fn render(&mut self, token: SourceToken, mode: TextMode) -> TextToken {
        let separated = mode == TextMode::Glsl
            && !token.text.is_empty()
            && self
                .previous
                .as_ref()
                .is_some_and(|previous| previous.joins(&token));
        if !token.text.is_empty() {
            self.previous = if token.trivia {
                None
            } else {
                Some(Boundary {
                    text: token.text.clone(),
                    value: token.value.clone(),
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
