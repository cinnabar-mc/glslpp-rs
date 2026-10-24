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
    /// True when spelling is generated or copied into a macro expansion.
    pub synthesized: bool,
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
            synthesized: true,
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
    previous_end: Option<u32>,
    has_previous: bool,
}

impl Output {
    /// Copies contiguous source text and separates every synthesized boundary.
    pub(crate) fn render(&mut self, token: SourceToken) -> TextToken {
        let separated = self.has_previous
            && !token.text.is_empty()
            && !token.text.starts_with(char::is_whitespace)
            && (token.space
                || token.synthesized
                || self.previous_end != Some(token.location.start));
        if !token.text.is_empty() {
            self.has_previous = !token.trivia;
            self.previous_end = if token.synthesized {
                None
            } else {
                Some(token.location.end)
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
