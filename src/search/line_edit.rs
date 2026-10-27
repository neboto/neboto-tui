//! Cursor editing for the one-line query inputs (`/` in the list and in
//! the detail pane): the usual shell keys over a `String` plus a cursor.
//!
//! The cursor is stored as a distance **from the end**, in chars, so `0`
//! means "after the last char". That way the many places that assign a
//! query outright (history recall, jumps, `@` completion) leave the cursor
//! at the end without touching it, and a stale value is simply clamped.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// Only the cursor moved.
    Moved,
    /// Not an editing key; the caller handles it.
    Ignored,
}

/// The cursor as a char index from the start, given its distance from
/// the end.
pub fn cursor_index(text: &str, back: usize) -> usize {
    let len = text.chars().count();
    len - back.min(len)
}

fn byte_at(text: &str, idx: usize) -> usize {
    text.char_indices().nth(idx).map(|(b, _)| b).unwrap_or(text.len())
}

/// Start of the word before `idx`: skip spaces, then non-spaces.
fn word_left(chars: &[char], mut idx: usize) -> usize {
    while idx > 0 && chars[idx - 1] == ' ' {
        idx -= 1;
    }
    while idx > 0 && chars[idx - 1] != ' ' {
        idx -= 1;
    }
    idx
}

/// End of the word after `idx`: skip spaces, then non-spaces.
fn word_right(chars: &[char], mut idx: usize) -> usize {
    while idx < chars.len() && chars[idx] == ' ' {
        idx += 1;
    }
    while idx < chars.len() && chars[idx] != ' ' {
        idx += 1;
    }
    idx
}

/// Apply one key to `text` with the cursor `back` chars from the end.
///
/// Moves: `←` `→`, `Home` `End`, `Ctrl-A` `Ctrl-E`, a word at a time with
/// `Alt`/`Ctrl` + arrows or `Alt-b` `Alt-f` (what macOS terminals send for
/// Option-arrow). Deletes: `Backspace` `Delete`, `Ctrl-W` (word before),
/// `Ctrl-U` (everything before), `Ctrl-K` (everything after). A plain or
/// shifted char is inserted at the cursor; other modified keys are ignored.
pub fn apply(text: &mut String, back: &mut usize, key: KeyEvent) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let idx = len - (*back).min(len);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let move_to = |back: &mut usize, to: usize| {
        *back = len - to;
        Edit::Moved
    };
    let delete = |text: &mut String, back: &mut usize, from: usize, to: usize| {
        if from == to {
            return Edit::Moved;
        }
        let (a, b) = (byte_at(text, from), byte_at(text, to));
        text.replace_range(a..b, "");
        *back = len - to;
        Edit::Changed
    };

    match key.code {
        KeyCode::Left if ctrl || alt => move_to(back, word_left(&chars, idx)),
        KeyCode::Right if ctrl || alt => move_to(back, word_right(&chars, idx)),
        KeyCode::Char('b') if alt => move_to(back, word_left(&chars, idx)),
        KeyCode::Char('f') if alt => move_to(back, word_right(&chars, idx)),
        KeyCode::Left => move_to(back, idx.saturating_sub(1)),
        KeyCode::Right => move_to(back, (idx + 1).min(len)),
        KeyCode::Home => move_to(back, 0),
        KeyCode::End => move_to(back, len),
        KeyCode::Char('a') if ctrl => move_to(back, 0),
        KeyCode::Char('e') if ctrl => move_to(back, len),
        KeyCode::Char('w') if ctrl => delete(text, back, word_left(&chars, idx), idx),
        KeyCode::Backspace if alt => delete(text, back, word_left(&chars, idx), idx),
        KeyCode::Char('u') if ctrl => delete(text, back, 0, idx),
        KeyCode::Char('k') if ctrl => {
            let edit = delete(text, back, idx, len);
            *back = 0;
            edit
        }
        KeyCode::Backspace => delete(text, back, idx.saturating_sub(1), idx),
        KeyCode::Delete => {
            if idx == len {
                return Edit::Moved;
            }
            text.replace_range(byte_at(text, idx)..byte_at(text, idx + 1), "");
            *back -= 1;
            Edit::Changed
        }
        KeyCode::Char(c) if !ctrl && !alt => {
            text.insert(byte_at(text, idx), c);
            *back = len - idx;
            Edit::Changed
        }
        _ => Edit::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn m(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }
    /// Run keys over `start` (cursor at the end); return the text with `|`
    /// at the cursor.
    fn run(start: &str, keys: &[KeyEvent]) -> String {
        let mut text = start.to_string();
        let mut back = 0;
        for key in keys {
            apply(&mut text, &mut back, *key);
        }
        let i = byte_at(&text, cursor_index(&text, back));
        format!("{}|{}", &text[..i], &text[i..])
    }

    #[test]
    fn typing_at_the_end_appends() {
        assert_eq!(run("pro", &[k(KeyCode::Char('d'))]), "prod|");
    }

    #[test]
    fn fixes_a_typo_in_the_middle() {
        let mut keys = vec![k(KeyCode::Left); 4];
        keys.extend([k(KeyCode::Backspace), k(KeyCode::Char('o')), k(KeyCode::Char('d'))]);
        assert_eq!(run("prx-api", &keys), "prod|-api");
    }

    #[test]
    fn home_end_and_emacs_moves() {
        assert_eq!(run("api", &[k(KeyCode::Home), k(KeyCode::Char('x'))]), "x|api");
        assert_eq!(run("api", &[m(KeyCode::Char('a'), KeyModifiers::CONTROL), k(KeyCode::Char('x'))]), "x|api");
        assert_eq!(run("api", &[k(KeyCode::Home), m(KeyCode::Char('e'), KeyModifiers::CONTROL)]), "api|");
        assert_eq!(run("api", &[k(KeyCode::Home), k(KeyCode::End)]), "api|");
    }

    #[test]
    fn moves_clamp_at_the_ends() {
        assert_eq!(run("ab", &[k(KeyCode::Right)]), "ab|");
        assert_eq!(run("ab", &[k(KeyCode::Left), k(KeyCode::Left), k(KeyCode::Left)]), "|ab");
        assert_eq!(run("ab", &[k(KeyCode::Home), k(KeyCode::Backspace)]), "|ab");
        assert_eq!(run("ab", &[k(KeyCode::Delete)]), "ab|");
    }

    #[test]
    fn word_moves() {
        let alt = KeyModifiers::ALT;
        assert_eq!(run("prod api gw", &[m(KeyCode::Left, alt)]), "prod api |gw");
        assert_eq!(run("prod api gw", &[m(KeyCode::Char('b'), alt), m(KeyCode::Char('b'), alt)]), "prod |api gw");
        assert_eq!(
            run("prod api gw", &[k(KeyCode::Home), m(KeyCode::Right, KeyModifiers::CONTROL)]),
            "prod| api gw"
        );
        assert_eq!(run("prod api gw", &[k(KeyCode::Home), m(KeyCode::Char('f'), alt)]), "prod| api gw");
    }

    #[test]
    fn word_and_line_deletes() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(run("prod api ", &[m(KeyCode::Char('w'), ctrl)]), "prod |");
        assert_eq!(run("prod api", &[k(KeyCode::Left), m(KeyCode::Char('w'), ctrl)]), "prod |i");
        assert_eq!(run("prod api", &[k(KeyCode::Left), m(KeyCode::Char('u'), ctrl)]), "|i");
        assert_eq!(run("prod api", &[k(KeyCode::Home), k(KeyCode::Right), m(KeyCode::Char('k'), ctrl)]), "p|");
        assert_eq!(run("prod api", &[m(KeyCode::Backspace, KeyModifiers::ALT)]), "prod |");
    }

    #[test]
    fn delete_removes_the_char_under_the_cursor() {
        assert_eq!(run("prxod", &[k(KeyCode::Home), k(KeyCode::Right), k(KeyCode::Right), k(KeyCode::Delete)]), "pr|od");
    }

    #[test]
    fn shifted_chars_insert_and_other_modified_keys_are_ignored() {
        let mut text = "a".to_string();
        let mut back = 0;
        assert_eq!(apply(&mut text, &mut back, m(KeyCode::Char('B'), KeyModifiers::SHIFT)), Edit::Changed);
        assert_eq!(apply(&mut text, &mut back, m(KeyCode::Char('x'), KeyModifiers::CONTROL)), Edit::Ignored);
        assert_eq!(apply(&mut text, &mut back, k(KeyCode::Enter)), Edit::Ignored);
        assert_eq!(text, "aB");
    }

    #[test]
    fn multibyte_chars_edit_by_char() {
        assert_eq!(run("café", &[k(KeyCode::Left), k(KeyCode::Backspace)]), "ca|é");
        assert_eq!(run("é", &[k(KeyCode::Home), k(KeyCode::Char('x'))]), "x|é");
    }

    #[test]
    fn a_stale_cursor_clamps() {
        let mut text = "ab".to_string();
        let mut back = 9;
        assert_eq!(cursor_index(&text, back), 0);
        apply(&mut text, &mut back, k(KeyCode::Char('x')));
        assert_eq!(text, "xab");
        assert_eq!(cursor_index(&text, back), 1);
    }
}
