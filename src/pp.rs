use crate::lexer::{self, Token as LexerToken, TokenValue as LexerTokenValue};
use crate::token::*;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    convert::TryFrom,
    rc::Rc,
};

mod directive;
mod if_parser;
use self::directive::DirectiveProcessor;
pub mod text;
use self::text::SourceToken as Token;

#[derive(Clone, PartialEq, Debug)]
struct Define {
    name: String,
    function_like: bool,
    params: HashMap<String, usize>,
    tokens: Vec<Token>,
}

#[derive(Debug)]
struct DefineInvocation {
    define: Rc<Define>,
    define_position: usize,
    location: Location,
    space: bool,
    first: bool,

    parameters: Vec<Vec<Token>>,
    parameter_expanding: usize,
    parameter_position: usize,
}

impl DefineInvocation {
    /// Applies invocation spacing and source lines to a replacement token.
    fn output_token(&mut self, mut token: Token) -> Token {
        token.output_location = self.location;
        if self.first {
            token.space = self.space;
            self.first = false;
        }
        token
    }
}

pub type Step<T> = Result<T, StepExit>;

#[derive(Clone, PartialEq, Debug)]
pub enum StepExit {
    Error((PreprocessorError, Location)),
    Continue,
    Finished,
}

use self::StepExit::Continue;
use self::StepExit::Finished;

impl<T> From<StepExit> for Step<T> {
    fn from(exit: StepExit) -> Step<T> {
        Err(exit)
    }
}

trait MeLexer {
    fn step(&mut self) -> Step<Token>;
    fn get_define(&self, name: &str) -> Option<&Rc<Define>>;
    fn apply_line_offset(&self, line: u32, location: Location) -> Step<u32>;
}

fn make_unexpected_error(token: LexerToken) -> StepExit {
    let error = match token.value {
        LexerTokenValue::Integer(i) => PreprocessorError::UnexpectedToken(TokenValue::Integer(i)),
        LexerTokenValue::Float(f) => PreprocessorError::UnexpectedToken(TokenValue::Float(f)),
        LexerTokenValue::Ident(s) => PreprocessorError::UnexpectedToken(TokenValue::Ident(s)),
        LexerTokenValue::Punct(p) => PreprocessorError::UnexpectedToken(TokenValue::Punct(p)),
        LexerTokenValue::NewLine => PreprocessorError::UnexpectedNewLine,
        LexerTokenValue::Opaque(value) => {
            PreprocessorError::UnexpectedToken(TokenValue::Ident(value))
        }
        LexerTokenValue::Trivia => {
            PreprocessorError::UnexpectedToken(TokenValue::Ident(token.text))
        }
        LexerTokenValue::Hash => PreprocessorError::UnexpectedHash,
    };
    StepExit::Error((error, token.location))
}

fn make_line_overflow_error(location: Location) -> StepExit {
    StepExit::Error((PreprocessorError::LineOverflow, location))
}

/// Carries lexical spelling and spacing through macro expansion.
pub(crate) fn convert_lexer_token(
    token: LexerToken,
) -> Result<Token, (PreprocessorError, Location)> {
    let location = token.location;
    let opaque = matches!(token.value, LexerTokenValue::Opaque(_));
    let trivia = matches!(
        token.value,
        LexerTokenValue::Trivia | LexerTokenValue::NewLine
    );
    let value = match token.value {
        LexerTokenValue::Integer(value) => TokenValue::Integer(value),
        LexerTokenValue::Float(value) => TokenValue::Float(value),
        LexerTokenValue::Ident(value) | LexerTokenValue::Opaque(value) => TokenValue::Ident(value),
        LexerTokenValue::Punct(value) => TokenValue::Punct(value),
        LexerTokenValue::Trivia | LexerTokenValue::NewLine => TokenValue::Ident(token.text.clone()),
        LexerTokenValue::Hash => return Err((PreprocessorError::UnexpectedHash, location)),
    };
    Ok(Token {
        value,
        location,
        output_location: location,
        text: token.text,
        space: token.leading_whitespace,
        trivia,
        opaque,
    })
}

/// Converts a lexical token into the internal preprocessing step type.
fn convert_lexer_token_to_step(token: LexerToken) -> Step<Token> {
    convert_lexer_token(token).map_err(StepExit::Error)
}

fn legal_redefinition(a: &Define, b: &Define) -> bool {
    assert!(a.name == b.name);
    a.function_like == b.function_like
        && a.params == b.params
        && a.tokens.len() == b.tokens.len()
        && a.tokens
            .iter()
            .zip(&b.tokens)
            .all(|(ta, tb)| ta.value == tb.value)
}

#[derive(Default)]
struct MacroProcessor {
    define_invocations: Vec<DefineInvocation>,
    defines_being_expanded: HashSet<String>,

    peeked: Option<Step<Token>>,
    define_line: u32,
}

impl MacroProcessor {
    fn start_define_invocation(
        &mut self,
        name: &str,
        location: Location,
        output_location: Location,
        space: bool,
        lexer: &mut dyn MeLexer,
    ) -> Step<bool> {
        // Defines can be expanding only once, it is not possible to do recursive defines
        if self.defines_being_expanded.contains(name) {
            return Ok(false);
        }

        if let Some(define) = lexer.get_define(name) {
            let mut invocation = DefineInvocation {
                define: define.clone(),
                define_position: 0,
                location: output_location,
                space,
                first: true,

                parameters: Default::default(),
                parameter_position: 0,
                parameter_expanding: usize::MAX,
            };

            // If this is a not a function-like define, __LINE__ inside the define is the line of the first
            // character of the invocation. Only the line of the top-level invocation counts.
            if !self.is_expanding_define() {
                self.define_line = location.line;
            }

            if invocation.define.function_like {
                let lparen_location = match self.step_no_continue(lexer) {
                    Ok(Token {
                        value: TokenValue::Punct(Punct::LeftParen),
                        location,
                        ..
                    }) => location,

                    // Function-like macros are not processed if there is no ( right after the identifier
                    token => {
                        self.peeked = Some(token);
                        return Ok(false);
                    }
                };

                // TODO still bail out if define was undefined until now? This would match
                // clang and GCC
                let (parameters, closing_location) =
                    self.parse_define_call_arguments(lexer, lparen_location)?;

                if !self.is_expanding_define() {
                    self.define_line = closing_location.line;
                }

                // Check for the number of arguments.
                match parameters.len().cmp(&invocation.define.params.len()) {
                    Ordering::Greater => {
                        let params_empty = parameters.len() == 1 && parameters[0].is_empty();
                        let expects_zero_args = invocation.define.params.is_empty();

                        if !(params_empty && expects_zero_args) {
                            return Err(StepExit::Error((
                                PreprocessorError::TooManyDefineArguments,
                                lparen_location,
                            )));
                        }
                    }
                    Ordering::Less => {
                        return Err(StepExit::Error((
                            PreprocessorError::TooFewDefineArguments,
                            lparen_location,
                        )));
                    }
                    _ => {}
                }

                // Fully expand the parameters
                for parameter in parameters {
                    invocation
                        .parameters
                        .push(self.expand_parameter(lexer, parameter)?);
                }
            }

            assert!(self
                .defines_being_expanded
                .insert(invocation.define.name.clone()));
            self.define_invocations.push(invocation);

            return Ok(true);
        }

        Ok(false)
    }

    // Parse the arguments of the function-like define starting after the first (. Also returns
    // the location of the closing ).
    fn parse_define_call_arguments(
        &mut self,
        lexer: &mut dyn MeLexer,
        mut current_location: Location,
    ) -> Step<(Vec<Vec<Token>>, Location)> {
        let mut paren_nesting = 0u32;
        let mut arguments = vec![vec![]];

        loop {
            // Get the next token (without additional expansion)
            let token = match self.step(lexer) {
                Err(StepExit::Continue) => continue,
                Err(StepExit::Finished) => {
                    return Err(StepExit::Error((
                        PreprocessorError::UnexpectedEndOfInput,
                        current_location,
                    )));
                }
                Err(err @ StepExit::Error(_)) => {
                    return Err(err);
                }
                Ok(token) => token,
            };

            current_location = token.location;

            // Handle special tokens
            match token.value {
                // Avoid overflow on parenthesis nesting counting.
                TokenValue::Punct(Punct::LeftParen) => match paren_nesting.checked_add(1) {
                    // TODO figure out a way to cover this code path? Maybe make it a max nesting
                    // so that it can be set to a small value?
                    None => {
                        return Err(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            current_location,
                        )));
                    }
                    Some(v) => paren_nesting = v,
                },

                TokenValue::Punct(Punct::RightParen) => {
                    // Return the arguments when we find our )
                    if paren_nesting == 0 {
                        return Ok((arguments, token.location));
                    }
                    paren_nesting -= 1;
                }

                TokenValue::Punct(Punct::Comma) => {
                    // Commas outside of () split arguments and must not be added to them.
                    if paren_nesting == 0 {
                        arguments.push(Default::default());
                        continue;
                    }
                }

                _ => {}
            }

            arguments.last_mut().unwrap().push(token);
        }
    }

    fn expand_parameter(&self, lexer: &mut dyn MeLexer, parameter: Vec<Token>) -> Step<Vec<Token>> {
        struct ExpandParameterLexer<'a> {
            parent_lexer: &'a dyn MeLexer,
            expander: &'a MacroProcessor,
            tokens: &'a Vec<Token>,
            position: usize,
        }

        impl<'a> MeLexer for ExpandParameterLexer<'a> {
            fn step(&mut self) -> Step<Token> {
                if let Some(token) = self.tokens.get(self.position) {
                    self.position += 1;
                    Ok(token.clone())
                } else {
                    Finished.into()
                }
            }

            fn get_define(&self, name: &str) -> Option<&Rc<Define>> {
                if self.expander.defines_being_expanded.contains(name) {
                    None
                } else {
                    self.parent_lexer.get_define(name)
                }
            }

            fn apply_line_offset(&self, line: u32, _: Location) -> Step<u32> {
                Ok(line)
            }
        }

        let mut parameter_lexer = ExpandParameterLexer {
            parent_lexer: lexer,
            expander: self,
            tokens: &parameter,
            position: 0,
        };

        let mut processor: MacroProcessor = Default::default();
        let mut expanded_parameters = Default::default();
        loop {
            match processor.step(&mut parameter_lexer) {
                Err(err @ StepExit::Error(_)) => return Err(err),
                Err(StepExit::Finished) => return Ok(expanded_parameters),
                Err(StepExit::Continue) => continue,
                Ok(token) => {
                    if let TokenValue::Ident(name) = &token.value {
                        if processor.start_define_invocation(
                            name,
                            token.location,
                            token.output_location,
                            token.space,
                            &mut parameter_lexer,
                        )? {
                            continue;
                        }
                    }

                    expanded_parameters.push(token);
                }
            }
        }
    }

    fn is_expanding_define(&self) -> bool {
        !self.define_invocations.is_empty()
    }

    fn step_internal(&mut self, lexer: &mut dyn MeLexer) -> Step<Token> {
        if let Some(step) = self.peeked.take() {
            return step;
        }

        if let Some(invocation) = self.define_invocations.last_mut() {
            // Keep expanding the parameters
            if let Some(argument) = invocation.parameters.get(invocation.parameter_expanding) {
                if let Some(token) = argument.get(invocation.parameter_position) {
                    invocation.parameter_position += 1;
                    return Ok(invocation.output_token(token.clone()));
                } else {
                    invocation.parameter_expanding = usize::MAX;
                    return Continue.into();
                }
            }

            // Take tokens from the define definition.
            if let Some(token) = invocation.define.tokens.get(invocation.define_position) {
                invocation.define_position += 1;

                // We found a parameter! Start expanding it.
                if let TokenValue::Ident(name) = &token.value {
                    if let Some(parameter_index) = invocation.define.params.get(name) {
                        invocation.parameter_expanding = *parameter_index;
                        invocation.parameter_position = 0;
                        return Continue.into();
                    }
                }

                return Ok(invocation.output_token(token.clone()));
            } else {
                self.defines_being_expanded.remove(&invocation.define.name);
                self.define_invocations.pop();
                return Continue.into();
            }
        }

        lexer.step()
    }

    fn step(&mut self, lexer: &mut dyn MeLexer) -> Step<Token> {
        let token = self.step_internal(lexer)?;

        if let TokenValue::Ident(name) = &token.value {
            if name == "__LINE__" {
                // When inside a define, __LINE__ is that define's line.
                let line = if self.is_expanding_define() {
                    self.define_line
                } else {
                    token.location.line
                };

                let value = lexer.apply_line_offset(line, token.location)? as u64;
                let mut generated = Token::generated(
                    TokenValue::Integer(Integer {
                        value,
                        signed: false,
                        width: 32,
                    }),
                    token.location,
                    value.to_string(),
                );
                generated.output_location = token.output_location;
                generated.space = token.space;
                return Ok(generated);
            }
        }

        Ok(token)
    }

    fn step_no_continue(&mut self, lexer: &mut dyn MeLexer) -> Step<Token> {
        loop {
            let step = self.step(lexer);
            if step != Continue.into() {
                return step;
            }
        }
    }
}

pub struct Preprocessor<'a> {
    directive_processor: DirectiveProcessor<'a>,
    macro_processor: MacroProcessor,
    text_output: text::Output,
}

impl<'a> Preprocessor<'a> {
    pub fn new(input: &'a str) -> Preprocessor<'a> {
        Preprocessor {
            directive_processor: DirectiveProcessor::new(input),
            macro_processor: Default::default(),
            text_output: Default::default(),
        }
    }

    /// Enables Iris conditions: float values truncate toward zero, and `?:` selects a value.
    /// Disabled by default; ordinary source values and `#line` expressions are unchanged.
    pub fn set_iris_conditions(&mut self, allow: bool) {
        self.directive_processor.iris_conditions = allow;
    }

    /// Retains source trivia, quotes and property punctuation for `next_text`.
    /// Set this before adding defines or consuming input. The ordinary GLSL API is unchanged.
    pub fn set_preserve_text(&mut self, preserve: bool) {
        self.directive_processor.preserve_text = preserve;
        self.directive_processor.lexer.set_preserve_text(preserve);
    }

    /// Sets original one-based line numbers for generated input lines before reading tokens.
    /// Token byte offsets stay in the input; `__LINE__` and `#line` use the supplied lines.
    /// Unmapped lines retain their input line number.
    pub fn set_source_lines(&mut self, lines: Vec<u32>) {
        self.directive_processor.source_lines = lines;
    }

    /// Reads one expanded fragment while retaining its source invocation line.
    pub fn next_text(&mut self) -> Option<Result<text::TextToken, (PreprocessorError, Location)>> {
        loop {
            match self.step() {
                Ok(token) => return Some(Ok(self.text_output.render(token))),
                Err(StepExit::Error(error)) => return Some(Err(error)),
                Err(StepExit::Finished) => return None,
                Err(StepExit::Continue) => continue,
            }
        }
    }

    /// Reports whether a hash-prefixed line names a supported preprocessing directive.
    pub fn is_directive(name: &str) -> bool {
        directive::is_directive(name)
    }

    pub fn add_define(
        &mut self,
        name: &str,
        content: &str,
    ) -> Result<(), (PreprocessorError, Location)> {
        self.directive_processor.add_define(name, content)
    }

    fn step(&mut self) -> Step<Token> {
        let token = self.macro_processor.step(&mut self.directive_processor)?;

        if token.opaque || token.trivia {
            return Ok(token);
        }

        // Is this token the start of a new macro?
        if let TokenValue::Ident(name) = &token.value {
            // Returns Continue if it started the define, token otherwise.
            if self.macro_processor.start_define_invocation(
                name,
                token.location,
                token.output_location,
                token.space,
                &mut self.directive_processor,
            )? {
                return Continue.into();
            }
        }

        Ok(token)
    }
}

pub type PreprocessorItem = Result<crate::token::Token, (PreprocessorError, Location)>;

impl<'a> Iterator for Preprocessor<'a> {
    type Item = PreprocessorItem;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.step() {
                Ok(token) => return Some(Ok(token.into_public())),
                Err(StepExit::Error(err)) => return Some(Err(err)),
                Err(StepExit::Finished) => return None,
                Err(StepExit::Continue) => continue,
            };
        }
    }
}
