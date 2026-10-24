use crate::token::{Integer, PreprocessorError, Punct};

use super::{Define, Location, MacroProcessor, MeLexer, Step, StepExit, Token, TokenValue};
use std::{cell::Cell, collections::HashMap, convert::TryInto, rc::Rc, vec};

/// Maximum recursive expression nesting before returning an error.
const MAX_EXPRESSION_DEPTH: usize = 64;

struct IfLexer<'macros> {
    tokens: vec::IntoIter<Token>,
    defines: &'macros HashMap<String, Rc<Define>>,
    work: &'macros Cell<u64>,
}

pub(super) struct IfParser<'macros> {
    lexer: IfLexer<'macros>,
    macro_processor: MacroProcessor,
    location: Location,

    parsing_if: bool,
    iris_conditions: bool,
    carry: Option<Token>,
    expression_depth: usize,
}

impl<'macros> IfParser<'macros> {
    /// Builds a new IfParser that can be reused
    ///
    /// `parsing_if` indicates wether or not non defined macros should be
    /// replaced with 0. `iris_conditions` enables float truncation and ternaries.
    pub fn new(
        tokens: Vec<Token>,
        defines: &'macros HashMap<String, Rc<Define>>,
        work: &'macros Cell<u64>,
        location: Location,
        parsing_if: bool,
        iris_conditions: bool,
    ) -> Self {
        IfParser {
            lexer: IfLexer {
                tokens: tokens.into_iter(),
                defines,
                work,
            },
            macro_processor: MacroProcessor::default(),
            location,

            parsing_if,
            iris_conditions,
            carry: None,
            expression_depth: 0,
        }
    }

    /// Reads a validated token without starting a new macro invocation.
    /// All expression tokens, including unexpanded `defined` operands, pass through here.
    fn unexpanded_next(&mut self) -> Step<Option<Token>> {
        let token = if let Some(token) = self.carry.take() {
            token
        } else {
            loop {
                match self.macro_processor.step(&mut self.lexer) {
                    Ok(token) if token.trivia => continue,
                    Ok(token) => break token,
                    Err(StepExit::Continue) => continue,
                    Err(StepExit::Finished) => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
        };
        if token.opaque {
            return Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(token.value),
                token.location,
            )));
        }
        Ok(Some(token))
    }

    /// Helper method to consume the next token with define expansion
    fn next(&mut self) -> Step<Option<Token>> {
        let token = match self.unexpanded_next()? {
            Some(t) => t,
            None => return Ok(None),
        };
        Ok(match token.value {
            TokenValue::Ident(ref name) if name != "defined" => {
                match self.add_define(name, token.location)? {
                    Some(t) => Some(t),
                    None => self.next()?,
                }
            }
            _ => Some(token),
        })
    }

    /// Helper method to get the next token with define expansion
    pub fn peek(&mut self) -> Step<Option<Token>> {
        self.carry = self.next()?;
        Ok(self.carry.clone())
    }

    /// Helper method to consume the next token without define expansion
    ///
    /// Returns an EOI error if there are no further tokens
    fn expect_unexpanded_next(&mut self) -> Step<Token> {
        self.unexpanded_next()?.ok_or(StepExit::Error((
            PreprocessorError::UnexpectedEndOfInput,
            self.location,
        )))
    }

    /// Helper method to consume the next token with define expansion
    ///
    /// Returns an EOI error if there are no further tokens
    fn expect_next(&mut self) -> Step<Token> {
        self.next()?.ok_or(StepExit::Error((
            PreprocessorError::UnexpectedEndOfInput,
            self.location,
        )))
    }

    /// Helper method to get the next token with define expansion
    ///
    /// Returns an EOI error if there are no further tokens
    fn expect_peek(&mut self) -> Step<Token> {
        self.peek()?.ok_or(StepExit::Error((
            PreprocessorError::UnexpectedEndOfInput,
            self.location,
        )))
    }

    fn add_define(&mut self, name: &str, location: Location) -> Step<Option<Token>> {
        if self.macro_processor.start_define_invocation(
            name,
            location,
            location,
            &mut self.lexer,
        )? {
            Ok(None)
        } else if self.parsing_if {
            Ok(Some(Token::generated(
                TokenValue::Integer(Integer {
                    value: 0,
                    signed: true,
                    width: 64,
                }),
                location,
                "0".to_owned(),
            )))
        } else {
            Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(TokenValue::Ident(name.to_string())),
                location,
            )))
        }
    }

    fn handle_defined(&mut self) -> Step<i64> {
        let next = self.expect_unexpanded_next()?;

        match next.value {
            TokenValue::Ident(ref name) => Ok(self.lexer.defines.get(name).is_some() as i64),
            TokenValue::Punct(Punct::LeftParen) => {
                let name_token = self.expect_unexpanded_next()?;
                let name = match name_token.value {
                    TokenValue::Ident(name) => Ok(name),
                    value => Err(StepExit::Error((
                        PreprocessorError::UnexpectedToken(value),
                        name_token.location,
                    ))),
                }?;

                let close_brace = self.expect_next()?;

                match close_brace.value {
                    TokenValue::Punct(Punct::RightParen) => {
                        Ok(self.lexer.defines.get(&name).is_some() as i64)
                    }
                    value => Err(StepExit::Error((
                        PreprocessorError::UnexpectedToken(value),
                        close_brace.location,
                    ))),
                }
            }
            value => Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(value),
                next.location,
            ))),
        }
    }

    fn parse_atom(&mut self) -> Step<i64> {
        let token = self.expect_next()?;

        match token.value {
            TokenValue::Ident(name) => {
                debug_assert_eq!(name, "defined");

                self.handle_defined()
            }
            TokenValue::Integer(int) => Ok(int.value as i64),
            TokenValue::Float(float) if self.iris_conditions => Ok(float.value as i64),
            TokenValue::Punct(Punct::LeftParen) => {
                let val = self.nested(Self::parse_conditional)?;

                let close_brace = self.expect_next()?;

                match close_brace.value {
                    TokenValue::Punct(Punct::RightParen) => Ok(val),
                    value => Err(StepExit::Error((
                        PreprocessorError::UnexpectedToken(value),
                        close_brace.location,
                    ))),
                }
            }
            value => Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(value),
                token.location,
            ))),
        }
    }

    fn parse_unary(&mut self) -> Step<i64> {
        match self.expect_peek()?.value {
            TokenValue::Punct(punct) => match punct {
                Punct::Plus | Punct::Minus | Punct::Bang | Punct::Tilde => {
                    self.next()?;

                    let val = self.nested(Self::parse_unary)?;

                    Ok(match punct {
                        Punct::Plus => val,
                        Punct::Minus => val.checked_neg().ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?,
                        Punct::Bang => (val == 0) as i64,
                        Punct::Tilde => !val,
                        _ => unreachable!(),
                    })
                }
                _ => self.parse_atom(),
            },
            _ => self.parse_atom(),
        }
    }

    fn parse_multiplicative(&mut self) -> Step<i64> {
        let mut left = self.parse_unary()?;

        while let Some(TokenValue::Punct(punct)) = self.peek()?.map(|t| t.value) {
            if let Punct::Star | Punct::Slash | Punct::Percent = punct {
                self.next()?;

                let right = self.parse_unary()?;

                match punct {
                    Punct::Star => {
                        left = left.checked_mul(right).ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?
                    }
                    Punct::Slash => {
                        left = left.checked_div(right).ok_or(StepExit::Error((
                            PreprocessorError::DivisionByZero,
                            self.location,
                        )))?
                    }
                    Punct::Percent => {
                        left = left.checked_rem(right).ok_or(StepExit::Error((
                            PreprocessorError::DivisionByZero,
                            self.location,
                        )))?
                    }
                    _ => unreachable!(),
                }
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_additive(&mut self) -> Step<i64> {
        let mut left = self.parse_multiplicative()?;

        while let Some(TokenValue::Punct(punct)) = self.peek()?.map(|t| t.value) {
            if let Punct::Plus | Punct::Minus = punct {
                self.next()?;

                let right = self.parse_multiplicative()?;

                match punct {
                    Punct::Plus => {
                        left = left.checked_add(right).ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?
                    }
                    Punct::Minus => {
                        left = left.checked_sub(right).ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?
                    }
                    _ => unreachable!(),
                }
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_shift(&mut self) -> Step<i64> {
        let mut left = self.parse_additive()?;

        while let Some(TokenValue::Punct(punct)) = self.peek()?.map(|t| t.value) {
            if let Punct::LeftShift | Punct::RightShift = punct {
                self.next()?;

                let right = self.parse_additive()?;

                match punct {
                    Punct::LeftShift => {
                        let right = right.try_into().map_err(|_| {
                            StepExit::Error((PreprocessorError::IntegerOverflow, self.location))
                        })?;
                        left = left.checked_shl(right).ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?
                    }
                    Punct::RightShift => {
                        let right = right.try_into().map_err(|_| {
                            StepExit::Error((PreprocessorError::IntegerOverflow, self.location))
                        })?;
                        left = left.checked_shr(right).ok_or(StepExit::Error((
                            PreprocessorError::IntegerOverflow,
                            self.location,
                        )))?
                    }
                    _ => unreachable!(),
                }
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_comparative(&mut self) -> Step<i64> {
        let mut left = self.parse_shift()?;

        while let Some(TokenValue::Punct(punct)) = self.peek()?.map(|t| t.value) {
            if let Punct::LeftAngle | Punct::RightAngle | Punct::LessEqual | Punct::GreaterEqual =
                punct
            {
                self.next()?;

                let right = self.parse_shift()?;

                match punct {
                    Punct::LeftAngle => left = (left < right) as i64,
                    Punct::RightAngle => left = (left > right) as i64,
                    Punct::LessEqual => left = (left <= right) as i64,
                    Punct::GreaterEqual => left = (left >= right) as i64,
                    _ => unreachable!(),
                }
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_equality(&mut self) -> Step<i64> {
        let mut left = self.parse_comparative()?;

        while let Some(TokenValue::Punct(punct)) = self.peek()?.map(|t| t.value) {
            if let Punct::EqualEqual | Punct::NotEqual = punct {
                self.next()?;

                let right = self.parse_comparative()?;

                match punct {
                    Punct::EqualEqual => left = (left == right) as i64,
                    Punct::NotEqual => left = (left != right) as i64,
                    _ => unreachable!(),
                }
            } else {
                break;
            }
        }

        Ok(left)
    }

    fn parse_bit_and(&mut self) -> Step<i64> {
        let mut left = self.parse_equality()?;

        while let Some(TokenValue::Punct(Punct::Ampersand)) = self.peek()?.map(|t| t.value) {
            self.next()?;

            let right = self.parse_equality()?;

            left &= right
        }

        Ok(left)
    }

    fn parse_bit_xor(&mut self) -> Step<i64> {
        let mut left = self.parse_bit_and()?;

        while let Some(TokenValue::Punct(Punct::Caret)) = self.peek()?.map(|t| t.value) {
            self.next()?;

            let right = self.parse_bit_and()?;

            left ^= right
        }

        Ok(left)
    }

    fn parse_bit_or(&mut self) -> Step<i64> {
        let mut left = self.parse_bit_xor()?;

        while let Some(TokenValue::Punct(Punct::Pipe)) = self.peek()?.map(|t| t.value) {
            self.next()?;

            let right = self.parse_bit_xor()?;

            left |= right
        }

        Ok(left)
    }

    fn parse_logical_and(&mut self) -> Step<i64> {
        let mut left = self.parse_bit_or()?;

        while let Some(TokenValue::Punct(Punct::LogicalAnd)) = self.peek()?.map(|t| t.value) {
            self.next()?;

            let right = self.parse_bit_or()?;

            left = (left != 0 && right != 0) as i64;
        }

        Ok(left)
    }

    fn parse_logical_or(&mut self) -> Step<i64> {
        let mut left = self.parse_logical_and()?;

        while let Some(TokenValue::Punct(Punct::LogicalOr)) = self.peek()?.map(|t| t.value) {
            self.next()?;

            let right = self.parse_logical_and()?;

            left = (left != 0 || right != 0) as i64;
        }

        Ok(left)
    }

    /// Parses one nested expression without exceeding the stack-depth budget.
    fn nested(&mut self, parse: fn(&mut Self) -> Step<i64>) -> Step<i64> {
        if self.expression_depth >= MAX_EXPRESSION_DEPTH {
            return Err(StepExit::Error((
                PreprocessorError::RecursionLimitReached,
                self.location,
            )));
        }
        self.expression_depth += 1;
        let result = parse(self);
        self.expression_depth -= 1;
        result
    }

    /// Parses the low-precedence, right-associative Iris conditional operator.
    fn parse_conditional(&mut self) -> Step<i64> {
        let condition = self.parse_logical_or()?;
        if !self.iris_conditions
            || !matches!(
                self.peek()?.map(|token| token.value),
                Some(TokenValue::Punct(Punct::Question))
            )
        {
            return Ok(condition);
        }
        self.next()?;
        let when_true = self.nested(Self::parse_conditional)?;
        let colon = self.expect_next()?;
        if colon.value != TokenValue::Punct(Punct::Colon) {
            return Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(colon.value),
                colon.location,
            )));
        }
        let when_false = self.nested(Self::parse_conditional)?;
        Ok(if condition != 0 {
            when_true
        } else {
            when_false
        })
    }

    /// Evaluates one preprocessor expression using the configured condition dialect.
    pub fn evaluate_expression(&mut self) -> Step<i64> {
        self.parse_conditional()
    }
}

impl<'macros> MeLexer for IfLexer<'macros> {
    fn step(&mut self) -> Step<Token> {
        self.tokens.next().ok_or(StepExit::Finished)
    }

    fn get_define(&self, name: &str) -> Option<&Rc<Define>> {
        self.defines.get(name)
    }

    fn apply_line_offset(&self, line: u32, _: Location) -> Step<u32> {
        Ok(line)
    }

    fn charge(&self, location: Location) -> Step<()> {
        super::charge_work(self.work, location)
    }
}
