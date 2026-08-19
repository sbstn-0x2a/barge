//! Minimaler Parser für Valve-KeyValues (VDF/ACF).
//!
//! Das Format ist rekursiv: `"key" "value"` für Skalare und `"key" { ... }`
//! für Objekte. Steam schreibt Schlüssel und Werte grundsätzlich gequotet;
//! ältere Dateien (z. B. das flache `libraryfolders.vdf`-Format) ebenfalls.
//! Wir tolerieren zusätzlich unquotete Tokens und `//`-Kommentare, damit der
//! Parser auch von Hand editierte Dateien verkraftet.
//!
//! Der Parser deckt sowohl `appmanifest_*.acf` (§3.5) als auch
//! `libraryfolders.vdf` (§3.4) ab — ein eigener, offline-fähiger Ersatz für
//! `keyvalues-parser`/`steamlocate`, deren Linux-Verhalten laut Handout (§13)
//! noch nicht validiert ist.

/// Ein KeyValues-Knoten: entweder ein String-Skalar oder ein Objekt mit
/// geordneten Schlüssel-Wert-Paaren. Die Reihenfolge bleibt erhalten und
/// doppelte Schlüssel sind erlaubt (kommen im `apps`-Block vor).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// Erstes direktes Kind mit diesem Schlüssel (case-insensitiv), falls das
    /// hier ein Objekt ist.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(pairs) => pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Value::Str(_) => None,
        }
    }

    /// Skalarwert dieses Knotens, falls es ein String ist.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Obj(_) => None,
        }
    }

    /// Bequemer Direktzugriff: `node.str("name")`.
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// Alle direkten Kinder als (Schlüssel, Wert)-Paare, falls Objekt.
    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Obj(pairs) => pairs,
            Value::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    LBrace,
    RBrace,
    Str(String),
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '{' => {
                chars.next();
                tokens.push(Token::LBrace);
            }
            '}' => {
                chars.next();
                tokens.push(Token::RBrace);
            }
            '/' => {
                // Zeilenkommentar `//...` überspringen, ein einzelnes '/'
                // behandeln wir als Teil eines unquoteten Tokens.
                chars.next();
                if chars.peek() == Some(&'/') {
                    for cc in chars.by_ref() {
                        if cc == '\n' {
                            break;
                        }
                    }
                } else {
                    tokens.push(read_bareword('/', &mut chars));
                }
            }
            '"' => {
                chars.next(); // öffnendes Quote
                let mut s = String::new();
                let mut closed = false;
                while let Some(cc) = chars.next() {
                    match cc {
                        '\\' => {
                            if let Some(esc) = chars.next() {
                                // Valve escaped nur \\ und \"; alles andere
                                // reichen wir unverändert weiter.
                                match esc {
                                    'n' => s.push('\n'),
                                    't' => s.push('\t'),
                                    other => s.push(other),
                                }
                            }
                        }
                        '"' => {
                            closed = true;
                            break;
                        }
                        _ => s.push(cc),
                    }
                }
                if !closed {
                    return Err("unbalanced quote".into());
                }
                tokens.push(Token::Str(s));
            }
            _ => {
                let first = chars.next().unwrap();
                tokens.push(read_bareword(first, &mut chars));
            }
        }
    }
    Ok(tokens)
}

fn read_bareword(first: char, chars: &mut std::iter::Peekable<std::str::Chars>) -> Token {
    let mut s = String::new();
    s.push(first);
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
            break;
        }
        s.push(c);
        chars.next();
    }
    Token::Str(s)
}

/// Parst einen kompletten KeyValues-Text in ein Wurzel-Objekt.
///
/// Die oberste Ebene ist typischerweise ein einzelnes benanntes Objekt
/// (`"AppState" { ... }` bzw. `"libraryfolders" { ... }`); wir geben das
/// äußere Objekt mit genau diesem einen Eintrag zurück.
pub fn parse(input: &str) -> Result<Value, String> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let mut pairs = Vec::new();
    while pos < tokens.len() {
        let (k, v) = parse_pair(&tokens, &mut pos)?;
        pairs.push((k, v));
    }
    Ok(Value::Obj(pairs))
}

fn parse_pair(tokens: &[Token], pos: &mut usize) -> Result<(String, Value), String> {
    let key = match tokens.get(*pos) {
        Some(Token::Str(s)) => s.clone(),
        Some(_) => return Err("expected key, found brace".into()),
        None => return Err("unexpected end of file".into()),
    };
    *pos += 1;

    match tokens.get(*pos) {
        Some(Token::Str(s)) => {
            *pos += 1;
            Ok((key, Value::Str(s.clone())))
        }
        Some(Token::LBrace) => {
            *pos += 1;
            let mut pairs = Vec::new();
            loop {
                match tokens.get(*pos) {
                    Some(Token::RBrace) => {
                        *pos += 1;
                        break;
                    }
                    None => return Err("unbalanced brace".into()),
                    _ => {
                        let (k, v) = parse_pair(tokens, pos)?;
                        pairs.push((k, v));
                    }
                }
            }
            Ok((key, Value::Obj(pairs)))
        }
        _ => Err(format!("expected value for key '{}'", key)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parst_acf_skalare() {
        let text = r#"
"AppState"
{
    "appid"      "440"
    "name"       "Team Fortress 2"
    "installdir" "Team Fortress 2"
    "StateFlags" "4"
}
"#;
        let root = parse(text).unwrap();
        let app = root.get("AppState").unwrap();
        assert_eq!(app.str("appid"), Some("440"));
        assert_eq!(app.str("name"), Some("Team Fortress 2"));
        assert_eq!(app.str("installdir"), Some("Team Fortress 2"));
        assert_eq!(app.str("StateFlags"), Some("4"));
    }

    #[test]
    fn parst_verschachtelte_libraryfolders() {
        let text = r#"
"libraryfolders"
{
    "0"
    {
        "path"  "/home/user/.local/share/Steam"
        "apps"  { "440" "1234567" }
    }
    "1" { "path" "/mnt/Games/SteamLibrary" }
}
"#;
        let root = parse(text).unwrap();
        let lf = root.get("libraryfolders").unwrap();
        assert_eq!(
            lf.get("0").unwrap().str("path"),
            Some("/home/user/.local/share/Steam")
        );
        assert_eq!(
            lf.get("1").unwrap().str("path"),
            Some("/mnt/Games/SteamLibrary")
        );
    }

    #[test]
    fn parst_altes_flaches_format() {
        let text = r#"
"libraryfolders"
{
    "0"  "/home/user/.local/share/Steam"
    "1"  "/mnt/Games/SteamLibrary"
}
"#;
        let root = parse(text).unwrap();
        let lf = root.get("libraryfolders").unwrap();
        assert_eq!(lf.str("0"), Some("/home/user/.local/share/Steam"));
        assert_eq!(lf.str("1"), Some("/mnt/Games/SteamLibrary"));
    }

    #[test]
    fn escapes_und_leerzeichen() {
        let text = r#""AppState" { "installdir" "Some \"Game\" Dir" }"#;
        let root = parse(text).unwrap();
        assert_eq!(
            root.get("AppState").unwrap().str("installdir"),
            Some("Some \"Game\" Dir")
        );
    }
}
