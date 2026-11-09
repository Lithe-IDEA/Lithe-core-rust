//! Checks actual Rust comment tokens without treating literals as comments.

use regex::Regex;
use rustc_lexer::{tokenize, TokenKind};
use std::{env, fs, process::ExitCode};

fn non_english_comments<'a>(source: &'a str, han: &Regex) -> Vec<(usize, &'a str)> {
    let mut found = Vec::new();
    let mut offset = 0;
    let mut line = 1;
    for token in tokenize(source) {
        let text = &source[offset..offset + token.len];
        if matches!(
            token.kind,
            TokenKind::LineComment | TokenKind::BlockComment { .. }
        ) && han.is_match(text)
        {
            found.push((line, text));
        }
        line += text.bytes().filter(|byte| *byte == b'\n').count();
        offset += token.len;
    }
    found
}

fn main() -> ExitCode {
    let han = Regex::new(r"\p{Han}").expect("fixed Unicode script expression");
    let paths: Vec<_> = env::args_os().skip(1).collect();
    if paths.is_empty() {
        eprintln!("Rust comment verification requires at least one source file");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for path in paths {
        match fs::read_to_string(&path) {
            Ok(source) => {
                for (line, comment) in non_english_comments(&source, &han) {
                    eprintln!(
                        "{}:{line}: Rust comments must be English: {comment}",
                        path.to_string_lossy()
                    );
                    failed = true;
                }
            }
            Err(error) => {
                eprintln!(
                    "{}: could not read Rust source: {error}",
                    path.to_string_lossy()
                );
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_with_comment_delimiters_do_not_hide_following_comments() {
        let source = r###"let glob = "projects/*/repos/\n晚到目录/\n";
let path = r##"中文 // /* " still a raw string"##;
let bytes = br#"/* raw byte string */"#;
// English comment.
let kept = "中文"; // 中文注释
"###;
        let han = Regex::new(r"\p{Han}").unwrap();
        assert_eq!(non_english_comments(source, &han), vec![(5, "// 中文注释")]);
    }

    #[test]
    fn nested_and_inline_block_comments_are_checked() {
        let source = "let a = 1; /* outer /* 中文 */ still outer */\n//! 中文文档\n";
        let han = Regex::new(r"\p{Han}").unwrap();
        assert_eq!(
            non_english_comments(source, &han),
            vec![
                (1, "/* outer /* 中文 */ still outer */"),
                (2, "//! 中文文档"),
            ]
        );
    }

    #[test]
    fn escaped_quotes_and_character_literals_preserve_token_boundaries() {
        let source = r###"let quote = '\''; let text = "\" // 中文文本"; /* English */
fn borrowed<'a>(x: &'a str) {} // 中文注释
"###;
        let han = Regex::new(r"\p{Han}").unwrap();
        assert_eq!(non_english_comments(source, &han), vec![(2, "// 中文注释")]);
    }
}
