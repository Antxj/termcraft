//! Terminal charset (session 4_6 "Term charset"): what the remote side
//! sends and expects. The emulator works in UTF-8, so output is decoded
//! to UTF-8 before parsing and typed / pasted text is encoded back.
//!
//! UTF-8 passes through untouched. Single-byte charsets use a 256-entry
//! table built once; GBK, Big5, Shift_JIS and EUC-KR use the streaming
//! decoders of `encoding_rs` (a character may be split across reads).

use encoding_rs::Encoding;
use std::borrow::Cow;
use std::collections::HashMap;

/// The 4_6 choices, in reference order (index = stored value).
pub const NAMES: &[&str] = &[
    "ISO-8859-1 (West Europe)",
    "ISO-8859-2 (East Europe)",
    "ISO-8859-3 (South Europe)",
    "ISO-8859-4 (North Europe)",
    "ISO-8859-5 (Cyrillic)",
    "ISO-8859-6 (Arabic)",
    "ISO-8859-7 (Greek)",
    "ISO-8859-8 (Hebrew)",
    "ISO-8859-9 (Turkish)",
    "ISO-8859-10 (Nordic)",
    "ISO-8859-11 (Thai)",
    "ISO-8859-13 (Baltic)",
    "ISO-8859-14 (Celtic)",
    "ISO-8859-15 (Euro)",
    "ISO-8859-16 (Balkan)",
    "UTF-8 (Unicode)",
    "KOI8-R (Russian)",
    "KOI8-U (Ukrainian)",
    "GBK (Chinese)",
    "Big5 (Chinese)",
    "SJIS (Japanese)",
    "eucKR (Korean)",
    "CP850 (OEM Codepage)",
    "Win1252 (Western)",
    "Win1251 (Cyrillic)",
    "Win1250 (Central European)",
    "Win1253 (Greek)",
    "Win1254 (Turkish)",
    "Win1255 (Hebrew)",
    "Win1256 (Arabic)",
    "Win1257 (Baltic)",
    "Win1258 (Vietnamese)",
    "CP437",
    "CP620 (Mazovia)",
    "CP819",
    "CP852",
    "CP878",
    "HP-ROMAN8",
    "VSCII",
    "DEC-MCS",
    "Use font encoding",
];

/// Index of "UTF-8 (Unicode)", the default.
pub const UTF8: usize = 15;

enum Source {
    Utf8,
    /// ISO-8859-x: bytes 0x80..=0x9F are C1 controls, which are dropped.
    Iso(&'static Encoding),
    /// Windows / KOI8 code pages: every byte is a character.
    Page(&'static Encoding),
    Oem(&'static [char; 128]),
    Latin1,
    DecMcs,
    HpRoman8,
    Multi(&'static Encoding),
}

fn source(index: usize) -> Source {
    use Source::*;
    use encoding_rs as e;
    match index {
        0 | 34 => Latin1,
        1 => Iso(e::ISO_8859_2),
        2 => Iso(e::ISO_8859_3),
        3 => Iso(e::ISO_8859_4),
        4 => Iso(e::ISO_8859_5),
        5 => Iso(e::ISO_8859_6),
        6 => Iso(e::ISO_8859_7),
        7 => Iso(e::ISO_8859_8),
        // Same as Windows-1254 / -874 outside 0x80..=0x9F.
        8 => Iso(e::WINDOWS_1254),
        9 => Iso(e::ISO_8859_10),
        10 => Iso(e::WINDOWS_874),
        11 => Iso(e::ISO_8859_13),
        12 => Iso(e::ISO_8859_14),
        13 => Iso(e::ISO_8859_15),
        14 => Iso(e::ISO_8859_16),
        16 | 36 => Page(e::KOI8_R),
        17 => Page(e::KOI8_U),
        18 => Multi(e::GBK),
        19 => Multi(e::BIG5),
        20 => Multi(e::SHIFT_JIS),
        21 => Multi(e::EUC_KR),
        22 => Oem(&oem_cp::code_table::DECODING_TABLE_CP850),
        23 => Page(e::WINDOWS_1252),
        24 => Page(e::WINDOWS_1251),
        25 => Page(e::WINDOWS_1250),
        26 => Page(e::WINDOWS_1253),
        27 => Page(e::WINDOWS_1254),
        28 => Page(e::WINDOWS_1255),
        29 => Page(e::WINDOWS_1256),
        30 => Page(e::WINDOWS_1257),
        31 => Page(e::WINDOWS_1258),
        32 => Oem(&oem_cp::code_table::DECODING_TABLE_CP437),
        35 => Oem(&oem_cp::code_table::DECODING_TABLE_CP852),
        37 => HpRoman8,
        39 => DecMcs,
        // CP620 and VSCII: no table yet, ISO-8859-1 is the closest.
        33 | 38 => Latin1,
        // 15 UTF-8, 40 "Use font encoding" (fonts are Unicode).
        _ => Utf8,
    }
}

/// HP Roman-8, 0xA0..=0xFF.
const HP_ROMAN8: &str = "\u{a0}ÀÂÈÊËÎÏ´ˋˆ¨˜ÙÛ₤¯Ýý°ÇçÑñ¡¿¤£¥§ƒ¢âêôûáéóúàèòùäëöüÅîØÆåíøæÄìÖÜÉïßÔÁÃãÐðÍÌÓÒÕõŠšÚŸÿÞþ·µ¶¾—¼½ªº«■»±\u{fffd}";

/// One byte → its character (`None` = dropped).
fn table(src: &Source) -> Option<Box<[Option<char>; 256]>> {
    let mut t = Box::new([None; 256]);
    for (b, slot) in t.iter_mut().enumerate() {
        *slot = Some(b as u8 as char);
    }
    let upper = |t: &mut [Option<char>; 256], f: &dyn Fn(u8) -> Option<char>| {
        for b in 0x80..=0xffu8 {
            t[usize::from(b)] = f(b);
        }
    };
    let via = |enc: &'static Encoding, b: u8| {
        let byte = [b];
        let (s, bad) = enc.decode_without_bom_handling(&byte);
        (!bad)
            .then(|| s.chars().next())
            .flatten()
            .or(Some('\u{fffd}'))
    };
    match *src {
        Source::Utf8 | Source::Multi(_) => return None,
        Source::Latin1 => upper(&mut t, &|b| (b >= 0xa0).then_some(b as char)),
        Source::Iso(enc) => upper(&mut t, &|b| (b >= 0xa0).then(|| via(enc, b)).flatten()),
        Source::Page(enc) => upper(&mut t, &|b| via(enc, b)),
        Source::Oem(map) => upper(&mut t, &|b| Some(map[usize::from(b - 0x80)])),
        Source::DecMcs => upper(&mut t, &|b| match b {
            0x80..=0x9f => None,
            0xa0
            | 0xa4
            | 0xa6
            | 0xac..=0xaf
            | 0xb4
            | 0xb8
            | 0xbe
            | 0xd0
            | 0xde
            | 0xf0
            | 0xfe
            | 0xff => Some('\u{fffd}'),
            0xa8 => Some('¤'),
            0xd7 => Some('Œ'),
            0xdd => Some('Ÿ'),
            0xf7 => Some('œ'),
            0xfd => Some('ÿ'),
            _ => Some(b as char),
        }),
        Source::HpRoman8 => {
            let chars: Vec<char> = HP_ROMAN8.chars().collect();
            upper(&mut t, &|b| match b {
                0x80..=0x9f => None,
                _ => chars.get(usize::from(b - 0xa0)).copied(),
            })
        }
    }
    Some(t)
}

enum Codec {
    Utf8,
    Single {
        decode: Box<[Option<char>; 256]>,
        encode: HashMap<char, u8>,
    },
    Multi {
        encoding: &'static Encoding,
        decoder: encoding_rs::Decoder,
    },
}

/// Encode with a multi-byte charset; unmappable characters become `?`.
fn encode_multi(encoding: &'static Encoding, mut text: &str, out: &mut Vec<u8>) {
    use encoding_rs::EncoderResult;
    let mut encoder = encoding.new_encoder();
    loop {
        out.reserve(
            encoder
                .max_buffer_length_from_utf8_without_replacement(text.len())
                .unwrap_or(text.len() * 2 + 16),
        );
        let (result, read) = encoder.encode_from_utf8_to_vec_without_replacement(text, out, true);
        text = &text[read..];
        match result {
            EncoderResult::InputEmpty => return,
            EncoderResult::Unmappable(_) => out.push(b'?'),
            EncoderResult::OutputFull => {}
        }
    }
}

/// Converts between the session charset and the emulator's UTF-8.
pub struct TermCharset {
    codec: Codec,
}

impl Default for TermCharset {
    fn default() -> Self {
        Self { codec: Codec::Utf8 }
    }
}

impl TermCharset {
    /// From the 4_6 index (unknown values = UTF-8).
    pub fn from_index(index: usize) -> Self {
        let src = source(index);
        let codec = match (&src, table(&src)) {
            (Source::Multi(enc), _) => Codec::Multi {
                encoding: enc,
                decoder: enc.new_decoder_without_bom_handling(),
            },
            (_, Some(decode)) => {
                let encode = (0x80..=0xffu8)
                    .filter_map(|b| match decode[usize::from(b)] {
                        Some(c) if c != '\u{fffd}' => Some((c, b)),
                        _ => None,
                    })
                    .collect();
                Codec::Single { decode, encode }
            }
            _ => Codec::Utf8,
        };
        Self { codec }
    }

    pub fn is_utf8(&self) -> bool {
        matches!(self.codec, Codec::Utf8)
    }

    /// Received bytes → UTF-8 for the emulator.
    pub fn decode<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        match &mut self.codec {
            Codec::Utf8 => Cow::Borrowed(bytes),
            // A multi-byte decoder may hold the start of a character whose
            // last byte is ASCII, so only single-byte charsets skip ASCII.
            Codec::Single { .. } if bytes.is_ascii() => Cow::Borrowed(bytes),
            Codec::Single { decode, .. } => {
                let mut out = String::with_capacity(bytes.len() + bytes.len() / 2);
                out.extend(bytes.iter().filter_map(|&b| decode[usize::from(b)]));
                Cow::Owned(out.into_bytes())
            }
            Codec::Multi { decoder, .. } => {
                let cap = decoder
                    .max_utf8_buffer_length(bytes.len())
                    .unwrap_or(bytes.len() * 3 + 16);
                let mut out = String::with_capacity(cap);
                let _ = decoder.decode_to_string(bytes, &mut out, false);
                Cow::Owned(out.into_bytes())
            }
        }
    }

    /// Typed or pasted UTF-8 → bytes for the remote side. Bytes that are
    /// not UTF-8 (raw key codes) are sent unchanged; characters the
    /// charset cannot represent become `?`.
    pub fn encode<'a>(&self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        if self.is_utf8() || bytes.is_ascii() {
            return Cow::Borrowed(bytes);
        }
        let mut out = Vec::with_capacity(bytes.len());
        for chunk in bytes.utf8_chunks() {
            let text = chunk.valid();
            match &self.codec {
                Codec::Utf8 => out.extend_from_slice(text.as_bytes()),
                Codec::Single { encode, .. } => out.extend(text.chars().map(|c| {
                    if c.is_ascii() {
                        c as u8
                    } else {
                        encode.get(&c).copied().unwrap_or(b'?')
                    }
                })),
                Codec::Multi { encoding, .. } => encode_multi(encoding, text, &mut out),
            }
            out.extend_from_slice(chunk.invalid());
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_the_schema_count() {
        assert_eq!(NAMES.len(), 41);
        assert_eq!(NAMES[UTF8], "UTF-8 (Unicode)");
        assert_eq!(HP_ROMAN8.chars().count(), 96);
    }

    #[test]
    fn utf8_and_ascii_are_borrowed() {
        let mut c = TermCharset::from_index(UTF8);
        assert!(matches!(c.decode("é".as_bytes()), Cow::Borrowed(_)));
        let mut l = TermCharset::from_index(0);
        assert!(matches!(l.decode(b"\x1b[0m ok"), Cow::Borrowed(_)));
    }

    #[test]
    fn single_byte_round_trip() {
        let mut latin1 = TermCharset::from_index(0);
        assert_eq!(&*latin1.decode(b"caf\xe9"), "café".as_bytes());
        assert_eq!(&*latin1.encode("café €".as_bytes()), b"caf\xe9 ?");
        // C1 controls are dropped.
        assert_eq!(&*latin1.decode(b"a\x9bb"), b"ab");

        let mut koi8 = TermCharset::from_index(16);
        assert_eq!(&*koi8.decode(b"\xf0\xd2\xc9"), "При".as_bytes());
        assert_eq!(&*koi8.encode("При".as_bytes()), b"\xf0\xd2\xc9");

        let mut cp437 = TermCharset::from_index(32);
        assert_eq!(&*cp437.decode(b"\xc9\xcd\xbb"), "╔═╗".as_bytes());

        let mut win1252 = TermCharset::from_index(23);
        assert_eq!(&*win1252.decode(b"\x80"), "€".as_bytes());
        assert_eq!(&*win1252.encode("€".as_bytes()), b"\x80");

        let mut dec = TermCharset::from_index(39);
        assert_eq!(&*dec.decode(b"\xd7\xf7"), "Œœ".as_bytes());
        let mut hp = TermCharset::from_index(37);
        assert_eq!(&*hp.decode(b"\xa1\xde"), "Àß".as_bytes());
    }

    #[test]
    fn multi_byte_split_across_reads() {
        let mut sjis = TermCharset::from_index(20);
        // "日本" in Shift_JIS, split inside the first character; the
        // second one ends with an ASCII byte.
        let mut all = sjis.decode(b"\x93").into_owned();
        all.extend_from_slice(&sjis.decode(b"\xfa\x96"));
        all.extend_from_slice(&sjis.decode(b"{"));
        assert_eq!(all, "日本".as_bytes());
        let mut gbk = TermCharset::from_index(18);
        assert_eq!(&*gbk.encode("中".as_bytes()), b"\xd6\xd0");
        assert_eq!(&*gbk.encode("中🙂".as_bytes()), b"\xd6\xd0?");
        assert_eq!(&*gbk.decode(b"\xd6\xd0"), "中".as_bytes());
    }

    #[test]
    fn raw_key_bytes_pass_through() {
        let latin1 = TermCharset::from_index(0);
        assert_eq!(&*latin1.encode(b"\x1b[A\xff"), b"\x1b[A\xff");
    }
}
