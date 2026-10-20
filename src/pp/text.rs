use crate::lexer::{Lexer, TokenValue as LexerValue};
use crate::token::{Location, Token as PublicToken, TokenValue};

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
    pub space: bool,
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
            space: false,
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

/// Retains token boundaries when writing expanded text.
#[derive(Default)]
pub(crate) struct Output {
    previous: Option<String>,
}

impl Output {
    /// Emits original trivia and adds spacing only where expansion requires it.
    pub(crate) fn render(&mut self, token: SourceToken) -> TextToken {
        let separated = self
            .previous
            .as_ref()
            .is_some_and(|previous| token.space || joins_tokens(previous, &token.text));
        self.previous = if token.trivia || token.text.is_empty() {
            None
        } else {
            Some(token.text.clone())
        };
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

/// Reports whether adjacent spellings would become a single preprocessing token.
fn joins_tokens(left: &str, right: &str) -> bool {
    let text = format!("{}{}", left, right);
    let mut lexer = Lexer::new(&text);
    lexer.set_preserve_text(true);
    let mut tokens = lexer.filter_map(|token| match token {
        Ok(token) if !matches!(token.value, LexerValue::Trivia | LexerValue::NewLine) => {
            Some(token)
        }
        _ => None,
    });
    tokens
        .next()
        .is_some_and(|token| token.location.end as usize > left.len())
}
