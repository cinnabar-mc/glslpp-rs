use crate::lexer::{Lexer, TokenValue as LexicalValue};
use crate::pp::{convert_lexer_token, Preprocessor};
use crate::token::{PreprocessorError, TokenValue};
use alloc::{format, string::String, vec::Vec};

/// Produces a repeatable choice without a random-number dependency.
fn choose<'a>(state: &mut u64, choices: &'a [&str]) -> &'a str {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    choices[*state as usize % choices.len()]
}

/// Collects semantic values, including directive metadata, and optionally renders source.
fn preprocess(
    source: &str,
    environment: &str,
    preserve: bool,
) -> (String, Result<Vec<TokenValue>, PreprocessorError>) {
    let mut pp = Preprocessor::new(source);
    if preserve {
        pp.set_preserve_glsl_text();
    }
    pp.set_work_limit(100_000);
    if let Err((error, _)) = pp.add_define("ENV", environment) {
        return (String::new(), Err(error));
    }
    let mut text = String::new();
    let mut values = Vec::new();
    if preserve {
        while let Some(token) = pp.next_text() {
            match token {
                Ok(token) => {
                    text.push_str(&token.text);
                    values.extend(token.value);
                }
                Err((error, _)) => return (text, Err(error)),
            }
        }
    } else {
        for token in pp {
            match token {
                Ok(token) => values.push(token.value),
                Err((error, _)) => return (text, Err(error)),
            }
        }
    }
    (text, Ok(values))
}

/// Applies downstream newline normalization and one nonrecursive line-splicing pass.
pub(super) fn downstream_glsl(source: &str) -> String {
    let mut input = source.chars().peekable();
    let mut normalized = String::new();
    while let Some(ch) = input.next() {
        if matches!(ch, '\r' | '\n') {
            if input
                .peek()
                .is_some_and(|next| matches!((ch, *next), ('\r', '\n') | ('\n', '\r')))
            {
                input.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(ch);
        }
    }
    let mut chars = normalized.chars().peekable();
    let mut result = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'\n') {
            chars.next();
        } else {
            result.push(ch);
        }
    }
    result
}

/// Compares rendered text with strict lexing and metadata with the ordinary token API.
fn check_glsl(source: &str, environment: &str) {
    let (_, expected) = preprocess(source, environment, false);
    let (text, actual) = preprocess(source, environment, true);
    assert_eq!(
        actual, expected,
        "source: {:?}, env: {:?}",
        source, environment
    );
    if let Ok(values) = expected {
        let expected: Vec<_> = values
            .into_iter()
            .filter(|value| {
                !matches!(
                    value,
                    TokenValue::Version(_) | TokenValue::Extension(_) | TokenValue::Pragma(_)
                )
            })
            .collect();
        let downstream = downstream_glsl(&text);
        let actual: Vec<_> = Lexer::new(&downstream)
            .filter_map(|token| {
                let token = token.unwrap_or_else(|error| panic!("{:?}: {:?}", text, error));
                if token.value == LexicalValue::NewLine {
                    None
                } else {
                    Some(convert_lexer_token(token).unwrap().value)
                }
            })
            .collect();
        assert_eq!(
            actual, expected,
            "source: {:?}, env: {:?}, text: {:?}",
            source, environment, text
        );
    }
}

#[test]
fn generated_glsl_matches_strict_tokens_and_directive_metadata() {
    let mut state = 0x474c_534c_2026_0006;
    for _ in 0..100_000 {
        let slashes = choose(&mut state, &["\\", "\\\\", "\\\\\\", "\\\\\\\\"]);
        let following = choose(&mut state, &["\n", "\n\n", "continued\n", "\\\n\n"]);
        let comment = format!("//before{}\n{}int visible;\n", slashes, following);
        let prefixes = ["", "/*before*/", &comment, "\t"];
        let prefix = choose(&mut state, &prefixes);
        let metadata = choose(
            &mut state,
            &[
                "",
                "#version 450\n",
                "#extension GL_EXT_test : enable\n",
                "#pragma optimize(on)\n",
                "#line 37\n__LINE__;\n",
            ],
        );
        let replacement = choose(
            &mut state,
            &[
                "-1",
                "1.0",
                "/",
                "+",
                "foo",
                "1e-2",
                "0x1E",
                "",
                "1 /*c*/ + 2",
                "1 // tail",
                "1 /* split\ncomment */ + 2",
                "ENV",
                "ID(-1)",
            ],
        );
        let left = choose(
            &mut state,
            &[
                "A", "ID(A)", "ENV", "ID(ENV)", "EMPTY", "/EMPTY", "foo", "1", "1.", ".", "+", "<",
                "<<", "&", "!", "-", "/*c*/A",
            ],
        );
        let right = choose(
            &mut state,
            &[
                "A",
                "ID(A)",
                "ENV",
                "EMPTY",
                "foo",
                "L",
                ".0",
                "e",
                "+2",
                "*2",
                "/2",
                "=2",
                "<2",
                "&2",
                "//tail\n2",
                "/*c*/2",
            ],
        );
        let gap = choose(&mut state, &["", " ", "\t", "/*gap*/", "\\\n", "\n"]);
        let skipped = choose(
            &mut state,
            &[
                "",
                "#if 0\n@ 'unclosed\n#endif\n",
                "#if 0\n\"/*\"\n#endif\n",
                "#if 1\n#else\n#error ignored\n#endif\n",
                "#if 0\n#if 1\n@ bad\n#endif\n#else\n#endif\n",
                "#ifdef ENV\n__LINE__;\n#endif\n",
            ],
        );
        let environment = choose(
            &mut state,
            &[
                " -1  \tfoo ",
                "1 // note",
                "1 /* note */ + 2",
                "1 /*",
                "/",
                "+",
                "foo",
                "",
                "0.50",
                "foo\nbar",
                "1 /\\\n/ note",
                "1 // note\n + 2",
                "/**/",
            ],
        );
        let source = format!(
            "{}{}#define A {}\n#define ID(x) x\n#define EMPTY\n{}{}{}{};\n",
            prefix, metadata, replacement, skipped, left, gap, right
        );
        check_glsl(&source, environment);
    }
}
