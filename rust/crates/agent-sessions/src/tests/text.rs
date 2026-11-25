//! Index terms and search queries.

use crate::text::{index_terms, match_expression, MAX_QUERY_TERMS};

#[test]
fn unspaced_runs_become_pairs_and_a_last_character() {
    assert_eq!(index_terms("今天去公园 ABC"), "今天 天去 去公 公园 园 abc");
    assert_eq!(index_terms("中"), "中");
    assert_eq!(index_terms("こんにちは"), "こん んに にち ちは は");
    assert_eq!(index_terms("한국어"), "한국 국어 어");
    assert_eq!(index_terms("用Rust写代码"), "用 rust 写代 代码 码");
    assert_eq!(index_terms(""), "");
    assert_eq!(index_terms("..., !!!"), "");
}

#[test]
fn queries_match_pairs_exactly_and_words_as_prefixes() {
    assert_eq!(
        match_expression("数据库").as_deref(),
        Some(r#""数据" "据库""#)
    );
    assert_eq!(match_expression("中").as_deref(), Some(r#""中"*"#));
    assert_eq!(
        match_expression("Connect to DB").as_deref(),
        Some(r#""connect"* "to"* "db"*"#)
    );
    assert_eq!(match_expression("db DB Db").as_deref(), Some(r#""db"*"#));
    assert_eq!(match_expression("!!!"), None);
    assert_eq!(match_expression(""), None);
}

#[test]
fn query_syntax_never_reaches_the_index() {
    assert_eq!(
        match_expression(r#"foo" OR bar*"#).as_deref(),
        Some(r#""foo"* "or"* "bar"*"#)
    );
    assert_eq!(
        match_expression("NEAR(a b) -c ^d col:e").as_deref(),
        Some(r#""near"* "a"* "b"* "c"* "d"* "col"* "e"*"#)
    );
    assert_eq!(
        match_expression("snake_case").as_deref(),
        Some(r#""snake"* "case"*"#)
    );
}

#[test]
fn long_words_are_cut_at_a_character_boundary() {
    assert_eq!(index_terms(&"a".repeat(100)), "a".repeat(64));
    assert_eq!(index_terms(&"é".repeat(40)), "é".repeat(32));
    // Query words are cut the same way, so they still match the index.
    assert_eq!(
        match_expression(&"a".repeat(100)),
        Some(format!("\"{}\"*", "a".repeat(64)))
    );
}

#[test]
fn a_query_keeps_its_first_terms() {
    let words: Vec<String> = (0..40).map(|number| format!("w{number}")).collect();
    let expression = match_expression(&words.join(" ")).expect("a query");
    let terms: Vec<&str> = expression.split(' ').collect();
    assert_eq!(terms.len(), MAX_QUERY_TERMS);
    assert_eq!(terms[0], r#""w0"*"#);
    assert_eq!(
        terms[MAX_QUERY_TERMS - 1],
        format!("\"w{}\"*", MAX_QUERY_TERMS - 1)
    );
}
