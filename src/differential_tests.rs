use crate::lexer::{Lexer, TextMode, TokenValue as LexicalValue};
use crate::pp::{convert_lexer_token, text::Output, Preprocessor};
use crate::token::{PreprocessorError, TokenValue};
use alloc::{format, string::String, vec::Vec};

/// Produces a repeatable choice without a random-number dependency.
fn choose<T: Copy>(state: &mut u64, choices: &[T]) -> T {
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

#[test]
fn generated_glsl_sequences_keep_tokens_after_rendering() {
    let mut state = 0x474c_534c_2026_0009;
    let atoms = [
        "x",
        "name",
        "_",
        "e",
        "E",
        "L",
        "f",
        "0",
        "2",
        "0x1E",
        "1u",
        "1e",
        "1E",
        "1e+",
        "1e-",
        "1.e",
        ".1E",
        "1e2",
        "0.5",
        ".5",
        "+",
        "-",
        "*",
        "/",
        ".",
        "=",
        "<",
        ">",
        "<<",
        ">>",
        "&",
        "|",
        "^",
        "!",
        ";",
        "ID(1e)",
        "ID(/)",
        "ID(x)",
        "E()",
        "EMPTY",
        "JOIN(1e,+)",
        "JOIN(/,*)",
        "JOIN(x,y)",
        "JOIN(,x)",
        "JOIN(x,)",
    ];
    for _ in 0..10_000 {
        let replacement = choose(
            &mut state,
            &[
                "1e", "1E", "1e+", "1e-", ".1E", "x", "2", "+", "-", "*", "/",
            ],
        );
        let mut source = format!(
            "#define ID(x) x\n#define JOIN(a,b) a b\n#define EMPTY\n#define E() {}\n",
            replacement
        );
        let length = choose(&mut state, &[2, 4, 8, 16]);
        for _ in 0..length {
            source.push_str(choose(&mut state, &atoms));
            source.push_str(choose(
                &mut state,
                &[
                    "",
                    " ",
                    "\t",
                    "\n",
                    "\\\n",
                    "/**/",
                    "/* *\\\\\n\n/ hidden */",
                    "/* /\\\n* tail */",
                    "/* \\\\\r\r\n * / */",
                    "// *\\\\\n\n",
                    "// \\\\\r\n\n",
                ],
            ));
        }
        source.push_str(";\n");
        check_glsl(&source, "1e");
    }
}

#[test]
fn generated_glsl_token_and_comment_joints_keep_tokens() {
    let mut state = 0x4a4f_494e_2026_0009;
    // Independent fragments model tokens placed together after expansion, pasting or recovery.
    let fragments = [
        "x",
        "_",
        "E",
        "L",
        "0",
        "2",
        "1u",
        "0x1E",
        "1e",
        "1E",
        "1e+",
        "1e-",
        "1.e",
        ".1E",
        ".5",
        "+",
        "-",
        "*",
        "/",
        ".",
        "=",
        "<",
        ">",
        "<<",
        ">>",
        "&",
        "|",
        "^",
        "!",
        ";",
        " ",
        "\t",
        "\n",
        "\\\n",
        "/**/",
        "/* *\\\\\n\n/ hidden */",
        "/* \\\\\r\r\n * / */",
        "// *\\\\\n\n",
        "// \\\\\r\n\n",
    ];
    for _ in 0..10_000 {
        let mut output = Output::default();
        let mut text = String::new();
        let mut expected = Vec::new();
        let mut input = Vec::new();
        let length = choose(&mut state, &[2, 4, 8, 16]);
        for _ in 0..length {
            let fragment = choose(&mut state, &fragments);
            input.push(fragment);
            let mut lexer = Lexer::new(fragment);
            lexer.set_text_mode(TextMode::Glsl);
            for token in lexer {
                let token = token.unwrap();
                if token.value == LexicalValue::NewLine
                    && token.location.start == token.location.end
                {
                    continue;
                }
                let token = convert_lexer_token(token).unwrap();
                if !token.trivia {
                    expected.push(token.value.clone());
                }
                text.push_str(&output.render(token, TextMode::Glsl).text);
            }
        }
        assert!(!text.contains("\\\n"), "{:?}: {:?}", input, text);
        let downstream = downstream_glsl(&text);
        let actual: Vec<_> = Lexer::new(&downstream)
            .map(Result::unwrap)
            .filter(|token| token.value != LexicalValue::NewLine)
            .map(|token| convert_lexer_token(token).unwrap().value)
            .collect();
        assert_eq!(actual, expected, "{:?}: {:?}", input, text);
    }
}
