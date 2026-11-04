// Tokens for both parts of a QML file
//
// aosh parses the declarative tree and passes JavaScript to QuickJS as source
// text. JavaScript is tokenized only to find where it ends, since strings,
// templates, regexes, and comments can hold `}` or newlines. The parser takes
// the expression back as a slice of the file.

/// Where in a file something is, for every message that names a place
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Loc {
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    Number(f64),
    /// A string literal, already unescaped. The source slice keeps the quotes
    Str(String),
    /// A template literal as written, only inside a JS expression
    Template,
    /// A regular expression literal, as written
    Regex,
    /// Punctuation and operators, longest match first: `===`, `=>`, `?.`, `{`
    Punct(&'static str),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    /// Byte range in the source
    pub start: usize,
    pub end: usize,
    pub loc: Loc,
    /// A line break precedes this token; a QML value may end there
    pub newline_before: bool,
}

const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=",
    "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=",
    "/=", "%=", "&=", "|=", "^=", "<<", ">>", "**",
    "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%", "&", "|",
    "^", "!", "~", "?", ":", "=", ".", "@",
];

#[derive(Debug)]
pub struct LexError {
    pub loc: Loc,
    pub what: String,
}

pub fn lex(src: &str) -> Result<Vec<Token>, LexError> {
    let mut l = Lexer { src: src.as_bytes(), text: src, pos: 0, line: 1, col: 1, out: Vec::new() };
    l.run()?;
    Ok(l.out)
}

struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    line: u32,
    col: u32,
    out: Vec<Token>,
}

impl<'a> Lexer<'a> {
    fn loc(&self) -> Loc {
        Loc { line: self.line, col: self.col }
    }

    fn err<T>(&self, what: impl Into<String>) -> Result<T, LexError> {
        Err(LexError { loc: self.loc(), what: what.into() })
    }

    fn peek(&self, ahead: usize) -> u8 {
        *self.src.get(self.pos + ahead).unwrap_or(&0)
    }

    fn bump(&mut self) -> u8 {
        let c = self.src[self.pos];
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else if c & 0xC0 != 0x80 {
            // Columns count characters, not bytes
            self.col += 1;
        }
        c
    }

    /// Whether `/` starts a regex rather than a division. It divides after a
    /// name, number, string, `)` or `]`. `}` is a guess: it ends a block more
    /// often than an object literal in these positions
    fn regex_allowed(&self) -> bool {
        match self.out.last().map(|t| &t.tok) {
            None => true,
            Some(Tok::Ident(w)) => matches!(
                w.as_str(),
                "return" | "typeof" | "instanceof" | "in" | "of" | "new" | "delete" | "void"
                    | "throw" | "case" | "do" | "else" | "yield" | "await"
            ),
            Some(Tok::Number(_)) | Some(Tok::Str(_)) | Some(Tok::Template) | Some(Tok::Regex) => false,
            Some(Tok::Punct(p)) => !matches!(*p, ")" | "]" | "}" | "++" | "--"),
        }
    }

    fn run(&mut self) -> Result<(), LexError> {
        self.tokens(false)
    }

    /// Tokens to the end of the file, or inside `${ ... }` to the closing `}`.
    /// The caller drops those, since a template is one token
    fn tokens(&mut self, until_close: bool) -> Result<(), LexError> {
        let mut newline = false;
        let mut depth = 0usize;
        let at = self.loc();
        loop {
            // Whitespace and comments, noting whether a line break went by
            loop {
                let c = self.peek(0);
                if c == b'\n' {
                    newline = true;
                    self.bump();
                } else if c == b' ' || c == b'\t' || c == b'\r' {
                    self.bump();
                } else if c == b'/' && self.peek(1) == b'/' {
                    while self.pos < self.src.len() && self.peek(0) != b'\n' {
                        self.bump();
                    }
                } else if c == b'/' && self.peek(1) == b'*' {
                    let at = self.loc();
                    self.bump();
                    self.bump();
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(LexError { loc: at, what: "a comment that never ends".into() });
                        }
                        if self.peek(0) == b'*' && self.peek(1) == b'/' {
                            self.bump();
                            self.bump();
                            break;
                        }
                        if self.bump() == b'\n' {
                            newline = true;
                        }
                    }
                } else {
                    break;
                }
            }
            if self.pos >= self.src.len() {
                if until_close {
                    return Err(LexError { loc: at, what: "a `${` that never closes".into() });
                }
                return Ok(());
            }
            let start = self.pos;
            let loc = self.loc();
            let c = self.peek(0);
            let tok = if c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 {
                while {
                    let c = self.peek(0);
                    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
                } {
                    self.bump();
                }
                Tok::Ident(self.text[start..self.pos].to_string())
            } else if c.is_ascii_digit() || (c == b'.' && self.peek(1).is_ascii_digit()) {
                self.number()?
            } else if c == b'"' || c == b'\'' {
                self.string(c)?
            } else if c == b'`' {
                self.template()?;
                Tok::Template
            } else if c == b'/' && self.regex_allowed() {
                self.regex()?;
                Tok::Regex
            } else {
                let rest = &self.text[self.pos..];
                let Some(p) = PUNCTS.iter().find(|p| rest.starts_with(**p)) else {
                    return self.err(format!("'{}' is not something QML or JavaScript has",
                                            rest.chars().next().unwrap_or('?')));
                };
                for _ in 0..p.len() {
                    self.bump();
                }
                Tok::Punct(p)
            };
            if until_close {
                match tok {
                    Tok::Punct("{") => depth += 1,
                    Tok::Punct("}") if depth == 0 => return Ok(()),
                    Tok::Punct("}") => depth -= 1,
                    _ => {}
                }
            }
            self.out.push(Token { tok, start, end: self.pos, loc, newline_before: newline });
            newline = false;
        }
    }

    fn number(&mut self) -> Result<Tok, LexError> {
        let start = self.pos;
        if self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
            let radix = match self.peek(1) { b'x' | b'X' => 16, b'b' | b'B' => 2, _ => 8 };
            self.bump();
            self.bump();
            let digits = self.pos;
            while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                self.bump();
            }
            let text: String = self.text[digits..self.pos].chars().filter(|c| *c != '_').collect();
            return match u64::from_str_radix(&text, radix) {
                Ok(v) => Ok(Tok::Number(v as f64)),
                Err(_) => self.err(format!("'{}' is not a number", &self.text[start..self.pos])),
            };
        }
        while self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
            self.bump();
        }
        if self.peek(0) == b'.' && self.peek(1) != b'.' {
            self.bump();
            while self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
                self.bump();
            }
        }
        if matches!(self.peek(0), b'e' | b'E')
            && (self.peek(1).is_ascii_digit() || (matches!(self.peek(1), b'+' | b'-') && self.peek(2).is_ascii_digit()))
        {
            self.bump();
            if matches!(self.peek(0), b'+' | b'-') {
                self.bump();
            }
            while self.peek(0).is_ascii_digit() {
                self.bump();
            }
        }
        // `10n`, a BigInt: left to QuickJS, a number as a QML literal
        if self.peek(0) == b'n' {
            self.bump();
        }
        let text: String = self.text[start..self.pos].chars().filter(|c| *c != '_' && *c != 'n').collect();
        match text.parse::<f64>() {
            Ok(v) => Ok(Tok::Number(v)),
            Err(_) => self.err(format!("'{}' is not a number", &self.text[start..self.pos])),
        }
    }

    fn string(&mut self, quote: u8) -> Result<Tok, LexError> {
        let at = self.loc();
        self.bump();
        let mut out = String::new();
        loop {
            if self.pos >= self.src.len() || self.peek(0) == b'\n' {
                return Err(LexError { loc: at, what: "a string that never ends".into() });
            }
            let c = self.peek(0);
            if c == quote {
                self.bump();
                return Ok(Tok::Str(out));
            }
            if c == b'\\' {
                self.bump();
                let e = self.bump();
                match e {
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    b'r' => out.push('\r'),
                    b'0' => out.push('\0'),
                    b'\n' => {}
                    b'u' => {
                        let start = self.pos;
                        let code = if self.peek(0) == b'{' {
                            self.bump();
                            while self.peek(0) != b'}' && self.pos < self.src.len() {
                                self.bump();
                            }
                            let hex = &self.text[start + 1..self.pos];
                            self.bump();
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            for _ in 0..4 {
                                self.bump();
                            }
                            u32::from_str_radix(&self.text[start..self.pos], 16).ok()
                        };
                        out.push(code.and_then(char::from_u32).unwrap_or('\u{FFFD}'));
                    }
                    b'x' => {
                        let start = self.pos;
                        self.bump();
                        self.bump();
                        let code = u32::from_str_radix(&self.text[start..self.pos], 16).ok();
                        out.push(code.and_then(char::from_u32).unwrap_or('\u{FFFD}'));
                    }
                    _ => {
                        // Other escapes stand for themselves; continuation
                        // bytes are copied below
                        let start = self.pos - 1;
                        while self.peek(0) & 0xC0 == 0x80 {
                            self.bump();
                        }
                        out.push_str(&self.text[start..self.pos]);
                    }
                }
                continue;
            }
            let start = self.pos;
            self.bump();
            while self.peek(0) & 0xC0 == 0x80 {
                self.bump();
            }
            out.push_str(&self.text[start..self.pos]);
        }
    }

    /// Skips a template literal, including nested `${ ... }`
    fn template(&mut self) -> Result<(), LexError> {
        let at = self.loc();
        self.bump();
        loop {
            if self.pos >= self.src.len() {
                return Err(LexError { loc: at, what: "a template literal that never ends".into() });
            }
            match self.peek(0) {
                b'`' => {
                    self.bump();
                    return Ok(());
                }
                b'\\' => {
                    self.bump();
                    self.bump();
                }
                b'$' if self.peek(1) == b'{' => {
                    self.bump();
                    self.bump();
                    let keep = self.out.len();
                    self.tokens(true)?;
                    self.out.truncate(keep);
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn regex(&mut self) -> Result<(), LexError> {
        let at = self.loc();
        self.bump();
        let mut class = false;
        loop {
            if self.pos >= self.src.len() || self.peek(0) == b'\n' {
                return Err(LexError { loc: at, what: "a regular expression that never ends".into() });
            }
            match self.bump() {
                b'\\' => {
                    self.bump();
                }
                b'[' => class = true,
                b']' => class = false,
                b'/' if !class => break,
                _ => {}
            }
        }
        while self.peek(0).is_ascii_alphabetic() {
            self.bump();
        }
        Ok(())
    }
}
