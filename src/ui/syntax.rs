//! Syntax colours for the code the detail panes show: CloudFormation
//! templates, policies, state machine definitions, user data. Styling
//! only. The rows stay plain text, so copy, export, `/` and the flat view
//! see exactly what they did before.
//!
//! Hand-rolled lexers for the three languages AWS hands back, rather than a
//! grammar crate: colours come from the runtime palette (so every theme
//! preset works), and the CloudFormation intrinsics (`!Ref`, `Fn::GetAtt`,
//! `${AWS::Region}`), which say what a template wires to what, get a colour
//! of their own. A line the lexer doesn't understand degrades to plain text,
//! never to garbage.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::ui::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Json,
    Yaml,
    Shell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Plain,
    Key,
    Str,
    /// Numbers, `true` / `false` / `null`.
    Literal,
    /// CloudFormation intrinsics and YAML tags; shell keywords.
    Intrinsic,
    /// Shell `$VAR` / `${…}`.
    Var,
    Comment,
    Punct,
}

fn style(tok: Tok) -> Style {
    let fg = |c| Style::default().fg(c);
    match tok {
        Tok::Plain => fg(theme::text_primary()),
        // The same colour as a key-value row's key.
        Tok::Key | Tok::Var => fg(theme::accent()),
        Tok::Str => fg(theme::success()),
        Tok::Literal => fg(theme::warning()),
        Tok::Intrinsic => fg(theme::heading()),
        Tok::Comment => fg(theme::text_dim()).add_modifier(Modifier::ITALIC),
        Tok::Punct => fg(theme::text_muted()),
    }
}

/// Which language a block of lines is written in, or `None` to leave it
/// alone. Only claims a block that's unmistakable: JSON by its opening
/// bracket, shell by its `#!sh` line, YAML by a leading `key:` and a
/// majority of mapping / list lines (a list of ARNs or a rule table is
/// neither, so it stays plain).
pub fn detect(lines: &[&str]) -> Option<Lang> {
    let first = lines.iter().map(|l| l.trim()).find(|l| !l.is_empty())?;
    if let Some(interp) = first.strip_prefix("#!") {
        return is_shell(interp).then_some(Lang::Shell);
    }
    if first.starts_with("#cloud-config") || first == "---" || first.starts_with("%YAML") {
        return Some(Lang::Yaml);
    }
    if first == "{"
        || first == "["
        || ["{\"", "[\"", "[{", "[[", "{}", "[]"].iter().any(|p| first.starts_with(p))
    {
        return Some(Lang::Json);
    }
    looks_like_yaml(lines).then_some(Lang::Yaml)
}

/// `#!/bin/bash -xe`, `#!/usr/bin/env sh` → true; `#!/usr/bin/env python3`
/// → false (the shell lexer would misread it).
fn is_shell(interp: &str) -> bool {
    let mut parts = interp.split_whitespace();
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let mut prog = parts.next().map(base).unwrap_or_default();
    if prog == "env" {
        prog = parts.next().map(base).unwrap_or_default();
    }
    matches!(prog.as_str(), "sh" | "bash" | "zsh" | "ksh" | "dash" | "ash")
}

fn looks_like_yaml(lines: &[&str]) -> bool {
    let mut block: Option<usize> = None;
    let (mut meaningful, mut yamlish) = (0usize, 0usize);
    let mut first_is_key = None;
    for line in lines {
        let indent = indent_of(line);
        let t = line.trim();
        if let Some(parent) = block {
            if t.is_empty() || indent > parent {
                if !t.is_empty() {
                    meaningful += 1;
                    yamlish += 1;
                }
                continue;
            }
            block = None;
        }
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        meaningful += 1;
        let (rest, item) = strip_dashes(t);
        let key = key_len(rest);
        first_is_key.get_or_insert(key.is_some());
        if key.is_some() || item {
            yamlish += 1;
        }
        let value = match key {
            Some(k) => rest[k..].split_once(':').map_or("", |(_, v)| v),
            None => rest,
        };
        if opens_block(value) {
            block = Some(indent);
        }
    }
    first_is_key == Some(true) && meaningful >= 2 && yamlish * 5 >= meaningful * 3
}

/// Colour each line of a block. Lines are lexed in order because YAML
/// block scalars (`|`, `>`) carry over from one line to the next.
pub fn highlight(lang: Lang, lines: &[&str]) -> Vec<Vec<Span<'static>>> {
    lex(lang, lines)
        .into_iter()
        .map(|toks| toks.into_iter().map(|(tok, text)| Span::styled(text, style(tok))).collect())
        .collect()
}

fn lex(lang: Lang, lines: &[&str]) -> Vec<Vec<(Tok, String)>> {
    let mut yaml = YamlState::default();
    lines
        .iter()
        .map(|line| {
            let mut out = Out::default();
            match lang {
                Lang::Json => json_line(line, &mut out),
                Lang::Yaml => yaml_line(line, &mut yaml, &mut out),
                Lang::Shell => shell_line(line, &mut out),
            }
            out.finish()
        })
        .collect()
}

/// Collects a line's tokens, merging neighbours of the same kind.
#[derive(Default)]
struct Out {
    toks: Vec<(Tok, String)>,
    tok: Option<Tok>,
    buf: String,
}

impl Out {
    fn push(&mut self, tok: Tok, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.tok != Some(tok) {
            self.flush();
            self.tok = Some(tok);
        }
        self.buf.push_str(text);
    }

    /// `text` as `tok`, with each `${…}` substitution as an intrinsic.
    fn push_subs(&mut self, tok: Tok, text: &str) {
        let mut rest = text;
        while let Some(a) = rest.find("${") {
            let Some(len) = rest[a..].find('}') else { break };
            self.push(tok, &rest[..a]);
            self.push(Tok::Intrinsic, &rest[a..a + len + 1]);
            rest = &rest[a + len + 1..];
        }
        self.push(tok, rest);
    }

    fn flush(&mut self) {
        if let Some(tok) = self.tok.take() {
            self.toks.push((tok, std::mem::take(&mut self.buf)));
        }
    }

    fn finish(mut self) -> Vec<(Tok, String)> {
        self.flush();
        self.toks
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Byte index just past the char starting at `i` (which may be past the
/// end). Keeps an escape's skipped char whole when it isn't ASCII.
fn next_char(s: &str, i: usize) -> usize {
    s.get(i..).and_then(|r| r.chars().next()).map_or(s.len(), |c| i + c.len_utf8())
}

/// End of a quoted string starting at `i` (`s[i]` is the quote): just past
/// the closing quote, or the end of the line if it never closes. `\`
/// escapes in double quotes; `''` is an escaped quote in single ones.
fn quoted_end(s: &str, i: usize) -> usize {
    let b = s.as_bytes();
    let q = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' if q == b'"' => j = next_char(s, j + 1),
            c if c == q => {
                if q == b'\'' && b.get(j + 1) == Some(&b'\'') {
                    j += 2;
                    continue;
                }
                return j + 1;
            }
            _ => j += 1,
        }
    }
    b.len()
}

fn is_number(w: &str) -> bool {
    let digits = w.strip_prefix(['-', '+']).unwrap_or(w);
    digits.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && (digits.parse::<f64>().is_ok()
            || digits.strip_prefix("0x").is_some_and(|h| u64::from_str_radix(h, 16).is_ok()))
}

fn is_literal(w: &str) -> bool {
    is_number(w) || ["true", "false", "null", "~"].iter().any(|l| w.eq_ignore_ascii_case(l))
}

/// `Ref` and the `Fn::` family: the keys that make a JSON or YAML object
/// an intrinsic function call.
fn is_intrinsic_key(key: &str) -> bool {
    key == "Ref" || key.starts_with("Fn::")
}

// ── JSON ─────────────────────────────────────────────────────────────────

fn json_line(line: &str, out: &mut Out) {
    let b = line.as_bytes();
    let mut i = 0;
    let punct = |c: u8| matches!(c, b'{' | b'}' | b'[' | b']' | b',' | b':');
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => {
                let s = i;
                while i < b.len() && matches!(b[i], b' ' | b'\t') {
                    i += 1;
                }
                out.push(Tok::Plain, &line[s..i]);
            }
            b'"' => {
                let s = i;
                i = quoted_end(line, i);
                let text = &line[s..i];
                let is_key = line[i..].trim_start().starts_with(':');
                if is_key {
                    let bare = text.trim_matches('"');
                    out.push(if is_intrinsic_key(bare) { Tok::Intrinsic } else { Tok::Key }, text);
                } else {
                    out.push_subs(Tok::Str, text);
                }
            }
            c if punct(c) => {
                out.push(Tok::Punct, &line[i..i + 1]);
                i += 1;
            }
            _ => {
                let s = i;
                while i < b.len() && !punct(b[i]) && !matches!(b[i], b' ' | b'\t' | b'"') {
                    i += 1;
                }
                let word = &line[s..i];
                out.push(if is_literal(word) { Tok::Literal } else { Tok::Plain }, word);
            }
        }
    }
}

// ── YAML ─────────────────────────────────────────────────────────────────

#[derive(Default)]
struct YamlState {
    /// Inside a `|` / `>` block scalar opened on a line with this indent:
    /// every following line indented deeper (or blank) is its text.
    block: Option<usize>,
}

/// Split leading sequence dashes off (`- - a` → `a`), and whether there
/// were any.
fn strip_dashes(mut t: &str) -> (&str, bool) {
    let mut item = false;
    while t == "-" || t.starts_with("- ") {
        item = true;
        t = t[1..].trim_start();
    }
    (t, item)
}

/// Length of the mapping key at the start of `t` (not counting its `:`), if
/// `t` starts with one: a quoted string, or a run with no spaces, followed
/// by `:` and a space or the end of the line. So `Fn::GetAtt:` is a key and
/// `arn:aws:s3:::bucket` and `https://…` are not.
fn key_len(t: &str) -> Option<usize> {
    let b = t.as_bytes();
    let colon_at = |j: usize| b.get(j) == Some(&b':') && matches!(b.get(j + 1), None | Some(b' '));
    match *b.first()? {
        b'"' | b'\'' => {
            let end = quoted_end(t, 0);
            let j = end + (t[end..].len() - t[end..].trim_start_matches(' ').len());
            (end <= b.len() && colon_at(j)).then_some(end)
        }
        c if c.is_ascii_alphanumeric() || c == b'_' => {
            for (i, &c) in b.iter().enumerate() {
                if c == b' ' || c == b'\t' {
                    return None;
                }
                if colon_at(i) {
                    return Some(i);
                }
            }
            None
        }
        _ => None,
    }
}

/// Split a trailing ` # comment` off a value.
fn split_comment(v: &str) -> (&str, &str) {
    let b = v.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'#' && (i == 0 || b[i - 1] == b' ') {
            return (&v[..i], &v[i..]);
        }
    }
    (v, "")
}

/// Leading `!Tag`s off a value: (`"!Sub "`, `"|"`) for `!Sub |`.
fn split_tags(v: &str) -> (&str, &str) {
    let mut rest = v.trim_start();
    while rest.starts_with('!') {
        let end = rest.find(' ').unwrap_or(rest.len());
        rest = rest[end..].trim_start();
    }
    (&v[..v.len() - rest.len()], rest)
}

/// Whether a mapping value or list item opens a block scalar: `|`, `>-`,
/// `|2+`, optionally after tags (`!Sub |`) and before a comment.
fn opens_block(value: &str) -> bool {
    let (_, rest) = split_tags(value);
    let head = split_comment(rest).0.trim_end();
    let mut chars = head.chars();
    matches!(chars.next(), Some('|' | '>')) && chars.all(|c| matches!(c, '-' | '+' | '0'..='9'))
}

fn yaml_line(line: &str, st: &mut YamlState, out: &mut Out) {
    let indent = indent_of(line);
    if let Some(parent) = st.block {
        if line.trim().is_empty() || indent > parent {
            out.push(Tok::Str, line);
            return;
        }
        st.block = None;
    }
    out.push(Tok::Plain, &line[..indent]);
    let mut rest = &line[indent..];
    if rest.starts_with('#') {
        out.push(Tok::Comment, rest);
        return;
    }
    if rest == "---" || rest == "..." {
        out.push(Tok::Punct, rest);
        return;
    }
    while rest == "-" || rest.starts_with("- ") {
        out.push(Tok::Punct, "-");
        rest = &rest[1..];
        let gap = rest.len() - rest.trim_start_matches(' ').len();
        out.push(Tok::Plain, &rest[..gap]);
        rest = &rest[gap..];
    }
    if let Some(k) = key_len(rest) {
        let key = &rest[..k];
        let bare = key.trim_matches(['"', '\'']);
        out.push(if is_intrinsic_key(bare) { Tok::Intrinsic } else { Tok::Key }, key);
        rest = &rest[k..];
        let colon = rest.find(':').map_or(rest.len(), |c| c + 1);
        out.push(Tok::Punct, &rest[..colon]);
        rest = &rest[colon..];
    }
    if opens_block(rest) {
        let (tags, after) = split_tags(rest);
        out.push(Tok::Intrinsic, tags);
        let (head, comment) = split_comment(after);
        out.push(Tok::Punct, head);
        out.push(Tok::Comment, comment);
        st.block = Some(indent);
        return;
    }
    yaml_value(rest, out);
}

/// A value: plain and quoted scalars, tags, and flow collections
/// (`[a, b]`, `{ Ref: X }`).
fn yaml_value(s: &str, out: &mut Out) {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => {
                let st = i;
                while i < b.len() && matches!(b[i], b' ' | b'\t') {
                    i += 1;
                }
                out.push(Tok::Plain, &s[st..i]);
            }
            b'#' if i == 0 || b[i - 1] == b' ' => {
                out.push(Tok::Comment, &s[i..]);
                return;
            }
            b'"' | b'\'' => {
                let st = i;
                i = quoted_end(s, i);
                let text = &s[st..i];
                // `{'Fn::Sub': …}`: a quoted key in a flow mapping.
                let after = s[i..].trim_start_matches(' ');
                if depth > 0 && after.starts_with(':') && matches!(after.as_bytes().get(1), None | Some(b' ')) {
                    let bare = text.trim_matches(['"', '\'']);
                    out.push(if is_intrinsic_key(bare) { Tok::Intrinsic } else { Tok::Key }, text);
                } else {
                    out.push_subs(Tok::Str, text);
                }
            }
            b'[' | b'{' => {
                depth += 1;
                out.push(Tok::Punct, &s[i..i + 1]);
                i += 1;
            }
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                out.push(Tok::Punct, &s[i..i + 1]);
                i += 1;
            }
            b',' if depth > 0 => {
                out.push(Tok::Punct, ",");
                i += 1;
            }
            b'!' => {
                let st = i;
                while i < b.len() && !matches!(b[i], b' ' | b',' | b']' | b'}') {
                    i += 1;
                }
                out.push(Tok::Intrinsic, &s[st..i]);
            }
            _ => {
                // A plain scalar runs to the end of the line, or a ` #`;
                // inside a flow collection also to `,` `]` `}` or a `: `.
                let st = i;
                let mut key = false;
                while i < b.len() {
                    let c = b[i];
                    if c == b'#' && b[i - 1] == b' ' {
                        break;
                    }
                    if depth > 0 && matches!(c, b',' | b']' | b'}') {
                        break;
                    }
                    if depth > 0 && c == b':' && matches!(b.get(i + 1), None | Some(b' ')) {
                        key = true;
                        break;
                    }
                    i += 1;
                }
                let raw = &s[st..i];
                let word = raw.trim_end();
                if key {
                    out.push(if is_intrinsic_key(word) { Tok::Intrinsic } else { Tok::Key }, word);
                } else if is_literal(word) {
                    out.push(Tok::Literal, word);
                } else {
                    out.push_subs(Tok::Plain, word);
                }
                out.push(Tok::Plain, &raw[word.len()..]);
                if key {
                    out.push(Tok::Punct, ":");
                    i += 1;
                }
            }
        }
    }
}

// ── Shell ────────────────────────────────────────────────────────────────

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in",
    "function", "select", "return", "export", "local", "readonly", "declare", "unset", "source",
];

/// Length of the variable reference at the start of `s` (which starts with
/// `$`): `${…}`, `$NAME`, `$1` / `$?` / `$@`, or the `$(` of a command
/// substitution.
fn var_len(s: &str) -> usize {
    let b = s.as_bytes();
    match b.get(1) {
        Some(b'{') => s.find('}').map_or(s.len(), |e| e + 1),
        Some(b'(') => 2,
        Some(c) if c.is_ascii_alphabetic() || *c == b'_' => {
            1 + b[1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count()
        }
        Some(c) if b"0123456789@#?*$!-".contains(c) => 2,
        _ => 1,
    }
}

fn shell_line(line: &str, out: &mut Out) {
    let b = line.as_bytes();
    let special = |c: u8| matches!(c, b';' | b'|' | b'&' | b'<' | b'>' | b'(' | b')');
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b' ' | b'\t' => {
                let s = i;
                while i < b.len() && matches!(b[i], b' ' | b'\t') {
                    i += 1;
                }
                out.push(Tok::Plain, &line[s..i]);
            }
            b'#' if i == 0 || matches!(b[i - 1], b' ' | b'\t' | b';') => {
                out.push(Tok::Comment, &line[i..]);
                return;
            }
            b'\'' => {
                let s = i;
                i = line[i + 1..].find('\'').map_or(b.len(), |e| i + e + 2);
                out.push(Tok::Str, &line[s..i]);
            }
            b'"' => {
                // Variables stay visible inside double quotes.
                out.push(Tok::Str, "\"");
                i += 1;
                let mut s = i;
                while i < b.len() && b[i] != b'"' {
                    match b[i] {
                        b'\\' => i = next_char(line, i + 1),
                        b'$' => {
                            out.push(Tok::Str, &line[s..i]);
                            let n = var_len(&line[i..]);
                            out.push(Tok::Var, &line[i..i + n]);
                            i += n;
                            s = i;
                        }
                        _ => i += 1,
                    }
                }
                let end = (i + 1).min(b.len());
                out.push(Tok::Str, &line[s..end]);
                i = end;
            }
            b'$' => {
                let n = var_len(&line[i..]);
                out.push(Tok::Var, &line[i..i + n]);
                i += n;
            }
            c if special(c) => {
                out.push(Tok::Punct, &line[i..i + 1]);
                i += 1;
            }
            b'\\' => {
                let e = next_char(line, i + 1);
                out.push(Tok::Plain, &line[i..e]);
                i = e;
            }
            _ => {
                let s = i;
                while i < b.len() && !special(b[i]) && !matches!(b[i], b' ' | b'\t' | b'\'' | b'"' | b'$' | b'\\') {
                    i += 1;
                }
                let word = &line[s..i];
                out.push(if SHELL_KEYWORDS.contains(&word) { Tok::Intrinsic } else { Tok::Plain }, word);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lang: Lang, line: &str, kind: Tok) -> Vec<String> {
        lex(lang, &[line])[0]
            .iter()
            .filter(|(t, _)| *t == kind)
            .map(|(_, s)| s.trim().to_string())
            .collect()
    }

    /// Text survives highlighting byte for byte: copy and wrap measure the
    /// rows, the spans must add up to the same thing.
    #[test]
    fn spans_concatenate_back_to_the_line() {
        let samples: &[(Lang, &str)] = &[
            (Lang::Json, r#"  "Statement": [{"Effect": "Allow", "Action": "s3:Get*", "N": -1.5e3}],"#),
            (Lang::Json, r#"  "unterminated \"esc"#),
            (Lang::Json, "  \"café\\é\": true"),
            (Lang::Yaml, "  - !GetAtt [Bucket, Arn]  # the bucket"),
            (Lang::Yaml, "  'quo''ted': {Ref: Foo, Name: \"x\"}"),
            (Lang::Yaml, "  Value: !Sub 'arn:aws:s3:::${Bucket}/*'"),
            (Lang::Shell, r#"  echo "id=${ID:-none} \é $1" | tee -a 'log' # done"#),
            (Lang::Shell, "  for f in *.sh; do bash \"$f\"; done"),
        ];
        for (lang, line) in samples {
            let spans = &highlight(*lang, &[line])[0];
            let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(&joined, line);
        }
    }

    #[test]
    fn json_keys_strings_literals_and_intrinsics() {
        let line = r#"    "Effect": "Allow", "Max": 10, "On": true, "Ref": "Bucket""#;
        assert_eq!(kinds(Lang::Json, line, Tok::Key), vec![r#""Effect""#, r#""Max""#, r#""On""#]);
        assert_eq!(kinds(Lang::Json, line, Tok::Str), vec![r#""Allow""#, r#""Bucket""#]);
        assert_eq!(kinds(Lang::Json, line, Tok::Literal), vec!["10", "true"]);
        assert_eq!(kinds(Lang::Json, line, Tok::Intrinsic), vec![r#""Ref""#]);
        assert_eq!(kinds(Lang::Json, r#"  "Fn::GetAtt": ["A", "Arn"]"#, Tok::Intrinsic), vec![r#""Fn::GetAtt""#]);
    }

    #[test]
    fn substitutions_inside_strings_are_intrinsics() {
        let line = r#"  "Fn::Sub": "arn:aws:s3:::${Bucket}/${AWS::Region}""#;
        assert_eq!(kinds(Lang::Json, line, Tok::Intrinsic), vec![r#""Fn::Sub""#, "${Bucket}", "${AWS::Region}"]);
    }

    #[test]
    fn yaml_keys_tags_and_values() {
        let line = "    BucketName: !Sub '${AWS::StackName}-logs'  # name";
        assert_eq!(kinds(Lang::Yaml, line, Tok::Key), vec!["BucketName"]);
        assert_eq!(kinds(Lang::Yaml, line, Tok::Intrinsic), vec!["!Sub", "${AWS::StackName}"]);
        assert_eq!(kinds(Lang::Yaml, line, Tok::Comment), vec!["# name"]);
        assert_eq!(kinds(Lang::Yaml, "  Port: 443", Tok::Literal), vec!["443"]);
        assert_eq!(kinds(Lang::Yaml, "  Enabled: false", Tok::Literal), vec!["false"]);
        // A version date is a plain scalar, not a number.
        assert!(kinds(Lang::Yaml, "  AWSTemplateFormatVersion: 2010-09-09", Tok::Literal).is_empty());
        // `Fn::` keys colour as the intrinsic they are.
        assert_eq!(kinds(Lang::Yaml, "  - Fn::GetAtt: [Role, Arn]", Tok::Intrinsic), vec!["Fn::GetAtt"]);
        // A colon inside a value doesn't make a key.
        assert_eq!(kinds(Lang::Yaml, "  Arn: arn:aws:iam::123:role/x", Tok::Key), vec!["Arn"]);
        // Flow mapping keys.
        assert_eq!(kinds(Lang::Yaml, "  Target: { Ref: Fn, Port: 80 }", Tok::Intrinsic), vec!["Ref"]);
        assert_eq!(
            kinds(Lang::Yaml, "  VpcId: !ImportValue {'Fn::Sub': '${Stack}-VpcId'}", Tok::Intrinsic),
            vec!["!ImportValue", "'Fn::Sub'", "${Stack}"]
        );
    }

    #[test]
    fn yaml_block_scalars_are_text_until_the_indent_returns() {
        let lines = [
            "  UserData: !Base64 |",
            "    #!/bin/bash",
            "    Key: not a key",
            "",
            "  Next: value",
        ];
        let t = lex(Lang::Yaml, &lines);
        assert!(t[1].iter().chain(&t[2]).chain(&t[3]).all(|(k, _)| *k == Tok::Str));
        assert!(t[4].iter().any(|(k, s)| *k == Tok::Key && s == "Next"));
        // `- |` opens one too.
        let t = lex(Lang::Yaml, &["  - |", "    a: b", "  - c"]);
        assert!(t[1].iter().all(|(k, _)| *k == Tok::Str));
    }

    #[test]
    fn shell_comments_strings_vars_keywords() {
        let line = r#"  if [ -n "$HOME" ]; then echo 'hi' # greet"#;
        assert_eq!(kinds(Lang::Shell, line, Tok::Intrinsic), vec!["if", "then"]);
        assert_eq!(kinds(Lang::Shell, line, Tok::Var), vec!["$HOME"]);
        assert_eq!(kinds(Lang::Shell, line, Tok::Str), vec!["\"", "\"", "'hi'"]);
        assert_eq!(kinds(Lang::Shell, line, Tok::Comment), vec!["# greet"]);
        // `#` inside a word isn't a comment.
        assert!(kinds(Lang::Shell, "  echo a#b", Tok::Comment).is_empty());
    }

    #[test]
    fn detects_only_what_is_unmistakable() {
        assert_eq!(detect(&["  {", "    \"Version\": \"2012-10-17\"", "  }"]), Some(Lang::Json));
        assert_eq!(detect(&["  [{\"a\": 1}]"]), Some(Lang::Json));
        assert_eq!(detect(&["  #!/bin/bash -xe", "  yum update -y"]), Some(Lang::Shell));
        assert_eq!(detect(&["  #!/usr/bin/env bash"]), Some(Lang::Shell));
        assert_eq!(detect(&["  #!/usr/bin/env python3", "  print(1)"]), None);
        assert_eq!(detect(&["  #cloud-config", "  packages: [git]"]), Some(Lang::Yaml));
        let template = [
            "  AWSTemplateFormatVersion: '2010-09-09'",
            "  Resources:",
            "    Fn:",
            "      Type: AWS::Lambda::Function",
            "      Properties:",
            "        Code:",
            "          ZipFile: |",
            "            import json",
            "            def handler(event, context):",
            "                return {}",
            "",
            "        Runtime: python3.12",
        ];
        assert_eq!(detect(&template), Some(Lang::Yaml));
        // Things that aren't code stay plain.
        assert_eq!(detect(&["  arn:aws:iam::123:role/a", "  arn:aws:iam::123:role/b"]), None);
        assert_eq!(detect(&["  tcp  443  0.0.0.0/0", "  tcp  22   10.0.0.0/8"]), None);
        assert_eq!(detect(&["  [INFO] started"]), None);
        assert_eq!(detect(&["  Note: one line"]), None);
        assert_eq!(detect(&["  SELECT *", "  FROM logs", "  WHERE x: 1"]), None);
        assert_eq!(detect(&["  ", "  "]), None);
    }
}
