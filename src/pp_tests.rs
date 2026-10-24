use super::lexer::{self, Token as LexerToken, TokenValue as LexerTokenValue};
use super::pp::{convert_lexer_token, Preprocessor, PreprocessorItem};
use super::token::{Float, Integer, Location, PreprocessorError, Punct, Token, TokenValue};

/// Collects rendered text and semantic values from either preservation mode.
fn preserved_output(source: &str, glsl: bool) -> (String, Vec<TokenValue>) {
    let mut pp = Preprocessor::new(source);
    if glsl {
        pp.set_preserve_glsl_text();
    } else {
        pp.set_preserve_text(true);
    }
    let mut text = String::new();
    let mut values = Vec::new();
    while let Some(token) = pp.next_text() {
        let token = token.unwrap();
        text.push_str(&token.text);
        values.extend(token.value);
    }
    (text, values)
}

/// Collects semantic values using ordinary preprocessing.
fn preprocessed_values(source: &str) -> Vec<TokenValue> {
    Preprocessor::new(source)
        .map(|token| token.unwrap().value)
        .collect()
}

#[test]
fn preserved_macro_invocations_ignore_trivia() {
    let source = "#define F(x) x\n#define Z() 7\nF (1) F/*gap*/(2) F\n(3) Z( ) Z(/*empty*/)\nF /* keep */ + 4;\nF /* trailing */";
    let expected = preprocessed_values("1 2 3 7 7 F + 4; F");
    for glsl in [false, true] {
        let (text, values) = preserved_output(source, glsl);
        assert_eq!(values, expected);
        assert!(text.contains("F /* keep */ + 4;"), "{}", text);
        assert!(text.contains("F /* trailing */"), "{}", text);
    }
}

#[test]
fn preserved_trivia_does_not_change_directive_metadata() {
    for prefix in ["  ", "\t/* before */ ", "// before\n", "/* before\n */"] {
        let source = format!(
            "{}#version 450\n  /* gap */ #extension GL_EXT_test : enable\n",
            prefix
        );
        let expected = preprocessed_values(&source);
        assert!(matches!(&expected[0], TokenValue::Version(version) if version.is_first_directive));
        assert!(
            matches!(&expected[1], TokenValue::Extension(extension) if !extension.has_non_directive_before)
        );
        for glsl in [false, true] {
            assert_eq!(preserved_output(&source, glsl).1, expected, "{}", source);
        }
    }
}

#[test]
fn preserved_output_separates_comment_and_token_boundaries() {
    for source in [
        "#define DIV /\n4 DIV/*c*/2;",
        "#define DIV /\n4 DIV/2;",
        "#define DIV /\n4 DIV*2;",
        "#define PLUS +\n1 PLUS+2;",
        "#define NAME() foo\nNAME()bar;",
        "#define NUM() 1\nNUM().0;",
        "#define NUM() 1\nNUM()e;",
    ] {
        let expected = preprocessed_values(source);
        for glsl in [false, true] {
            let (text, values) = preserved_output(source, glsl);
            assert_eq!(values, expected, "{}", source);
            assert_eq!(preprocessed_values(&text), expected, "{}", text);
        }
    }
}

#[test]
fn preservation_modes_have_identical_preprocessing_results() {
    for source in [
        "#define F(x) x\nF (1) F/*gap*/(2) F\n(3)",
        "#define F() 7\nF( ) F(/*empty*/) F(\n)",
        "#define F(x) x\nF /* keep */ + 1;\nF /* trailing */",
        "#define F(x) x\n#define A F\nA /* call */(4) F(F (5))",
        "  #version 450\n  #extension GL_EXT_test : enable\n#pragma optimize(on)\n",
        "/* before */ #version 450\n/* gap */ #extension GL_EXT_test : enable\n",
        "int x;\n#version 450\n#extension GL_EXT_test : enable\n",
        "#define DIV /\n4 DIV/*c*/2; 4 DIV/2; 4 DIV*2;",
        "#define PLUS +\n1 PLUS+2;\n#define NUM() 1\nNUM().0; NUM()e;",
        "#if 0\n#error It's skipped\n@ 'bad\n#else\n7\n#endif\n",
    ] {
        let expected = preprocessed_values(source);
        for glsl in [false, true] {
            let (text, values) = preserved_output(source, glsl);
            assert_eq!(values, expected, "{}", source);
            let rendered_values: Vec<_> = expected
                .iter()
                .filter(|value| {
                    !matches!(
                        value,
                        TokenValue::Version(_) | TokenValue::Extension(_) | TokenValue::Pragma(_)
                    )
                })
                .cloned()
                .collect();
            assert_eq!(preprocessed_values(&text), rendered_values, "{}", text);
        }
    }
}

#[test]
fn preserved_line_macros_match_token_mode() {
    let source = "#define F(x) x\n#line 80\nF (__LINE__);";
    let expected = preprocessed_values(source);
    for glsl in [false, true] {
        assert_eq!(preserved_output(source, glsl).1, expected);
    }
}

#[test]
fn preserved_glsl_splices_all_normalized_newlines() {
    for newline in ["\n", "\r", "\r\n", "\n\r"] {
        let source = format!("int va\\{}lue = 1;\n", newline);
        let expected = preprocessed_values(&source);
        let (text, values) = preserved_output(&source, true);
        assert_eq!(values, expected);
        assert_eq!(text, "int value = 1;\n");
        assert_eq!(preprocessed_values(&text), expected);
    }
}

#[test]
fn text_output_preserves_literals_comments_and_macro_boundaries() {
    let source = "#define NEG -1\n#if -NEG > 0\nvalue=NEG \\\n\"NEG\" /* NEG\n#if 0\n*/\nafter=0.50\n#endif\n";
    let mut pp = Preprocessor::new(source);
    pp.set_preserve_text(true);
    pp.set_iris_conditions(true);
    let mut output = String::new();
    while let Some(token) = pp.next_text() {
        output.push_str(&token.unwrap().text);
    }
    assert_eq!(
        output,
        "value=-1 \\\n\"NEG\" /* NEG\n#if 0\n*/\nafter=0.50\n"
    );
}

struct NoopPreprocessor<'a> {
    lexer: lexer::Lexer<'a>,
}

impl<'a> NoopPreprocessor<'a> {
    pub fn new(input: &'a str) -> NoopPreprocessor<'a> {
        NoopPreprocessor {
            lexer: lexer::Lexer::new(input),
        }
    }
}

impl<'a> Iterator for NoopPreprocessor<'a> {
    type Item = Result<Token, (PreprocessorError, Location)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.lexer.next() {
                Some(Ok(LexerToken {
                    value: LexerTokenValue::NewLine,
                    ..
                })) => continue,
                Some(Ok(token)) => {
                    return Some(Ok(convert_lexer_token(token).unwrap().into_public()))
                }
                None => return None,
                Some(Err(err)) => return Some(Err(err)),
            }
        }
    }
}

#[track_caller]
fn check_preprocessed_result(input: &str, expected: &str) {
    let pp_items: Vec<PreprocessorItem> = Preprocessor::new(input).collect();
    let noop_items: Vec<PreprocessorItem> = NoopPreprocessor::new(expected).collect();

    assert_eq!(pp_items.len(), noop_items.len());
    for (pp_item, noop_item) in pp_items.iter().zip(noop_items) {
        if let (Ok(pp_tok), Ok(ref noop_tok)) = (pp_item, noop_item) {
            assert_eq!(pp_tok.value, noop_tok.value);
        } else {
            unreachable!();
        }
    }
}

#[track_caller]
fn check_preprocessing_error(input: &str, expected_err: PreprocessorError) {
    for item in Preprocessor::new(input) {
        if let Err((err, _)) = item {
            assert_eq!(err, expected_err);
            return;
        }
    }
    unreachable!();
}

#[test]
fn parse_directive() {
    // Test parsing a simple directive
    check_preprocessed_result("#define A B", "");

    // Test parsing a simple directive with comment right after the hash
    check_preprocessed_result("# /**/ \tdefine A B", "");

    // Test preprocessing directive can only come after a newline
    check_preprocessing_error("42 #define A B", PreprocessorError::UnexpectedHash);

    // Test not an identifier after the hash
    check_preprocessing_error(
        "# ; A B",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Semicolon)),
    );

    // Test a fake directive
    check_preprocessing_error("#CoucouLeChat", PreprocessorError::UnknownDirective);
}

#[test]
fn parse_error() {
    // Test preprocessing directive can only come after a newline
    check_preprocessing_error("#error", PreprocessorError::ErrorDirective);
}

#[test]
fn parse_define() {
    // Test the define name must be an identifier
    check_preprocessing_error(
        "#define [",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::LeftBracket)),
    );
    check_preprocessing_error(
        "#define 1.0",
        PreprocessorError::UnexpectedToken(TokenValue::Float(Float {
            value: 1.0,
            width: 32,
        })),
    );

    // Test that there must be a name before the new line
    check_preprocessing_error(
        "#define
        A",
        PreprocessorError::UnexpectedNewLine,
    );

    // Test putting a garbage character for the define name
    check_preprocessing_error("#@", PreprocessorError::UnexpectedCharacter);
}

#[test]
fn parse_undef() {
    // Test the define name must be an identifier
    check_preprocessing_error(
        "#undef !",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Bang)),
    );

    // Test that there must be a name before the new line
    check_preprocessing_error(
        "#undef
        A",
        PreprocessorError::UnexpectedNewLine,
    );
}

#[test]
fn argument_less_define() {
    // Test a simple case
    check_preprocessed_result(
        "#define A B
         A",
        "B",
    );

    // Test an empty define
    check_preprocessed_result(
        "#define A
         A something",
        "something",
    );

    // Test a define containing a token that's itself
    check_preprocessed_result(
        "#define A A B C
         A",
        "A B C",
    );

    // Test a define invocation followed by () doesn't remove them
    check_preprocessed_result(
        "#define A foo
         A()",
        "foo()",
    );

    // Test nesting define
    check_preprocessed_result(
        "#define B C
         #define A B
         A",
        "C",
    );

    // Test nesting with a bunch of empty tokens
    check_preprocessed_result(
        "#define D
         #define C D D
         #define B C C D D
         #define A B B C C D D
         A truc",
        "truc",
    );

    // Test line continuation doesn't break the define
    check_preprocessed_result(
        "#define C A \\
         B
         C",
        "A B",
    );

    // Test that multiline comments don't break the define
    check_preprocessed_result(
        "#define C A /*
         */ B
         C",
        "A B",
    );

    // Test that substitution in defines happens at invocation time
    // Checks both when undefined a ident found in A and redefining it.
    check_preprocessed_result(
        "#define B stuff
         #define C stuffy stuff
         #define A B C
         #undef B
         #undef C
         #define C :)
         A",
        "B :)",
    );

    // The first token of the define can be a ( if it has whitespace after
    // the identifier
    check_preprocessed_result(
        "#define A ()
         #define B\t(!
         #define C/**/(a, b)
         A B C",
        "() (! (a, b)",
    );

    // Check that hashes are disallowed in defines
    check_preprocessing_error("#define A #", PreprocessorError::UnexpectedHash);
}

#[test]
fn function_like_define() {
    // Test calling a define with 1 argument
    check_preprocessed_result(
        "#define A(a) +a+
         A(1)",
        "+1+",
    );

    // Test calling a define with 0 arguments
    check_preprocessed_result(
        "#define A() foo
         A()",
        "foo",
    );

    // Test called a define with multiple arguments
    check_preprocessed_result(
        "#define A(a, b) b a
         A(1, 2)",
        "2 1",
    );

    // Test not calling a function-like macro just returns the identifier
    check_preprocessed_result(
        "#define A(a) foobar
         A + B",
        "A + B",
    );

    // Test that duplicate argument names are disallowed
    check_preprocessing_error("#define A(a, a) foo", PreprocessorError::DuplicateParameter);

    // Test that non ident or , are disallowed in the parameter list
    check_preprocessing_error(
        "#define A(%) foo",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Percent)),
    );
    check_preprocessing_error(
        "#define A(a, %) foo",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Percent)),
    );

    // Test that starting the param list with a comma is disallowed
    check_preprocessing_error(
        "#define A(,a) foo",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Comma)),
    );

    // Test that two identifier in a row is disallowed
    check_preprocessing_error(
        "#define A(a b) foo",
        PreprocessorError::UnexpectedToken(TokenValue::Ident("b".to_string())),
    );
    check_preprocessing_error(
        "#define A(a, b c) foo",
        PreprocessorError::UnexpectedToken(TokenValue::Ident("c".to_string())),
    );

    // Test passing too many arguments is disallowed
    check_preprocessing_error(
        "#define A(a, b) foo
         A(1, 2, 3)",
        PreprocessorError::TooManyDefineArguments,
    );

    // Test passing too few arguments is disallowed
    check_preprocessing_error(
        "#define A(a, b) foo
         A(1)",
        PreprocessorError::TooFewDefineArguments,
    );

    // Test passing no argument to a define with one parameter.
    check_preprocessed_result(
        "#define A(a) foo a
         A()",
        "foo",
    );

    // Test EOF while parsing define arguments
    check_preprocessing_error(
        "#define A(a, b) foo
         A(",
        PreprocessorError::UnexpectedEndOfInput,
    );

    // Test unknown token while parsing define arguments
    check_preprocessing_error(
        "#define A(a) foo
         A($)",
        PreprocessorError::UnexpectedCharacter,
    );

    // Test #error while parsing arguments
    check_preprocessing_error(
        "#define A(a) foo
         A(
         #error
         )",
        PreprocessorError::ErrorDirective,
    );

    // Test that commas inside () are not used to split parameters
    check_preprocessed_result(
        "#define STUFF(a, b) a + b
         STUFF((1, 2), 3)",
        "(1, 2) + 3",
    );

    // Test that commas inside more nesting
    check_preprocessed_result(
        "#define STUFF(a, b) a + b
         STUFF((((()1, 2))), 3)",
        "(((()1, 2))) + 3",
    );

    // Test that a macro can be used in its own arguments
    check_preprocessed_result(
        "#define B(foo) (foo)
         B(1 B(2))",
        "(1 (2))",
    );

    // Test that define expansion doesn't happen while parsing define call arguments. If it
    // were, the COMMA would make a third argument to A appear, which would be an error.
    check_preprocessed_result(
        "#define A(x, y) x + y
         #define COMMA ,
         A(1 COMMA 2, 3)",
        "1, 2 + 3",
    );

    // Test that define expansion of arguments happens before giving tokens in the define
    // call. If it weren't the COMMA wouldn't make a second argument to A appear, which would
    // be an error.
    check_preprocessed_result(
        "#define A(x, y) x + y
         #define COMMA ,
         #define B(foo) A(foo)
         B(1 COMMA 2)",
        "1 + 2",
    );

    // Same but checking args are fully expanded by doing some weird nesting. The inner B's
    // call to A will create the token that will cause the outer B to call A with two arguments
    check_preprocessed_result(
        "#define A(a, b) ,(a + b)
         #define B(foo) A(foo)
         #define COMMA ,
         B(1 B(2 COMMA 3))",
        ",(1 + (2 + 3))",
    );

    // Test that the ( , and ) can come from the expansion of an argument.
    check_preprocessed_result(
        "#define LPAREN (
         #define COMMA ,
         #define RPAREN )
         #define A(a, b) (a + b)
         #define B(a) a
         B(A LPAREN 1 COMMA 2 RPAREN)",
        "(1 + 2)",
    );

    // Test that a define being expanded cannot be re-expanded inside an argument to an inner
    // define call.
    check_preprocessed_result(
        "#define LPAREN (
         #define COMMA ,
         #define RPAREN )
         #define A(a) a
         #define B(a) a
         #define C B(A(C))
         C",
        "C",
    );

    // Test that an error during define argument expansion gets surfaced properly.
    check_preprocessing_error(
        "#define A(x, y) x + y
         #define COMMA ,
         #define B(foo) A(1, foo)
         B(2 COMMA 3)",
        PreprocessorError::TooManyDefineArguments,
    );

    // Test putting comments inside the arguments to a function-like macro
    check_preprocessed_result(
        "#define A(x, y) x + y
         A(/*this is a 2*/ 2,
           // And below is a 3!
           3
        )",
        "2 + 3",
    );
}

#[test]
fn define_redefinition() {
    // Test that it is valid to redefine a define with the same tokens.
    // Function-like case.
    check_preprocessed_result(
        "#define A(x, y) (x + y)
         #define A(x, y) (x + y)",
        "",
    );
    // Not function-like case.
    check_preprocessed_result(
        "#define A (x, y)
         #define A (x, y)",
        "",
    );

    // Oh no a token is different!
    check_preprocessing_error(
        "#define A (a, y)
         #define A (x, y)",
        PreprocessorError::DefineRedefined,
    );

    // Oh no, one has more tokens!
    check_preprocessing_error(
        "#define A a b
         #define A a",
        PreprocessorError::DefineRedefined,
    );

    // Oh no, one is function-like and not the other
    check_preprocessing_error(
        "#define A a
         #define A() a",
        PreprocessorError::DefineRedefined,
    );

    // Oh no, a parameter name is different!
    check_preprocessing_error(
        "#define A(b) a
         #define A(c) a",
        PreprocessorError::DefineRedefined,
    );

    // Oh no, the parameter count is different!
    check_preprocessing_error(
        "#define A(b, d) a
         #define A(c) a",
        PreprocessorError::DefineRedefined,
    );

    // Oh no, the parameter order is different!
    check_preprocessing_error(
        "#define A(b, d) a
         #define A(d, b) a",
        PreprocessorError::DefineRedefined,
    );
}

#[test]
fn define_undef() {
    // Basic test
    check_preprocessed_result(
        "#define A B
         #undef A
         A",
        "A",
    );

    // It is ok to undef a non-existent define
    check_preprocessed_result(
        "#undef A
         A",
        "A",
    );

    // It is ok to undef a define twice
    check_preprocessed_result(
        "#define A B
         #undef A
         #undef A
         A",
        "A",
    );
}

#[test]
fn parse_if() {
    // Basic test of parsing and operations.
    check_preprocessed_result(
        "#if 0
             1
         #endif
         #if 1
             2
         #endif",
        "2",
    );

    // Check that garbage tokens are allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #if % ^ * )
         #endif
         #endif",
        "",
    );

    // Check a couple simple expressions
    check_preprocessed_result(
        "#define FOO
         #if defined(FOO)
         A
         #endif",
        "A",
    );
    check_preprocessed_result(
        "#if defined FOO
         A
         #endif",
        "",
    );
    check_preprocessed_result(
        "#define FOO 0
         #if FOO
         A
         #endif",
        "",
    );

    check_preprocessed_result(
        "#define FOO FOO
         #if FOO
         #endif",
        "",
    );

    check_preprocessed_result(
        "#define FOO 1
         #define BAR 1
         #if (FOO & BAR) == 1
         2
         #endif",
        "2",
    );

    check_preprocessed_result(
        "#if 1 + -2 * 3 == -5
         2
         #endif",
        "2",
    );

    check_preprocessed_result(
        "#if 4 % 3 == 1
         2
         #endif",
        "2",
    );

    check_preprocessed_result(
        "#if 4 / 3 == 1
         2
         #endif",
        "2",
    );

    // TODO test that the if_parse gives back the unused tokens
    // TODO test expressions?
}

#[test]
fn parse_ifdef() {
    // Basic test of parsing and operations.
    check_preprocessed_result(
        "#define A
         #ifdef B
             1
         #endif
         #ifdef A
             2
         #endif",
        "2",
    );

    // Check that extra tokens after the identifier are disallowed.
    check_preprocessing_error(
        "#ifdef B ;
         #endif",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Semicolon)),
    );

    // Check that the identifier is required.
    check_preprocessing_error(
        "#ifdef
         #endif",
        PreprocessorError::UnexpectedNewLine,
    );

    // Check that extra tokens are allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #ifdef B ;
         #endif
         #endif",
        "",
    );

    // Check that having no identifier is allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #ifdef
         #endif
         #endif",
        "",
    );
}

#[test]
fn parse_ifndef() {
    // Basic test of parsing and operations.
    check_preprocessed_result(
        "#define A
         #ifndef B
             1
         #endif
         #ifndef A
             2
         #endif",
        "1",
    );

    // Check that extra tokens after the identifier are disallowed.
    check_preprocessing_error(
        "#ifndef B ;
         #endif",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Semicolon)),
    );

    // Check that the identifier is required.
    check_preprocessing_error(
        "#ifndef
         #endif",
        PreprocessorError::UnexpectedNewLine,
    );

    // Check that extra tokens are allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #ifndef B ;
         #endif
         #endif",
        "",
    );

    // Check that having no identifier is allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #ifndef
         #endif
         #endif",
        "",
    );
}

#[test]
fn parse_elif() {
    // Basic test of using elif
    check_preprocessed_result(
        "#if 1
             1
         #elif 1
             2
         #endif
         #if 0
             3
         #elif 1
             4
         #elif 0
             5
         #endif",
        "1 4",
    );

    // Check that #elif must appear in a block
    check_preprocessing_error(
        "#elif
         aaa",
        PreprocessorError::ElifOutsideOfBlock,
    );

    // Check that #elif must appear before the #else
    check_preprocessing_error(
        "#if 0
         #else
         #elif 1
         #endif",
        PreprocessorError::ElifAfterElse,
    );

    // Check that #elif must appear before the #else even when skipping
    check_preprocessing_error(
        "#if 0
             #if 0
             #else
             #elif 1
             #endif
         #endif",
        PreprocessorError::ElifAfterElse,
    );

    // Check that the condition isn't processed if the block is skipped.
    check_preprocessed_result(
        "#if 0
            #if 1
            #elif # !
            #endif
         #endif",
        "",
    );

    // Check that the condition isn't processed if a previous segment was used.
    check_preprocessed_result(
        "#if !defined(FOO)
         #elif # !
         #endif

         #if 0
         #elif 1
         #elif # %
         #endif",
        "",
    );

    // Check that elif isn't taken if at least one block was taken.
    check_preprocessed_result(
        "#if 1
             A
         #elif 1
             B
         #endif
         
         #if 0
             C
         #elif 1
             D
         #elif 1
             E
         #endif",
        "A D",
    );
}

#[test]
fn parse_else() {
    // Basic test of using else with if and elif
    check_preprocessed_result(
        "#if 0
             1
         #else
             2
         #endif
         #if 0
             3
         #elif 0
             4
         #else
             5
         #endif",
        "2 5",
    );

    // Check that #else must appear in a block
    check_preprocessing_error(
        "#else
         aaa",
        PreprocessorError::ElseOutsideOfBlock,
    );

    // Check that else can only appear once in a block, when skipping or not.
    check_preprocessing_error(
        "#if 0
         #else
         #else
         #endif",
        PreprocessorError::MoreThanOneElse,
    );
    check_preprocessing_error(
        "#if 0
            #if 0
            #else
            #else
            #endif
         #endif",
        PreprocessorError::MoreThanOneElse,
    );

    // Check that else isn't taken if at least one block was taken.
    check_preprocessed_result(
        "#if 1
             A
         #else
             B
         #endif
         #if 0
             C
         #elif 1
             D
         #else
             E
         #endif",
        "A D",
    );

    // Check that else isn't taken if it's skipped from the outside.
    check_preprocessed_result(
        "#if 0
             #if 0
             #else
                FOO
             #endif
         #endif",
        "",
    );
}

#[test]
fn parse_endif() {
    // Basic test of using endif
    check_preprocessed_result(
        "#if 0
         a
         #endif
         b",
        "b",
    );

    // Check that extra tokens are disallowed.
    check_preprocessing_error(
        "#if 1
         #endif %",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Percent)),
    );

    // Check that extra tokens are disallowed even for an inner_skipped block.
    check_preprocessing_error(
        "#if 0
         #endif %",
        PreprocessorError::UnexpectedToken(TokenValue::Punct(Punct::Percent)),
    );

    // Check that extra tokens are allowed if we are skipping.
    check_preprocessed_result(
        "#if 0
         #if 0
         #endif %
         #endif",
        "",
    );

    // Check that a missing endif triggers an error, nest and unnested case.
    check_preprocessing_error("#if 0", PreprocessorError::UnfinishedBlock);
    check_preprocessing_error(
        "#if 1
         #if 0
         #endif",
        PreprocessorError::UnfinishedBlock,
    );

    // Check that endif must be well nested with ifs.
    check_preprocessing_error("#endif", PreprocessorError::EndifOutsideOfBlock);
    check_preprocessing_error(
        "#if 1
         #endif
         #endif",
        PreprocessorError::EndifOutsideOfBlock,
    );
}

#[test]
fn skipping_behavior() {
    // Check regular tokens are skipped
    check_preprocessed_result(
        "#if 0
             a b % 1 2u
         #endif",
        "",
    );
    // Check random hash is allowed while skipping
    check_preprocessed_result(
        "#if 0
             a # b
         #endif",
        "",
    );

    // Check that a hash at the start of the line and nothing else if valid while skipping
    check_preprocessed_result(
        "#if 0
             #
         #endif",
        "",
    );

    // Check invalid directives are allowed while skipping
    check_preprocessed_result(
        "#if 0
             #CestlafauteaNandi
         #endif",
        "",
    );

    // Check that defines skipped (otherwise there would be a redefinition error)
    check_preprocessed_result(
        "#if 0
             #define A 1
         #endif
         #define A 2",
        "",
    );

    // Check that undefs are skipped.
    check_preprocessed_result(
        "#define A 1
         #if 0
             #undef A
         #endif
         A",
        "1",
    );

    // Check that #error is skipped.
    check_preprocessed_result(
        "#if 0
             #error
         #endif",
        "",
    );

    // Check that #line directives are skipped.
    check_preprocessed_result(
        "#if 0
             #line 1000
         #endif
         __LINE__",
        "4u",
    );

    // Check that #version/extension/pragma are skipped.
    check_preprocessed_result(
        "#if 0
             #version a b c
             #extension 1 2 3
             #pragma tic
         #endif",
        "",
    );
}

#[test]
fn parse_line() {
    // Test that #line with a uint and int is allowed.
    check_preprocessed_result(
        "#line 4u
         #line 3
         #line 0xF00
         __LINE__",
        "0xF01u",
    );

    // Test with something other than a number after #line (including a newline)
    check_preprocessing_error("#line !", PreprocessorError::UnexpectedEndOfInput);
    check_preprocessing_error("#line", PreprocessorError::UnexpectedEndOfInput);
    check_preprocessing_error(
        "#line foo",
        PreprocessorError::UnexpectedToken(TokenValue::Ident("foo".into())),
    );

    // Test that an invalid #line directive is allowed if skipping
    check_preprocessed_result(
        "#if 0
         #line !
         #endif
         __LINE__",
        "4u",
    );
    // Test that #line must have a newline after the integer (this will change when #line
    // supports constant expressions)
    check_preprocessing_error("#line 1 #", PreprocessorError::UnexpectedHash);

    // test that the integer must fit in a u32
    check_preprocessing_error("#line 4294967296u", PreprocessorError::LineOverflow);
    // test that the integer must fit in a u32
    check_preprocessed_result("#line 4294967295u", "");
    check_preprocessing_error("#line -1", PreprocessorError::LineOverflow);

    // Test some expression
    check_preprocessed_result(
        "#line 20 << 2 + 1
         __LINE__",
        "161u",
    );
    check_preprocessed_result(
        "#line 20 * 0 -2 + 100
        __LINE__",
        "99u",
    );
    check_preprocessed_result(
        "#line 0 (1 << 1 * (10)) % 2
        __LINE__",
        "1u",
    );

    // Test passing both a line and file id is allowed.
    check_preprocessed_result("#line 0 1", "");

    // Test passing more numbers is invalid.
    check_preprocessing_error(
        "#line 0 1 2",
        PreprocessorError::UnexpectedToken(TokenValue::Integer(Integer {
            value: 2,
            signed: true,
            width: 32,
        })),
    );
}

#[test]
fn line_define() {
    // Test that the __LINE__ define gives the number of the line.
    check_preprocessed_result(
        "__LINE__
         __LINE__

         __LINE__",
        "1u 2u 4u",
    );

    // Test that __LINE__ split over multiple lines gives the first line.
    check_preprocessed_result("__L\\\nINE__", "1u");

    // Test that the __LINE__ define used in define gives the invocation's line
    check_preprocessed_result(
        "#define MY_DEFINE __LINE__
         MY_DEFINE
         MY\\\n_DEFINE",
        "2u 3u",
    );

    // Test a corner case where the __LINE__ is a peeked token for function-like
    // define parsing.
    check_preprocessed_result(
        "#define A(foo) Bleh
         A __LINE__ B",
        "A 2u B",
    );

    // Test that __LINE__ inside function like defines is the position of the closing )
    check_preprocessed_result(
        "#define B + __LINE__ +
         #define A(X, Y) X __LINE__ Y B
         A(-, -)
         A(-, -
         )",
        "- 3u - + 3u +
         - 5u - + 5u +",
    );

    // Test that the __LINE__ inside a define's argument get the correct value.
    check_preprocessed_result(
        "#define A(X) X
         A(__LINE__
         __LINE__)",
        "2u 3u",
    );
    check_preprocessed_result(
        "#define B(X) X
         #define A(X) B(X) + __LINE__
         A(__LINE__)",
        "3u + 3u",
    );

    // Check that #line is taken into account and can modify the line number in both directions.
    check_preprocessed_result(
        "#line 1000
         __LINE__
         __LINE__

         #line 0
         __LINE__
         __LINE__",
        "1001u 1002u 1u 2u",
    );

    // Check that line computations are not allowed to overflow an u32
    check_preprocessed_result(
        "#line 4294967294
         __LINE__",
        "4294967295u",
    );
    check_preprocessing_error(
        "#line 4294967295
         __LINE__",
        PreprocessorError::LineOverflow,
    );

    // Check that line directives don't allow floats
    check_preprocessing_error(
        "#line 1.0",
        PreprocessorError::UnexpectedToken(TokenValue::Float(Float {
            value: 1.0,
            width: 32,
        })),
    )
}

#[test]
fn parse_version() {
    // Check that the #version directive is recognized and gets all the tokens until the newline
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("#version 1 ; (").collect();
    assert_eq!(tokens.len(), 1);
    match &tokens[0] {
        Ok(Token {
            value: TokenValue::Version(version),
            ..
        }) => {
            assert!(!version.has_comments_before);
            assert!(version.is_first_directive);
            assert_eq!(version.tokens.len(), 3);
            assert_eq!(
                version.tokens[0].value,
                TokenValue::Integer(Integer {
                    value: 1,
                    signed: true,
                    width: 32
                })
            );
            assert_eq!(version.tokens[1].value, TokenValue::Punct(Punct::Semicolon));
            assert_eq!(version.tokens[2].value, TokenValue::Punct(Punct::LeftParen));
        }
        _ => {
            unreachable!();
        }
    };

    // Check that we correctly detect comments before the #version directive.
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("/**/#version (").collect();
    assert_eq!(tokens.len(), 1);
    match &tokens[0] {
        Ok(Token {
            value: TokenValue::Version(version),
            ..
        }) => {
            assert!(version.has_comments_before);
            assert!(version.is_first_directive);
            assert_eq!(version.tokens.len(), 1);
            assert_eq!(version.tokens[0].value, TokenValue::Punct(Punct::LeftParen));
        }
        _ => {
            unreachable!();
        }
    };

    // Check that we properly detect tokens before the #version directive.
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("4 \n #version (").collect();
    assert_eq!(tokens.len(), 2);
    match &tokens[1] {
        Ok(Token {
            value: TokenValue::Version(version),
            ..
        }) => {
            assert!(!version.has_comments_before);
            assert!(!version.is_first_directive);
            assert_eq!(version.tokens.len(), 1);
            assert_eq!(version.tokens[0].value, TokenValue::Punct(Punct::LeftParen));
        }
        _ => {
            unreachable!();
        }
    };

    // Same thing but with another preprocessor directive.
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("#line 1\n #version (").collect();
    assert_eq!(tokens.len(), 1);
    match &tokens[0] {
        Ok(Token {
            value: TokenValue::Version(version),
            ..
        }) => {
            assert!(!version.has_comments_before);
            assert!(!version.is_first_directive);
            assert_eq!(version.tokens.len(), 1);
            assert_eq!(version.tokens[0].value, TokenValue::Punct(Punct::LeftParen));
        }
        _ => {
            unreachable!();
        }
    };
}

#[test]
fn parse_extension() {
    // Check that the #extension directive is recognized and gets all the tokens until the newline
    let tokens: Vec<PreprocessorItem> =
        Preprocessor::new("#extension USE_WEBGPU_INSTEAD_OF_GL : required").collect();
    assert_eq!(tokens.len(), 1);
    match &tokens[0] {
        Ok(Token {
            value: TokenValue::Extension(extension),
            ..
        }) => {
            assert!(!extension.has_non_directive_before);
            assert_eq!(extension.tokens.len(), 3);
            assert_eq!(
                extension.tokens[0].value,
                TokenValue::Ident("USE_WEBGPU_INSTEAD_OF_GL".into())
            );
            assert_eq!(extension.tokens[1].value, TokenValue::Punct(Punct::Colon));
            assert_eq!(
                extension.tokens[2].value,
                TokenValue::Ident("required".into())
            );
        }
        _ => {
            unreachable!();
        }
    };

    // Check that we correctly detect non directive tokens before the #extension
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("miaou\n#extension").collect();
    assert_eq!(tokens.len(), 2);
    match &tokens[1] {
        Ok(Token {
            value: TokenValue::Extension(extension),
            ..
        }) => {
            assert!(extension.has_non_directive_before);
            assert_eq!(extension.tokens.len(), 0);
        }
        _ => {
            unreachable!();
        }
    };
}

#[test]
fn parse_pragma() {
    // Check that the #extension directive is recognized and gets all the tokens until the newline
    let tokens: Vec<PreprocessorItem> = Preprocessor::new("#pragma stuff").collect();
    assert_eq!(tokens.len(), 1);
    match &tokens[0] {
        Ok(Token {
            value: TokenValue::Pragma(pragma),
            ..
        }) => {
            assert_eq!(pragma.tokens.len(), 1);
            assert_eq!(pragma.tokens[0].value, TokenValue::Ident("stuff".into()));
        }
        _ => {
            unreachable!();
        }
    };
}

#[test]
fn add_define() {
    // Test adding multiple defines at the start.
    let mut pp = Preprocessor::new("A B");
    pp.add_define("A", "bat").unwrap();
    pp.add_define("B", "man").unwrap();

    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "bat");
        }
        _ => {
            unreachable!();
        }
    }
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "man");
        }
        _ => {
            unreachable!();
        }
    }

    // Test that a multiline define works.
    let mut pp = Preprocessor::new("A");
    pp.add_define("A", "bat\nman").unwrap();

    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "bat");
        }
        _ => {
            unreachable!();
        }
    }
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "man");
        }
        _ => {
            unreachable!();
        }
    }

    // Test adding defines in the middle without overwriting.
    let mut pp = Preprocessor::new("A A");
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "A");
        }
        _ => {
            unreachable!();
        }
    }

    pp.add_define("A", "foo").unwrap();
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "foo");
        }
        _ => {
            unreachable!();
        }
    }

    // Test adding defines in the middle with overwriting.
    let mut pp = Preprocessor::new(
        "#define A bat
         A A",
    );
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "bat");
        }
        _ => {
            unreachable!();
        }
    }

    pp.add_define("A", "foo").unwrap();
    match pp.next() {
        Some(Ok(Token {
            value: TokenValue::Ident(ident),
            ..
        })) => {
            assert_eq!(ident, "foo");
        }
        _ => {
            unreachable!();
        }
    }

    // Test a parsing error.
    let mut pp = Preprocessor::new("A");
    assert_eq!(
        pp.add_define("A", "@").unwrap_err().0,
        PreprocessorError::UnexpectedCharacter
    );

    // Test some crashes detected by fuzzing
    let pp = Preprocessor::new("#ifG/fp");
    for _ in pp {}

    let pp = Preprocessor::new("\n#if~O%t");
    for _ in pp {}

    let pp = Preprocessor::new("#if~f>>~f");
    for _ in pp {}
}

#[test]
fn float_conditions_are_opt_in_and_use_expanded_macro_state() {
    let input = "#define BASE 1.5
        #define ALIAS BASE
        #if 0
        #undef BASE
        #define BASE 9.5
        #endif
        #if ALIAS == 1 && ENV == 2
        10
        #endif
        #undef BASE
        #if BASE
        99
        #else
        20
        #endif
        #if 0.9
        99
        #elif -1.5 == -1
        30
        #endif
        ENV";
    let mut pp = Preprocessor::new(input);
    pp.set_iris_conditions(true);
    pp.add_define("ENV", "2.5").unwrap();
    let values: Vec<_> = pp.map(|item| item.unwrap().value).collect();
    let integers: Vec<_> = values
        .iter()
        .filter_map(|value| match value {
            TokenValue::Integer(integer) => Some(integer.value),
            _ => None,
        })
        .collect();
    assert_eq!(integers, [10, 20, 30]);
    assert!(matches!(values.last(), Some(TokenValue::Float(float)) if float.value == 2.5));
    assert!(Preprocessor::new("#if 1.5\n10\n#endif").any(|item| item.is_err()));
}

#[test]
fn float_conditions_do_not_relax_line_directives() {
    let mut pp = Preprocessor::new("#line 1.5\n10");
    pp.set_iris_conditions(true);
    assert!(pp.any(|item| item.is_err()));
}

#[test]
fn condition_nesting_returns_an_error_in_both_modes() {
    for iris in [false, true].iter().copied() {
        for expression in [
            format!("{}1{}", "(".repeat(10_000), ")".repeat(10_000)),
            format!("{}1", "!".repeat(10_000)),
        ]
        .iter()
        {
            let source = format!("#if {}\n1\n#endif", expression);
            let mut pp = Preprocessor::new(&source);
            pp.set_iris_conditions(iris);
            assert!(
                pp.any(|item| matches!(item, Err((PreprocessorError::RecursionLimitReached, _))))
            );
        }
        let source = format!("#if {}1{}\n1\n#endif", "(".repeat(32), ")".repeat(32));
        let mut pp = Preprocessor::new(&source);
        pp.set_iris_conditions(iris);
        assert!(pp.all(|item| item.is_ok()));
    }
    let source = format!("#if {}1\n1\n#endif", "0 ? 1 : ".repeat(10_000));
    let mut pp = Preprocessor::new(&source);
    pp.set_iris_conditions(true);
    assert!(pp.any(|item| matches!(item, Err((PreprocessorError::RecursionLimitReached, _)))));
}

#[test]
fn iris_ternaries_have_conditional_precedence_and_associativity() {
    for &(condition, expected) in [
        ("MC_VERSION >= 12100 ? 1 : 0", 1),
        ("0 ? 2 : 0 ? 3 : 4", 4),
        ("1 ? 0 ? 3 : 4 : 5", 4),
        ("1 || 0 ? 7 : 9", 7),
        ("0 ? 2 : 1.5", 1),
    ]
    .iter()
    {
        let source = format!("#if ({}) == {}\n7\n#else\n9\n#endif", condition, expected);
        let mut pp = Preprocessor::new(&source);
        pp.set_iris_conditions(true);
        pp.add_define("MC_VERSION", "12111").unwrap();
        let values: Vec<_> = pp.map(|item| item.unwrap().value).collect();
        assert!(
            matches!(values.as_slice(), [TokenValue::Integer(value)] if value.value == 7),
            "{:?}",
            values
        );
    }
    let mut pp = Preprocessor::new("#if 0\n9\n#elif 1 ? 1 : 0\n7\n#endif");
    pp.set_iris_conditions(true);
    let values: Vec<_> = pp.map(|item| item.unwrap().value).collect();
    assert!(matches!(values.as_slice(), [TokenValue::Integer(value)] if value.value == 7));
    assert!(Preprocessor::new("#if 1 ? 1 : 0\n7\n#endif").any(|item| item.is_err()));
    for condition in ["1 ? 2", "1 ? : 3", "1 ? 2 3", "1 ? 2 :"].iter() {
        let source = format!("#if {}\n7\n#endif", condition);
        let mut pp = Preprocessor::new(&source);
        pp.set_iris_conditions(true);
        assert!(pp.any(|item| item.is_err()));
    }
}

#[test]
fn condition_negation_overflow_returns_an_error() {
    check_preprocessing_error(
        "#if -(-9223372036854775807 - 1)\n1\n#endif",
        PreprocessorError::IntegerOverflow,
    );
}

#[test]
fn text_macros_keep_invocation_lines_and_property_spelling() {
    let source = "entry=VALUE\n#define ADD(x) x+1\ncalc=ADD(2)\n#define LINE __LINE__\nline=LINE\nliteral=\"https://host/*literal*/\" \\u0041\n";
    let mut pp = Preprocessor::new(source);
    pp.set_preserve_text(true);
    pp.add_define("VALUE", "0x10").unwrap();
    let mut lines = Vec::new();
    let mut output = String::new();
    while let Some(token) = pp.next_text() {
        let token = token.unwrap();
        if token.text == "0x10" || token.text == "2" || token.text == "5" {
            lines.push(token.line);
        }
        output.push_str(&token.text);
    }
    assert_eq!(lines, [1, 3, 5]);
    assert_eq!(
        output,
        "entry=0x10\ncalc=2+1\nline=5\nliteral=\"https://host/*literal*/\" \\u0041\n"
    );
}

#[test]
fn text_literals_are_not_accepted_as_condition_identifiers() {
    for condition in ["\\", "\"literal\"", "1.0.0"] {
        let source = format!("#if {}\nbad=1\n#endif\nafter=ok\n", condition);
        let mut pp = Preprocessor::new(&source);
        pp.set_preserve_text(true);
        pp.set_iris_conditions(true);
        let mut output = String::new();
        let mut errors = 0;
        while let Some(token) = pp.next_text() {
            match token {
                Ok(token) => output.push_str(&token.text),
                Err(_) => errors += 1,
            }
        }
        assert_eq!(errors, 1, "{}", condition);
        assert!(!output.contains("bad="));
        assert!(output.contains("after=ok"));
    }
}

#[test]
fn macro_operands_keep_following_operators() {
    for text_mode in [false, true] {
        for condition in [
            "VALUE / 10000 == 1",
            "VALUE % 10000 == 2111",
            "VALUE * 2 == 24222",
            "VALUE + 1 == 12112",
            "VALUE << 1 == 24222",
        ] {
            let source = format!("#if {}\n1\n#else\n0\n#endif\n", condition);
            let mut pp = Preprocessor::new(&source);
            pp.set_preserve_text(text_mode);
            pp.add_define("VALUE", "12111").unwrap();
            let values: Vec<_> = pp
                .map(|token| token.unwrap().value)
                .filter_map(|value| {
                    if let TokenValue::Integer(value) = value {
                        Some(value.value)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(values, [1], "{}", condition);
        }
    }
}

#[test]
fn generated_lines_keep_original_line_macros_and_explicit_offsets() {
    let mut pp =
        Preprocessor::new("int a = __LINE__;\n\nint b = __LINE__;\n#line 80\nint c = __LINE__;");
    pp.set_source_lines(vec![12, 12, 34, 35, 36]);
    let values: Vec<_> = pp
        .map(Result::unwrap)
        .filter_map(|token| match token.value {
            TokenValue::Integer(value) => Some(value.value),
            _ => None,
        })
        .collect();
    assert_eq!(values, [12, 34, 81]);
}

#[test]
fn nested_macro_arguments_stop_at_the_expansion_limit() {
    let mut input = String::from("#define D(x) x x\n");
    input += &"D(".repeat(24);
    input += "a";
    input += &")".repeat(24);
    let mut pp = Preprocessor::new(&input);
    assert!(pp.any(|item| matches!(item, Err((PreprocessorError::RecursionLimitReached, _)))));
}

#[test]
fn work_limit_bounds_macro_expansion_inside_conditions() {
    let mut input = String::from("#define M0 0\n");
    for level in 1..=40 {
        input += &format!("#define M{level} M{p} + M{p}\n", p = level - 1);
    }
    input += "#if M40\n1\n#endif\n";
    let mut pp = Preprocessor::new(&input);
    pp.set_work_limit(100_000);
    assert!(pp.any(|item| matches!(item, Err((PreprocessorError::RecursionLimitReached, _)))));
}

#[test]
fn work_limit_is_unlimited_by_default() {
    let mut pp = Preprocessor::new("#define A 1 + 1\n#if A\n2\n#endif\n");
    assert!(pp.all(|item| item.is_ok()));
}

#[test]
fn nested_expansion_errors_report_the_invocation_line() {
    let mut pp = Preprocessor::new("#define ID(x) x\n#define BAD ID(1,2)\nvalue BAD\n");
    pp.set_preserve_text(true);
    let error = loop {
        match pp.next_text() {
            Some(Err((error, location))) => break (error, location.line()),
            Some(Ok(_)) => continue,
            None => panic!("expected an expansion error"),
        }
    };
    assert_eq!(error, (PreprocessorError::TooManyDefineArguments, 3));
}

#[test]
fn inactive_groups_accept_nonconditional_payloads() {
    for preserve in [false, true].iter().copied() {
        for payload in [
            "# error \"x\"",
            "#error It's x",
            "# error \"unclosed",
            "#unknown @ ` $",
            "unbalanced ' \" text",
        ]
        .iter()
        {
            let source = format!(
                "#if 0\n{}\n#if @ invalid\n#error \"nested\n#endif\n#endif\n1\n",
                payload
            );
            let mut pp = Preprocessor::new(&source);
            pp.set_preserve_text(preserve);
            let values: Vec<_> = pp
                .map(|token| token.unwrap().value)
                .filter_map(|value| match value {
                    TokenValue::Integer(value) => Some(value.value),
                    _ => None,
                })
                .collect();
            assert_eq!(values, [1], "{}", payload);
        }
    }
}

#[test]
fn preserved_glsl_keeps_active_directive_comments_and_splices() {
    let source = "#version 450\n#define FLAG /* kept */\n#if 0 /* hidden */\n#error It's bad\n#else /* hidden too */\n#define VALUE 2 /* value */\n// ignored \\\n#error hidden\nint x = VALUE;\n#endif /* active */\n";
    let mut pp = Preprocessor::new(source);
    pp.set_preserve_glsl_text();
    let mut comments = Vec::new();
    let mut values = Vec::new();
    while let Some(token) = pp.next_text() {
        let token = token.unwrap();
        if token.text.starts_with("/*") {
            comments.push(token.text.clone());
        }
        if let Some(TokenValue::Integer(value)) = token.value {
            values.push(value.value);
        }
    }
    assert_eq!(comments, ["/* kept */", "/* value */", "/* active */"]);
    assert_eq!(values, [2]);
    let mut invalid = Preprocessor::new("float x = 'bad;\n");
    invalid.set_preserve_glsl_text();
    assert!(invalid.any(|token| token.is_err()));
}

#[test]
fn iris_conditional_recovery_matches_jcpp() {
    for (source, expected) in [
        ("#define EMPTY\n#if EMPTY\nyes\n#else\nno\n#endif\n", "no"),
        (
            "#define ON\n#ifdef ON && OFF\nyes\n#else\nno\n#endif\n",
            "yes",
        ),
        ("#ifdef\nfirst\nsecond\n#else\nno\n#endif\n", "second"),
        ("#ifndef\nfirst\nsecond\n#else\nno\n#endif\n", "second"),
        ("#if 0\n#ifdef\nno\n#endif\n#endif\nyes\n", "yes"),
    ] {
        let mut pp = Preprocessor::new(source);
        pp.set_iris_conditions(true);
        pp.set_iris_recovery(true);
        let tokens = pp.collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(tokens.len(), 1, "{}", source);
        assert!(
            matches!(&tokens[0].value, TokenValue::Ident(name) if name == expected),
            "{:?}",
            tokens
        );
        assert!(
            Preprocessor::new(source)
                .collect::<Result<Vec<_>, _>>()
                .is_err()
                || source.starts_with("#if 0")
        );
    }
}

#[test]
fn iris_recovery_accepts_unclosed_conditionals_only_when_requested() {
    for (source, expected) in [
        ("#if 1\nyes\n", Some("yes")),
        ("#if 0\nno\n", None),
        ("#ifdef\n// discarded\nyes\n", Some("yes")),
    ] {
        let mut pp = Preprocessor::new(source);
        pp.set_iris_recovery(true);
        let tokens = pp.collect::<Result<Vec<_>, _>>().unwrap();
        let names = tokens
            .iter()
            .filter_map(|t| match t.value {
                TokenValue::Ident(ref name) => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, expected.into_iter().collect::<Vec<_>>());
        let mut strict = Preprocessor::new(source);
        strict.set_iris_conditions(true);
        assert!(strict.collect::<Result<Vec<_>, _>>().is_err());
    }
}
