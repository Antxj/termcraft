//! Delphi binary form (`TPF0`) reader → object tree + textual `.dfm` rendering.

use anyhow::{Result, bail};
use std::fmt::Write as _;

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    List(Vec<Value>),
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    Bool(bool),
    Binary(usize),
    Set(Vec<String>),
    Nil,
    Collection(Vec<Vec<(String, Value)>>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) | Self::Ident(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Object {
    pub class: String,
    pub name: String,
    pub props: Vec<(String, Value)>,
    pub children: Vec<Object>,
}

impl Object {
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn str_prop(&self, key: &str) -> Option<&str> {
        self.prop(key).and_then(Value::as_str)
    }
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Object, usize)) {
        fn go<'a>(o: &'a Object, depth: usize, f: &mut impl FnMut(&'a Object, usize)) {
            f(o, depth);
            for c in &o.children {
                go(c, depth + 1, f);
            }
        }
        go(self, 0, f);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let Some(s) = self.b.get(self.p..self.p + n) else {
            bail!("truncated DFM at {}", self.p)
        };
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn peek(&self) -> Result<u8> {
        self.b
            .get(self.p)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("truncated DFM"))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn len(&mut self) -> Result<usize> {
        usize::try_from(self.i32()?).map_err(|_| anyhow::anyhow!("negative length"))
    }
    fn sstr(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        Ok(latin1(self.take(n)?))
    }

    fn value(&mut self) -> Result<Value> {
        Ok(match self.u8()? {
            0 => Value::Null,
            1 => {
                let mut items = Vec::new();
                while self.peek()? != 0 {
                    items.push(self.value()?);
                }
                self.u8()?;
                Value::List(items)
            }
            2 => Value::Int(i64::from(self.take(1)?[0] as i8)),
            3 => Value::Int(i64::from(i16::from_le_bytes(self.take(2)?.try_into()?))),
            4 => Value::Int(i64::from(self.i32()?)),
            5 => Value::Float(extended_to_f64(self.take(10)?)),
            6 => Value::Str(self.sstr()?),
            7 => Value::Ident(self.sstr()?),
            8 => Value::Bool(false),
            9 => Value::Bool(true),
            10 => {
                let n = self.len()?;
                self.take(n)?;
                Value::Binary(n)
            }
            11 => {
                let mut items = Vec::new();
                loop {
                    let s = self.sstr()?;
                    if s.is_empty() {
                        break;
                    }
                    items.push(s);
                }
                Value::Set(items)
            }
            12 => {
                let n = self.len()?;
                Value::Str(latin1(self.take(n)?))
            }
            13 => Value::Nil,
            14 => {
                let mut items = Vec::new();
                while self.peek()? != 0 {
                    if matches!(self.peek()?, 2..=4) {
                        self.value()?; // explicit item order index
                    }
                    if self.u8()? != 1 {
                        bail!("expected collection item at {}", self.p - 1);
                    }
                    let mut props = Vec::new();
                    while self.peek()? != 0 {
                        let k = self.sstr()?;
                        props.push((k, self.value()?));
                    }
                    self.u8()?;
                    items.push(props);
                }
                self.u8()?;
                Value::Collection(items)
            }
            15 => Value::Float(f64::from(f32::from_le_bytes(self.take(4)?.try_into()?))),
            16 | 17 | 21 => Value::Float(f64::from_le_bytes(self.take(8)?.try_into()?)),
            18 => {
                let n = self.len()?;
                let units: Vec<u16> = self
                    .take(n * 2)?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                Value::Str(String::from_utf16_lossy(&units))
            }
            19 => Value::Int(i64::from_le_bytes(self.take(8)?.try_into()?)),
            20 => {
                let n = self.len()?;
                Value::Str(String::from_utf8_lossy(self.take(n)?).into_owned())
            }
            t => bail!("unknown DFM value type {t} at {}", self.p - 1),
        })
    }

    fn object(&mut self) -> Result<Object> {
        if self.peek()? & 0xf0 == 0xf0 {
            let flags = self.u8()? & 0x0f;
            if flags & 2 != 0 {
                self.value()?; // child position
            }
        }
        let class = self.sstr()?;
        let name = self.sstr()?;
        let mut props = Vec::new();
        while self.peek()? != 0 {
            let k = self.sstr()?;
            props.push((k, self.value()?));
        }
        self.u8()?;
        let mut children = Vec::new();
        while self.peek()? != 0 {
            children.push(self.object()?);
        }
        self.u8()?;
        Ok(Object {
            class,
            name,
            props,
            children,
        })
    }
}

/// Delphi ANSI strings in these forms are Windows-1252; Latin-1 is close
/// enough for labels and never fails.
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn extended_to_f64(raw: &[u8]) -> f64 {
    let mant = u64::from_le_bytes(raw[..8].try_into().unwrap_or_default());
    let se = u16::from_le_bytes([raw[8], raw[9]]);
    if mant == 0 {
        return 0.0;
    }
    let sign = if se & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from(se & 0x7fff) - 16383;
    sign * (mant as f64 / (1u64 << 63) as f64) * 2f64.powi(exp)
}

pub fn parse(blob: &[u8]) -> Result<Object> {
    if blob.get(..4) != Some(b"TPF0") {
        bail!("not a TPF0 form");
    }
    let mut r = Reader { b: blob, p: 4 };
    r.object()
}

pub fn is_form(blob: &[u8]) -> bool {
    blob.starts_with(b"TPF0")
}

// ---------- textual rendering ----------

pub fn quote(s: &str) -> String {
    let mut out = String::new();
    let mut in_q = false;
    for ch in s.chars() {
        let printable = (' '..='~').contains(&ch);
        if printable {
            if !in_q {
                out.push('\'');
                in_q = true;
            }
            if ch == '\'' {
                out.push_str("''");
            } else {
                out.push(ch);
            }
        } else {
            if in_q {
                out.push('\'');
                in_q = false;
            }
            let _ = write!(out, "#{}", ch as u32);
        }
    }
    if in_q {
        out.push('\'');
    }
    if out.is_empty() { "''".into() } else { out }
}

fn render_value(v: &Value, ind: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("<null>"),
        Value::List(items) => {
            out.push('(');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                render_value(it, ind, out);
            }
            out.push(')');
        }
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => {
            let _ = write!(out, "{f}");
        }
        Value::Str(s) => out.push_str(&quote(s)),
        Value::Ident(s) => out.push_str(s),
        Value::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Value::Binary(n) => {
            let _ = write!(out, "{{binary {n} bytes}}");
        }
        Value::Set(items) => {
            let _ = write!(out, "[{}]", items.join(", "));
        }
        Value::Nil => out.push_str("nil"),
        Value::Collection(items) => {
            out.push('<');
            for props in items {
                let _ = write!(out, "\n{}item", "  ".repeat(ind + 1));
                for (k, v) in props {
                    let _ = write!(out, "\n{}{k} = ", "  ".repeat(ind + 2));
                    render_value(v, ind + 2, out);
                }
                let _ = write!(out, "\n{}end", "  ".repeat(ind + 1));
            }
            out.push('>');
        }
    }
}

pub fn to_text(obj: &Object) -> String {
    fn go(o: &Object, ind: usize, out: &mut String) {
        let pad = "  ".repeat(ind);
        let _ = writeln!(out, "{pad}object {}: {}", o.name, o.class);
        for (k, v) in &o.props {
            let _ = write!(out, "{pad}  {k} = ");
            render_value(v, ind + 1, out);
            out.push('\n');
        }
        for c in &o.children {
            go(c, ind + 1, out);
        }
        let _ = writeln!(out, "{pad}end");
    }
    let mut out = String::new();
    go(obj, 0, &mut out);
    out
}

/// Delphi `TShortCut` → "Ctrl+Shift+F5".
pub fn shortcut_label(sc: i64) -> String {
    let sc = sc as u32;
    let mut parts = Vec::new();
    if sc & 0x4000 != 0 {
        parts.push("Ctrl".to_string());
    }
    if sc & 0x8000 != 0 {
        parts.push("Alt".to_string());
    }
    if sc & 0x2000 != 0 {
        parts.push("Shift".to_string());
    }
    let vk = sc & 0xff;
    parts.push(match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0d => "Enter".into(),
        0x1b => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "PgUp".into(),
        0x22 => "PgDn".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2d => "Ins".into(),
        0x2e => "Del".into(),
        0x30..=0x39 | 0x41..=0x5a => char::from(vk as u8).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        0xbb => "+".into(),
        0xbd => "-".into(),
        other => format!("VK_{other:#x}"),
    });
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_form_with_collection() {
        let mut b = b"TPF0".to_vec();
        let sstr = |b: &mut Vec<u8>, s: &str| {
            b.push(s.len() as u8);
            b.extend_from_slice(s.as_bytes());
        };
        sstr(&mut b, "TForm1");
        sstr(&mut b, "Form1");
        sstr(&mut b, "Caption");
        b.push(6);
        sstr(&mut b, "Hi");
        sstr(&mut b, "Columns");
        b.push(14);
        b.push(1); // item
        sstr(&mut b, "Width");
        b.extend_from_slice(&[2, 50]);
        b.push(0); // end item
        b.push(0); // end collection
        b.push(0); // end props
        b.push(0); // end children

        let f = parse(&b).unwrap();
        assert_eq!(f.class, "TForm1");
        assert_eq!(f.str_prop("Caption"), Some("Hi"));
        assert!(matches!(f.prop("Columns"), Some(Value::Collection(items)) if items.len() == 1));
        assert!(to_text(&f).contains("Caption = 'Hi'"));
    }

    #[test]
    fn shortcuts() {
        assert_eq!(shortcut_label(0x4000 | 0x54), "Ctrl+T");
        assert_eq!(shortcut_label(0x6000 | 0x74), "Ctrl+Shift+F5");
    }
}
