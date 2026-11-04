// A kernel command's arguments, split into words, quotes and all
//
// msh unquotes a line before a program sees it, and `mikudod` quotes such an
// argument again on its way here (`run_machine_command`), so the rules are
// msh's, the part a word needs:
//
//   * `'...'` is literal, every character of it;
//   * `"..."` is literal except that `\"` and `\\` are a quote and a backslash;
//   * `\` outside quotes makes the next character ordinary.
//
// Quotes are removed. An unclosed quote takes the rest of the line: a command
// typed at the console is not refused for a missing quote at its end.

use alloc::string::String;
use alloc::vec::Vec;

pub fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            return out;
        }
        let mut w = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            match c {
                '\'' => {
                    for c in chars.by_ref() {
                        if c == '\'' {
                            break;
                        }
                        w.push(c);
                    }
                }
                '"' => {
                    while let Some(c) = chars.next() {
                        match c {
                            '"' => break,
                            '\\' if matches!(chars.peek(), Some('"') | Some('\\')) => {
                                w.push(chars.next().unwrap_or('\\'));
                            }
                            c => w.push(c),
                        }
                    }
                }
                '\\' => w.push(chars.next().unwrap_or('\\')),
                c => w.push(c),
            }
        }
        out.push(w);
    }
}
