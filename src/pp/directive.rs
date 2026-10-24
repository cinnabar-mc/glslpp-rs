use super::*;

/// Declares directive spellings with the variants used by the dispatch match.
macro_rules! directive_names {
    ($($kind:ident => $name:literal),+ $(,)?) => {
        enum DirectiveName { $($kind),+ }
        impl DirectiveName {
            /// Resolves a supported preprocessing directive name.
            fn parse(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$kind),)+ _ => None }
            }
        }
    };
}

directive_names! {
    Error => "error",
    Line => "line",
    Define => "define",
    Undef => "undef",
    If => "if",
    Ifdef => "ifdef",
    Ifndef => "ifndef",
    Elif => "elif",
    Else => "else",
    Endif => "endif",
    Version => "version",
    Extension => "extension",
    Pragma => "pragma",
}

/// Recognizes names using the same declaration as directive dispatch.
pub(super) fn is_directive(name: &str) -> bool {
    DirectiveName::parse(name).is_some()
}

struct DirectiveBlock {
    start_location: Location,
    had_valid_segment: bool,
    had_else: bool,
    outer_skipped: bool,
}

pub(super) struct DirectiveProcessor<'a> {
    pub(super) lexer: lexer::Lexer<'a>,
    defines: HashMap<String, Rc<Define>>,
    pub(super) work: Cell<u64>,
    skipping: bool,
    blocks: Vec<DirectiveBlock>,
    line_offset: i64,
    pub(super) source_lines: Vec<u32>,
    had_directive: bool,
    had_non_directive_token: bool,
    pub(super) iris_conditions: bool,
    pub(super) iris_recovery: bool,
    pub(super) preserve_text: bool,
    comments: VecDeque<Token>,
}

impl<'a> DirectiveProcessor<'a> {
    pub(super) fn new(input: &'a str) -> DirectiveProcessor<'a> {
        DirectiveProcessor {
            lexer: lexer::Lexer::new(input),
            defines: Default::default(),
            work: Cell::new(u64::MAX),
            skipping: false,
            blocks: Default::default(),
            line_offset: 0,
            source_lines: Vec::new(),
            had_directive: false,
            had_non_directive_token: false,
            iris_conditions: false,
            iris_recovery: false,
            preserve_text: false,
            comments: Default::default(),
        }
    }

    /// Reads one lexical token, including preserved trivia.
    fn get_raw_lexer_token(&mut self) -> Step<LexerToken> {
        self.lexer.set_skipping(self.skipping);
        match self.lexer.next() {
            None => Finished.into(),
            Some(Ok(mut tok)) => {
                if let Some(line) = tok
                    .location
                    .line
                    .checked_sub(1)
                    .and_then(|line| self.source_lines.get(line as usize))
                {
                    tok.location.line = *line;
                }
                Ok(tok)
            }
            Some(Err(err)) => Err(StepExit::Error(err)),
        }
    }

    /// Reads a directive token, ignoring preserved source trivia.
    fn get_lexer_token(&mut self) -> Step<LexerToken> {
        loop {
            let token = self.get_raw_lexer_token()?;
            if token.value != LexerTokenValue::Trivia {
                return Ok(token);
            }
            if self.lexer.text_mode() == lexer::TextMode::Glsl && token.text.starts_with('/') {
                self.comments.push_back(convert_lexer_token_to_step(token)?);
            }
        }
    }

    fn expect_a_lexer_token(&mut self, current_location: Location) -> Step<LexerToken> {
        match self.get_lexer_token() {
            Ok(token) => Ok(token),
            Err(Finished) => Err(StepExit::Error((
                PreprocessorError::UnexpectedEndOfInput,
                current_location,
            ))),
            Err(e) => Err(e),
        }
    }

    fn expect_lexer_token(
        &mut self,
        expected: LexerTokenValue,
        current_location: Location,
    ) -> Step<Location> {
        let token = self.expect_a_lexer_token(current_location)?;
        if token.value == expected {
            Ok(token.location)
        } else {
            Err(make_unexpected_error(token))
        }
    }

    fn expect_lexer_ident(&mut self, current_location: Location) -> Step<(String, Location)> {
        let token = self.expect_a_lexer_token(current_location)?;
        if let LexerTokenValue::Ident(name) = token.value {
            Ok((name, token.location))
        } else {
            Err(make_unexpected_error(token))
        }
    }

    /// Discards a directive tail tolerantly without changing conditional state.
    fn skip_directive_tail(&mut self) {
        self.lexer.set_skipping(true);
        for token in self.lexer.by_ref() {
            if matches!(
                token,
                Ok(LexerToken {
                    value: LexerTokenValue::NewLine,
                    ..
                })
            ) {
                break;
            }
        }
        self.lexer.set_skipping(self.skipping);
    }

    fn gather_until_newline(&mut self) -> Step<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let token = self.get_lexer_token()?;
            if token.value == LexerTokenValue::NewLine {
                return Ok(tokens);
            }
            tokens.push(convert_lexer_token_to_step(token)?);
        }
    }

    fn parse_define_directive(&mut self, directive_location: Location) -> Step<()> {
        if self.skipping {
            self.skip_directive_tail();
            return Ok(());
        }

        let (name, name_location) = self.expect_lexer_ident(directive_location)?;

        // TODO validate the name?
        let mut define = Define {
            name,
            function_like: false,
            params: Default::default(),
            tokens: Default::default(),
        };

        // TODO what if token is none? EOF but still need to check it is not a redefinition?
        let mut token = self.get_lexer_token()?;

        // The is a function-like define, parse the argument list, leaving token as the next
        // token.
        if token.value == Punct::LeftParen.into() && !token.leading_whitespace {
            define.function_like = true;
            let mut first_param = true;
            loop {
                token = self.get_lexer_token()?;
                match &token.value {
                    LexerTokenValue::Punct(Punct::RightParen) => {
                        token = self.get_raw_lexer_token()?;
                        break;
                    }

                    LexerTokenValue::Ident(param_name) => {
                        if !first_param {
                            return Err(make_unexpected_error(token));
                        }
                        first_param = false;
                        define
                            .params
                            .insert(param_name.clone(), define.params.len());
                    }

                    LexerTokenValue::Punct(Punct::Comma) => {
                        if first_param {
                            return Err(make_unexpected_error(token));
                        }

                        let (param_name, param_location) =
                            self.expect_lexer_ident(token.location)?;
                        if define.params.contains_key(&param_name) {
                            return Err(StepExit::Error((
                                PreprocessorError::DuplicateParameter,
                                param_location,
                            )));
                        }
                        define.params.insert(param_name, define.params.len());
                    }
                    _ => {
                        return Err(make_unexpected_error(token));
                    }
                }
            }
        }

        // Tokens until the newline are that define's tokens (including the current one)
        loop {
            if token.value == LexerTokenValue::NewLine {
                break;
            }
            let mut replacement = convert_lexer_token_to_step(token)?;
            if replacement.trivia
                && self.lexer.text_mode() == lexer::TextMode::Glsl
                && replacement.text.starts_with('/')
            {
                self.comments.push_back(replacement.clone());
                replacement.text = " ".to_owned();
            }
            define.tokens.push(replacement);
            token = self.get_raw_lexer_token()?;
        }
        define.tokens = text::replacement_tokens(
            define.tokens,
            self.lexer.text_mode() == lexer::TextMode::Properties,
        );

        // Defines are allowed to be redefined if they are exactly the same up to token locations.
        if let Some(previous_define) = self.defines.get(&define.name) {
            if legal_redefinition(previous_define, &define) {
                Ok(())
            } else {
                Err(StepExit::Error((
                    PreprocessorError::DefineRedefined,
                    name_location,
                )))
            }
        } else {
            self.defines.insert(define.name.clone(), Rc::new(define));
            Ok(())
        }
    }

    /// Registers environment macro tokens using the selected lexical mode.
    pub(super) fn add_define(
        &mut self,
        name: &str,
        content: &str,
    ) -> Result<(), (PreprocessorError, Location)> {
        let mut define = Define {
            name: name.to_string(),
            function_like: false,
            params: Default::default(),
            tokens: Default::default(),
        };

        // Convert the content to tokens and add it to the define.
        let mut lexer = lexer::Lexer::new(content);
        lexer.set_text_mode(self.lexer.text_mode());
        loop {
            match lexer.next() {
                Some(Ok(mut lexer_token)) => {
                    // Skip over newlines (the lexer always adds a newline, which would cause an
                    // error in convert_lexer_token).
                    if lexer_token.value == LexerTokenValue::NewLine {
                        continue;
                    }

                    // Reject malformed octal numbers supplied as environment replacements.
                    let spelling = lexer_token.text.as_bytes();
                    let digits = spelling.iter().take_while(|ch| ch.is_ascii_digit()).count();
                    let number = &spelling[..digits];
                    if self.lexer.text_mode() == lexer::TextMode::Properties
                        && number.starts_with(b"0")
                        && number.iter().any(|ch| matches!(ch, b'8' | b'9'))
                        && !matches!(spelling.get(digits), Some(b'.' | b'e' | b'E'))
                    {
                        return Err((PreprocessorError::IntegerOverflow, lexer_token.location));
                    }

                    if self.lexer.text_mode() == lexer::TextMode::Glsl
                        && lexer_token.value == LexerTokenValue::Trivia
                        && lexer_token.text.starts_with('/')
                    {
                        lexer_token.text = " ".to_owned();
                    }

                    define.tokens.push(convert_lexer_token(lexer_token)?);
                }

                Some(Err(err)) => return Err(err),
                None => break,
            }
        }

        // Note this overwrites existing defines, we might want to add an option to make this
        // an error in the future.
        self.defines.insert(define.name.clone(), Rc::new(define));

        Ok(())
    }

    fn parse_undef_directive(&mut self, directive_location: Location) -> Step<()> {
        if self.skipping {
            self.skip_directive_tail();
            return Ok(());
        }

        let (name, name_location) = self.expect_lexer_ident(directive_location)?;
        // TODO check predefine
        // It is valid to undef a name that is not defined.
        self.defines.remove(&name);

        self.expect_lexer_token(LexerTokenValue::NewLine, name_location)?;
        Ok(())
    }

    fn parse_error_directive(&mut self, directive_location: Location) -> Step<()> {
        if self.skipping {
            self.skip_directive_tail();
            Ok(())
        } else {
            // TODO consume the rest of the line and make a nice error message
            Err(StepExit::Error((
                PreprocessorError::ErrorDirective,
                directive_location,
            )))
        }
    }

    fn parse_line_directive(&mut self, directive_location: Location) -> Step<()> {
        if self.skipping {
            self.skip_directive_tail();
            return Ok(());
        }

        let line = self.gather_until_newline()?;

        let mut parser = if_parser::IfParser::new(
            line,
            &self.defines,
            &self.work,
            directive_location,
            false,
            false,
        );
        let line = parser.evaluate_expression()?;

        // Validates that the line is between 0 and 2^31 as per the C standard.
        if line as u64 >= (1 << 32) {
            return Err(make_line_overflow_error(directive_location));
        }
        self.line_offset = line - directive_location.line as i64;

        if parser.peek()?.is_some() {
            // TODO figure out what to do with the file, either number or string?
            let _source_id = parser.evaluate_expression()?;
        }

        if let Some(token) = parser.peek()? {
            return Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(token.value),
                token.location,
            )));
        }

        Ok(())
    }

    fn evaluate_if_expression(&mut self, location: Location, line: Vec<Token>) -> Step<bool> {
        let mut parser = if_parser::IfParser::new(
            line,
            &self.defines,
            &self.work,
            location,
            true,
            self.iris_conditions,
        );
        if self.iris_recovery && parser.peek()?.is_none() {
            return Ok(false);
        }
        let res = parser.evaluate_expression()?;

        if let Some(token) = parser.peek()? {
            Err(StepExit::Error((
                PreprocessorError::UnexpectedToken(token.value),
                token.location,
            )))
        } else {
            Ok(res != 0)
        }
    }

    fn parse_if_directive(&mut self, directive_location: Location) -> Step<()> {
        self.parse_if_like_directive(directive_location, |this, location| {
            let line = this.gather_until_newline()?;
            this.evaluate_if_expression(location, line)
        })
    }

    /// Reads one conditional macro name, with the recovery used by Iris's JCPP listener.
    fn conditional_name(&mut self, location: Location, negated: bool) -> Step<bool> {
        if !self.iris_recovery {
            let (name, name_location) = self.expect_lexer_ident(location)?;
            self.expect_lexer_token(LexerTokenValue::NewLine, name_location)?;
            return Ok(self.defines.contains_key(&name) != negated);
        }
        let token = self.expect_a_lexer_token(location)?;
        let result = match token.value {
            LexerTokenValue::Ident(name) => self.defines.contains_key(&name) != negated,
            _ => true,
        };
        // JCPP skips the rest of the physical line even when the missing name consumed NL.
        self.skip_directive_tail();
        Ok(result)
    }

    fn parse_ifdef_directive(&mut self, directive_location: Location) -> Step<()> {
        self.parse_if_like_directive(directive_location, |this, location| {
            this.conditional_name(location, false)
        })
    }

    fn parse_ifndef_directive(&mut self, directive_location: Location) -> Step<()> {
        self.parse_if_like_directive(directive_location, |this, location| {
            this.conditional_name(location, true)
        })
    }

    fn parse_elif_directive(&mut self, directive_location: Location) -> Step<()> {
        self.skipping = true;

        // Do checks that the #elif block is well structured even if skipping.
        let block = self.blocks.last().ok_or(StepExit::Error((
            PreprocessorError::ElifOutsideOfBlock,
            directive_location,
        )))?;

        if block.had_else {
            return Err(StepExit::Error((
                PreprocessorError::ElifAfterElse,
                directive_location,
            )));
        }

        // The condition isn't parsed if it doesn't need to (and doesn't produce errors).
        if block.outer_skipped || block.had_valid_segment {
            self.skip_directive_tail();
            return Ok(());
        }

        let line = self.gather_until_newline()?;
        if self.evaluate_if_expression(directive_location, line)? {
            self.skipping = false;
            self.blocks.last_mut().unwrap().had_valid_segment = true;
        }

        Ok(())
    }

    fn parse_else_directive(&mut self, directive_location: Location) -> Step<()> {
        self.expect_lexer_token(LexerTokenValue::NewLine, directive_location)?;

        let block = self.blocks.last_mut().ok_or(StepExit::Error((
            PreprocessorError::ElseOutsideOfBlock,
            directive_location,
        )))?;

        // #else can only appear once in a block.
        if block.had_else {
            if self.preserve_text {
                self.skipping = true;
            }
            Err(StepExit::Error((
                PreprocessorError::MoreThanOneElse,
                directive_location,
            )))
        } else {
            self.skipping = block.outer_skipped || block.had_valid_segment;
            block.had_else = true;
            Ok(())
        }
    }

    fn parse_endif_directive(&mut self, directive_location: Location) -> Step<()> {
        let block = self.blocks.pop().ok_or(StepExit::Error((
            PreprocessorError::EndifOutsideOfBlock,
            directive_location,
        )))?;

        // After #endif we start processing tokens iff the block was not skipped.
        self.skipping = block.outer_skipped;

        if self.skipping {
            self.skip_directive_tail();
            Ok(())
        } else {
            self.expect_lexer_token(LexerTokenValue::NewLine, directive_location)?;
            Ok(())
        }
    }

    fn parse_if_like_directive(
        &mut self,
        directive_location: Location,
        parse: impl Fn(&mut DirectiveProcessor, Location) -> Step<bool>,
    ) -> Step<()> {
        if self.skipping {
            self.blocks.push(DirectiveBlock {
                start_location: directive_location,
                had_valid_segment: false,
                had_else: false,
                outer_skipped: true,
            });
            self.skip_directive_tail();
            Ok(())
        } else {
            let parsed = parse(self, directive_location);
            if !self.preserve_text {
                parsed.as_ref().map_err(Clone::clone)?;
            }
            let result = parsed.as_ref().copied().unwrap_or(false);
            self.skipping = !result;

            self.blocks.push(DirectiveBlock {
                start_location: directive_location,
                had_valid_segment: !self.skipping,
                had_else: false,
                outer_skipped: false,
            });
            parsed.map(|_| ())
        }
    }

    fn parse_version_directive(&mut self, directive_location: Location) -> Step<Token> {
        if self.skipping {
            self.skip_directive_tail();
            Continue.into()
        } else {
            Ok(Token::generated(
                TokenValue::Version(Version {
                    tokens: self
                        .gather_until_newline()?
                        .into_iter()
                        .map(Token::into_public)
                        .collect(),
                    is_first_directive: !(self.had_directive || self.had_non_directive_token),
                    has_comments_before: self.lexer.had_comments(),
                }),
                directive_location,
                String::new(),
            ))
        }
    }

    fn parse_extension_directive(&mut self, directive_location: Location) -> Step<Token> {
        if self.skipping {
            self.skip_directive_tail();
            Continue.into()
        } else {
            Ok(Token::generated(
                TokenValue::Extension(Extension {
                    tokens: self
                        .gather_until_newline()?
                        .into_iter()
                        .map(Token::into_public)
                        .collect(),
                    has_non_directive_before: self.had_non_directive_token,
                }),
                directive_location,
                String::new(),
            ))
        }
    }

    fn parse_pragma_directive(&mut self, directive_location: Location) -> Step<Token> {
        if self.skipping {
            self.skip_directive_tail();
            Continue.into()
        } else {
            Ok(Token::generated(
                TokenValue::Pragma(Pragma {
                    tokens: self
                        .gather_until_newline()?
                        .into_iter()
                        .map(Token::into_public)
                        .collect(),
                }),
                directive_location,
                String::new(),
            ))
        }
    }

    /// Dispatches directives and keeps comments in their original conditional group.
    fn parse_directive(&mut self, hash_location: Location) -> Step<Token> {
        let was_skipping = self.skipping;
        let token = self.expect_a_lexer_token(hash_location)?;
        let name = match &token.value {
            LexerTokenValue::Ident(name) => DirectiveName::parse(name),
            _ => None,
        };
        let opens_group = matches!(
            name,
            Some(DirectiveName::If | DirectiveName::Ifdef | DirectiveName::Ifndef)
        );
        let conditional = matches!(
            name,
            Some(
                DirectiveName::If
                    | DirectiveName::Ifdef
                    | DirectiveName::Ifndef
                    | DirectiveName::Elif
                    | DirectiveName::Else
                    | DirectiveName::Endif
            )
        );
        let result = if was_skipping && !conditional {
            if token.value == LexerTokenValue::NewLine {
                Continue.into()
            } else {
                self.skip_directive_tail();
                Continue.into()
            }
        } else {
            (|| {
                if let LexerTokenValue::Ident(ref directive) = token.value {
                    match DirectiveName::parse(directive) {
                        // TODO elif line
                        Some(DirectiveName::Error) => self.parse_error_directive(token.location)?,
                        Some(DirectiveName::Line) => self.parse_line_directive(token.location)?,

                        Some(DirectiveName::Define) => {
                            self.parse_define_directive(token.location)?
                        }
                        Some(DirectiveName::Undef) => self.parse_undef_directive(token.location)?,

                        Some(DirectiveName::If) => self.parse_if_directive(token.location)?,
                        Some(DirectiveName::Ifdef) => self.parse_ifdef_directive(token.location)?,
                        Some(DirectiveName::Ifndef) => {
                            self.parse_ifndef_directive(token.location)?
                        }
                        Some(DirectiveName::Elif) => self.parse_elif_directive(token.location)?,
                        Some(DirectiveName::Else) => self.parse_else_directive(token.location)?,
                        Some(DirectiveName::Endif) => self.parse_endif_directive(token.location)?,

                        Some(DirectiveName::Version) => {
                            let result = self.parse_version_directive(token.location);
                            self.had_directive = true;
                            return result;
                        }
                        Some(DirectiveName::Extension) => {
                            let result = self.parse_extension_directive(token.location);
                            self.had_directive = true;
                            return result;
                        }
                        Some(DirectiveName::Pragma) => {
                            let result = self.parse_pragma_directive(token.location);
                            self.had_directive = true;
                            return result;
                        }
                        None => {
                            if !self.skipping {
                                return Err(StepExit::Error((
                                    PreprocessorError::UnknownDirective,
                                    token.location,
                                )));
                            }
                        }
                    }
                    self.had_directive = true;
                    Continue.into()
                } else if !self.skipping {
                    make_unexpected_error(token).into()
                } else {
                    Continue.into()
                }
            })()
        };
        let active = if opens_group {
            !self.skipping
        } else {
            !was_skipping
        };
        if !active {
            self.comments.clear();
        } else if !self.comments.is_empty() {
            let mut newline =
                Token::generated(TokenValue::Ident(String::new()), hash_location, "\n".into());
            newline.trivia = true;
            self.comments.push_back(newline);
        }
        if self.lexer.text_mode() == lexer::TextMode::Properties {
            let mut newline =
                Token::generated(TokenValue::Ident(String::new()), hash_location, "\n".into());
            newline.trivia = true;
            self.comments.push_back(newline);
        }
        result
    }
}

impl<'a> MeLexer for DirectiveProcessor<'a> {
    /// Uses the selected source policy when expanding macro arguments.
    fn text_mode(&self) -> lexer::TextMode {
        self.lexer.text_mode()
    }

    fn charge(&self, location: Location) -> Step<()> {
        charge_work(&self.work, location)
    }

    fn step(&mut self) -> Step<Token> {
        let step = (|| {
            if let Some(comment) = self.comments.pop_front() {
                return Ok(comment);
            }
            let lexer_token = self.get_raw_lexer_token()?;
            match lexer_token.value {
                LexerTokenValue::NewLine if self.preserve_text => {
                    convert_lexer_token_to_step(lexer_token)
                }
                LexerTokenValue::NewLine => Continue.into(),
                LexerTokenValue::Hash => {
                    if lexer_token.start_of_line {
                        self.parse_directive(lexer_token.location)
                    } else if !self.skipping {
                        make_unexpected_error(lexer_token).into()
                    } else {
                        Continue.into()
                    }
                }

                _ => {
                    if !self.skipping {
                        if lexer_token.value != LexerTokenValue::Trivia {
                            self.had_non_directive_token = true;
                        }
                        convert_lexer_token_to_step(lexer_token)
                    } else {
                        Continue.into()
                    }
                }
            }
        })();

        if step == Err(StepExit::Finished) && !self.iris_recovery {
            if let Some(block) = self.blocks.pop() {
                return Err(StepExit::Error((
                    PreprocessorError::UnfinishedBlock,
                    block.start_location,
                )));
            }
        }

        step
    }

    fn get_define(&self, name: &str) -> Option<&Rc<Define>> {
        self.defines.get(name)
    }

    fn apply_line_offset(&self, line: u32, location: Location) -> Step<u32> {
        if let Ok(offset_line) = u32::try_from(line as i64 + self.line_offset) {
            Ok(offset_line)
        } else {
            Err(make_line_overflow_error(location))
        }
    }
}
