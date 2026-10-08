#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TokKind {
    Eof,
    Word,
    Str,
    Op,
    LParen,
    RParen,
    Comma,
}

#[derive(Clone, Debug)]
pub(super) struct Token {
    pub kind: TokKind,
    pub val: String,
    pub pos: usize, // byte offset
}

fn tok(kind: TokKind, val: &str, pos: usize) -> Token {
    Token {
        kind,
        val: val.to_string(),
        pos,
    }
}

/// ASCII white space as Go's unicode.IsSpace sees it.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

pub(super) fn lex(s: &str) -> Result<Vec<Token>, String> {
    use TokKind::*;
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            // All of is_space, not just " \t\n\r": a word can't start on a
            // space byte, so anything else here would never advance.
            _ if is_space(c) => i += 1,
            b'(' => {
                out.push(tok(LParen, "(", i));
                i += 1;
            }
            b')' => {
                out.push(tok(RParen, ")", i));
                i += 1;
            }
            b',' => {
                out.push(tok(Comma, ",", i));
                i += 1;
            }
            b'"' | b'\'' => {
                let start = i;
                i += 1;
                let mut v = Vec::new();
                while i < b.len() && b[i] != c {
                    if b[i] == b'\\' && i + 1 < b.len() {
                        i += 1;
                    }
                    v.push(b[i]);
                    i += 1;
                }
                if i >= b.len() {
                    return Err(format!("unterminated string at {}", start + 1));
                }
                i += 1;
                // Only ASCII bytes were dropped, so v is still valid UTF-8.
                out.push(Token {
                    kind: Str,
                    val: String::from_utf8_lossy(&v).into_owned(),
                    pos: start,
                });
            }
            b'=' | b'!' | b'<' | b'>' | b'~' | b'&' | b'|' => {
                let start = i;
                let n = b.get(i + 1).copied();
                let two = n == Some(b'=')
                    || (c == b'&' && n == Some(b'&'))
                    || (c == b'|' && n == Some(b'|'));
                let op = &s[i..i + 1 + two as usize];
                i += op.len();
                match op {
                    "&&" => out.push(tok(Word, "AND", start)),
                    "||" => out.push(tok(Word, "OR", start)),
                    "!" => out.push(tok(Word, "NOT", start)),
                    "&" | "|" => return Err(format!("unexpected {op:?} at {}", start + 1)),
                    "==" => out.push(tok(Op, "=", start)),
                    _ => out.push(tok(Op, op, start)),
                }
            }
            _ => {
                let start = i;
                while i < b.len() && !is_space(b[i]) && !b"()=!<>~,\"'&|".contains(&b[i]) {
                    i += 1;
                }
                out.push(tok(Word, &s[start..i], start));
            }
        }
    }
    out.push(tok(Eof, "", s.len()));
    Ok(out)
}
