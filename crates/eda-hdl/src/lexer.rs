//! Verilog lexer: whitespace/comments, identifiers, based and plain numbers,
//! strings, and one/two/three-character punctuation.

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Ident(String),
    /// `size` is present for sized literals; `text` holds the digit string
    /// (for based numbers, in the given radix, possibly containing x/z).
    Num {
        size: Option<u32>,
        based: bool,
        signed: bool,
        radix: u32,
        text: String,
    },
    Str(String),
    Punct(&'static str),
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
}

const PUNCTS3: &[&str] = &["===", "!==", "<<=", ">>=", "<<<", "|->", "|=>"];
const PUNCTS2: &[&str] = &[
    "##", "->", "::", "<=", ">=", "==", "!=", "&&", "||", "<<", ">>", "~&", "~|", "~^", "^~", "+=",
    "-=", "*=", "+:", "-:",
];

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
            line: 1,
        }
    }

    pub fn tokenize(src: &'a str) -> Result<Vec<Token>, String> {
        let mut lx = Lexer::new(src);
        let mut out = Vec::new();
        loop {
            let t = lx.next_token()?;
            let eof = t.tok == Tok::Eof;
            out.push(t);
            if eof {
                break;
            }
        }
        Ok(out)
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }

    fn peek_at(&self, k: usize) -> u8 {
        if self.pos + k < self.src.len() {
            self.src[self.pos + k]
        } else {
            0
        }
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
        }
        c
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.bump();
                }
                b'/' if self.peek_at(1) == b'/' => {
                    while self.peek() != b'\n' && self.pos < self.src.len() {
                        self.bump();
                    }
                }
                b'/' if self.peek_at(1) == b'*' => {
                    self.bump();
                    self.bump();
                    loop {
                        if self.pos >= self.src.len() {
                            return;
                        }
                        if self.peek() == b'*' && self.peek_at(1) == b'/' {
                            self.bump();
                            self.bump();
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, String> {
        self.skip_trivia();
        let line = self.line;
        if self.pos >= self.src.len() {
            return Ok(Token {
                tok: Tok::Eof,
                line,
            });
        }
        let c = self.peek();
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let mut s = String::new();
            while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' || self.peek() == b'$'
            {
                s.push(self.bump() as char);
            }
            return Ok(Token {
                tok: Tok::Ident(s),
                line,
            });
        }
        if c.is_ascii_digit() || (c == b'\'' && self.starts_based_literal()) {
            return self.lex_number(line);
        }
        if c == b'"' {
            self.bump();
            let mut s = String::new();
            while self.peek() != b'"' {
                if self.pos >= self.src.len() {
                    return Err(format!("line {}: unterminated string", line));
                }
                let ch = self.bump();
                if ch == b'\\' {
                    let esc = self.bump();
                    s.push(match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'\\' => '\\',
                        b'"' => '"',
                        other => other as char,
                    });
                } else {
                    s.push(ch as char);
                }
            }
            self.bump(); // closing quote
            return Ok(Token {
                tok: Tok::Str(s),
                line,
            });
        }
        // punctuation
        for p in PUNCTS3 {
            if self.src[self.pos..].starts_with(p.as_bytes()) {
                for _ in 0..3 {
                    self.bump();
                }
                return Ok(Token {
                    tok: Tok::Punct(p),
                    line,
                });
            }
        }
        for p in PUNCTS2 {
            if self.src[self.pos..].starts_with(p.as_bytes()) {
                for _ in 0..2 {
                    self.bump();
                }
                return Ok(Token {
                    tok: Tok::Punct(p),
                    line,
                });
            }
        }
        let ch = self.bump() as char;
        if "()[]{};,:?#.=<+-*/%<>&|^~!@'".contains(ch) {
            return Ok(Token {
                tok: Tok::Punct(match ch {
                    '(' => "(",
                    ')' => ")",
                    '[' => "[",
                    ']' => "]",
                    '{' => "{",
                    '}' => "}",
                    ';' => ";",
                    ',' => ",",
                    ':' => ":",
                    '?' => "?",
                    '#' => "#",
                    '.' => ".",
                    '=' => "=",
                    '+' => "+",
                    '-' => "-",
                    '*' => "*",
                    '/' => "/",
                    '%' => "%",
                    '<' => "<",
                    '>' => ">",
                    '&' => "&",
                    '|' => "|",
                    '^' => "^",
                    '~' => "~",
                    '!' => "!",
                    '@' => "@",
                    '\'' => "'",
                    _ => unreachable!(),
                }),
                line,
            });
        }
        Err(format!("line {}: unexpected character '{}'", line, ch))
    }

    /// `'<base>...` (unsized based literal) immediately ahead?
    fn starts_based_literal(&self) -> bool {
        let b1 = self.peek_at(1);
        let base = if b1 == b's' || b1 == b'S' {
            self.peek_at(2)
        } else {
            b1
        };
        matches!(base, b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'h' | b'H')
    }

    fn lex_number(&mut self, line: usize) -> Result<Token, String> {
        // C-style binary/hex literals (`0b`, `0x`) have no Verilog base
        // character after an apostrophe and were previously split into a
        // decimal zero followed by an identifier.
        if self.peek() == b'0'
            && matches!(self.peek_at(1), b'b' | b'B' | b'x' | b'X')
            && self.peek_at(2).is_ascii_hexdigit()
        {
            let radix = if self.peek_at(1) == b'b' || self.peek_at(1) == b'B' {
                2
            } else {
                16
            };
            self.bump();
            self.bump();
            let mut text = String::new();
            while self.peek().is_ascii_hexdigit() || self.peek() == b'_' {
                let c = self.bump();
                if c != b'_' {
                    text.push(c as char);
                }
            }
            if text.is_empty() {
                return Err(format!("line {}: based number with no digits", line));
            }
            return Ok(Token {
                tok: Tok::Num {
                    size: None,
                    based: true,
                    signed: false,
                    radix,
                    text,
                },
                line,
            });
        }

        let mut size_digits = String::new();
        while self.peek().is_ascii_digit() || self.peek() == b'_' {
            let c = self.bump();
            if c != b'_' {
                size_digits.push(c as char);
            }
        }
        if self.peek() == b'\'' {
            self.bump();
            // optional 's' signed marker
            let mut signed = false;
            if self.peek() == b's' || self.peek() == b'S' {
                self.bump();
                signed = true;
            }
            let base = self.bump() as char;
            let radix = match base.to_ascii_lowercase() {
                'b' => 2,
                'o' => 8,
                'd' => 10,
                'h' => 16,
                other => {
                    return Err(format!(
                        "line {}: unknown number base '{}' (expected b/o/d/h)",
                        line, other
                    ))
                }
            };
            let mut text = String::new();
            while self.peek().is_ascii_hexdigit()
                || matches!(self.peek(), b'x' | b'X' | b'z' | b'Z' | b'?' | b'_')
            {
                let c = self.bump();
                if c != b'_' {
                    text.push(c as char);
                }
            }
            if text.is_empty() {
                return Err(format!("line {}: based number with no digits", line));
            }
            // validate digits for radix 2/8/10 (no hex-only chars)
            let max = match radix {
                2 => '1',
                8 => '7',
                10 => '9',
                _ => 'f',
            };
            for ch in text.chars() {
                if !ch.is_ascii_hexdigit() {
                    continue; // x/z/? accepted everywhere
                }
                if ch.to_ascii_lowercase() > max {
                    return Err(format!(
                        "line {}: digit '{}' invalid for base {}",
                        line, ch, radix
                    ));
                }
            }
            let size =
                if size_digits.is_empty() {
                    None
                } else {
                    Some(size_digits.parse::<u32>().map_err(|_| {
                        format!("line {}: bad literal size '{}'", line, size_digits)
                    })?)
                };
            return Ok(Token {
                tok: Tok::Num {
                    size,
                    based: true,
                    signed,
                    radix,
                    text,
                },
                line,
            });
        }
        Ok(Token {
            tok: Tok::Num {
                size: None,
                based: false,
                signed: false,
                radix: 10,
                text: size_digits,
            },
            line,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        Lexer::tokenize(src)
            .unwrap()
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn idents_and_keywords() {
        let toks = kinds("module foo_bar _x $display endmodule");
        assert_eq!(
            toks,
            vec![
                Tok::Ident("module".into()),
                Tok::Ident("foo_bar".into()),
                Tok::Ident("_x".into()),
                Tok::Ident("$display".into()),
                Tok::Ident("endmodule".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn based_numbers() {
        let toks = kinds("4'b101x 8'h3f 'hzz 16'd10 5 12'o17");
        assert_eq!(
            toks,
            vec![
                Tok::Num {
                    size: Some(4),
                    based: true,
                    signed: false,
                    radix: 2,
                    text: "101x".into()
                },
                Tok::Num {
                    size: Some(8),
                    based: true,
                    signed: false,
                    radix: 16,
                    text: "3f".into()
                },
                Tok::Num {
                    size: None,
                    based: true,
                    signed: false,
                    radix: 16,
                    text: "zz".into()
                },
                Tok::Num {
                    size: Some(16),
                    based: true,
                    signed: false,
                    radix: 10,
                    text: "10".into()
                },
                Tok::Num {
                    size: None,
                    based: false,
                    signed: false,
                    radix: 10,
                    text: "5".into()
                },
                Tok::Num {
                    size: Some(12),
                    based: true,
                    signed: false,
                    radix: 8,
                    text: "17".into()
                },
                Tok::Eof
            ]
        );
    }

    #[test]
    fn puncts_and_comments() {
        let toks = kinds("a <= b; // comment\n c === d /* block */ && e");
        assert_eq!(
            toks,
            vec![
                Tok::Ident("a".into()),
                Tok::Punct("<="),
                Tok::Ident("b".into()),
                Tok::Punct(";"),
                Tok::Ident("c".into()),
                Tok::Punct("==="),
                Tok::Ident("d".into()),
                Tok::Punct("&&"),
                Tok::Ident("e".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn strings() {
        let toks = kinds("\"a=%b\" \"tab\\there\"");
        assert_eq!(
            toks,
            vec![
                Tok::Str("a=%b".into()),
                Tok::Str("tab\there".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn line_tracking_and_errors() {
        let toks = Lexer::tokenize("a\nb\nc").unwrap();
        assert_eq!(toks[1].line, 2);
        assert_eq!(toks[2].line, 3);
        assert!(Lexer::tokenize("4'q10").is_err());
        assert!(Lexer::tokenize("\"open").is_err());
        assert!(Lexer::tokenize("`").is_err());
    }
}
