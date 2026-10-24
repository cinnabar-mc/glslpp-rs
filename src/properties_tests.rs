use crate::pp::Preprocessor;
use alloc::string::String;

/// A prepared properties input, environment, expected text, and registration outcome.
type Case<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], &'a str, bool);

#[test]
fn properties_golden_corpus() {
    let cases: &[Case<'_>] = &[
        ("skipped_double_quote", "#if 0\nvalue=\"/*\"\n#endif\nvisible=1\n", &[], "\n\n\nvisible=1\n", false),
        ("skipped_single_quote", "#if 0\nvalue='/*'\n#endif\nvisible=1\n", &[], "\n\n\nvisible=1\n", false),
        ("skipped_unclosed_quote", "#if 0\nvalue=\"/*\n#endif\nvisible=1\n", &[], "\n\n\nvisible=1\n", false),
        ("nested_skipped_quotes", "#if 0\n#if 1\nvalue=\"/*\"\n#endif\n#else\nvisible=1\n#endif\n", &[], "\n\n\n\n\nvisible=1\n\n", false),
        ("directive_lines", "#define A -1\n#if A < 0\nvalue=A\n#else\nhidden=1\n#endif\n", &[], "\n\nvalue=-1\n\n\n\n", false),
        ("environment_line_comment", "value=ENV+2\nnested=ID(ENV)\n#define ID(x) x\nnext=ID(ENV)\n", &[("ENV", "1 // note")], "value=1 // note+2\nnested=ID(1 // note)\n\nnext=1\n", false),
        ("environment_unclosed_comment", "#define ID(x) x\nvalue=ID(ENV)\n", &[("ENV", "1 /* unclosed")], "\nvalue=1 /* unclosed\n", false),
        ("environment_closed_comment", "#define ID(x) x\nvalue=ID(ENV)\n", &[("ENV", "1 /* closed */")], "\nvalue=1\n", false),
        ("environment_whitespace", "#define ID(x) x\nvalue=ENV\nnested=ID(ENV)\n", &[("ENV", " -1  \tfoo ")], "\nvalue= -1  \tfoo \nnested=-1 foo\n", false),
        ("source_comments", "value=a/* first\nsecond */b\nnext=a// tail\n", &[], "value=a/* first\nsecond */b\nnext=a// tail\n", false),
        ("skipped_comments", "#if 0\nhidden=a/* first\nsecond */b\n#endif\nvisible=1\n", &[], "\n\n\nvisible=1\n", false),
        ("quoted_comments", "#define A 7\nvalue=\"A /* literal */\"\nnext='A // literal'\nafter=A\n", &[], "\nvalue=\"A /* literal */\"\nnext='A // literal'\nafter=7\n", false),
        ("unclosed_quotes", "#define A 7\nvalue=\"A\nnext=A\nother='A\nafter=A\n", &[], "\nvalue=\"A\nnext=7\nother='A\nafter=7\n", false),
        ("quoted_macro_arguments", "#define ID(x) x\nvalue=ID(\"a,b\")\nother=ID('/*')\n", &[], "\nvalue=\"a,b\"\nother='/*'\n", false),
        ("grouped_arguments", "#define ID(x) x\n#define PAIR(a,b) a + b\nvalue=ID(a ( b , c ))\nother=PAIR(a/*c*/, b)\n", &[], "\n\nvalue=a( b, c)\nother=a + b\n", false),
        ("literal_joints", "#define A() foo\n#define N() 1\n#define EMPTY\nvalue=A()bar\nnumber=N().0\ncomment=/EMPTY*2\n", &[], "\n\n\nvalue=foobar\nnumber=1.0\ncomment=/*2\n", false),
        ("number_spelling", "#define A -1\nvalue=A\ninvalid=09\nfloat=1.0.0\nhex=0xG\nexponent=1e-2\n", &[], "\nvalue=-1\ninvalid=09\nfloat=1.0.0\nhex=0xG\nexponent=1e-2\n", false),
        ("numeric_macro_suffixes", "#define A 7\n#define ID(x) x\nvalue=1A\nother=1ENV\ncall=1ID(A)\n", &[("ENV", "-2")], "\n\nvalue=1A\nother=1ENV\ncall=1ID(7)\n", false),
        ("paths", "#define A textures/foo.png\n#define FILE(x) textures/x.png\nvalue=A\nother=FILE(test)\n", &[], "\n\nvalue=textures/foo.png\nother=textures/test.png\n", false),
        ("continuations", "#define A 7\nvalue=A \\\nnext=A\nquoted=\"A\\\nlast=A\n", &[], "\nvalue=7 \\\nnext=7\nquoted=\"A\\\nlast=7\n", false),
        ("hashes_and_blanks", "\nvalue=foobar\nnext=1\n", &[], "\nvalue=foobar\nnext=1\n", false),
        ("environment_bad_octal", "value=ENV\n", &[("ENV", "09f")], "", true),
    ];
    for (name, source, environment, expected, invalid_environment) in cases {
        // The caller shields physical backslashes before preprocessing properties.
        let source = source.replace('\\', "PROPERTIES_BACKSLASH");
        let mut pp = Preprocessor::new(&source);
        pp.set_preserve_text(true);
        let mut rejected = false;
        for (key, value) in *environment {
            if pp.add_define(key, value).is_err() {
                rejected = true;
                break;
            }
        }
        assert_eq!(rejected, *invalid_environment, "{}", name);
        if rejected {
            continue;
        }
        let mut actual = String::new();
        while let Some(token) = pp.next_text() {
            actual.push_str(&token.unwrap().text);
        }
        assert_eq!(
            actual.replace("PROPERTIES_BACKSLASH", "\\"),
            *expected,
            "{}",
            name
        );
    }
}
