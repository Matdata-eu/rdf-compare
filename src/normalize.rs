//! Opt-in literal normalisation: rewrites literals to a canonical lexical
//! form before the set-diff, so values that mean the same thing (`"01"` and
//! `"1"` as `xsd:integer`, `@EN` and `@en`, WKT with different spacing or
//! coordinate precision) compare equal.
//!
//! The datatype of a literal is never changed: `"1"^^xsd:int` and
//! `"1"^^xsd:integer` still differ. Lexical forms that are not valid for
//! their datatype are left untouched.

use oxrdf::{Literal, NamedNode, Quad, Term, Triple};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const WKT_LITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";

const INTEGER_TYPES: &[&str] = &[
    "integer",
    "long",
    "int",
    "short",
    "byte",
    "nonNegativeInteger",
    "positiveInteger",
    "nonPositiveInteger",
    "negativeInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
];

/// Which literal rewrites to apply. The default does nothing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Normalization {
    /// Canonicalise numbers, booleans, date/times, language tags and WKT.
    pub literals: bool,
    /// Round WKT coordinates to this many decimals (also normalises WKT
    /// formatting on its own, without `literals`).
    pub wkt_precision: Option<u8>,
}

impl Normalization {
    pub fn is_noop(&self) -> bool {
        !self.literals && self.wkt_precision.is_none()
    }

    pub fn quad(&self, mut q: Quad) -> Quad {
        if !self.is_noop() {
            q.object = self.term(q.object);
        }
        q
    }

    pub fn triple(&self, mut t: Triple) -> Triple {
        if !self.is_noop() {
            t.object = self.term(t.object);
        }
        t
    }

    pub fn term(&self, t: Term) -> Term {
        match t {
            Term::Literal(l) => Term::Literal(self.literal(l)),
            other => other,
        }
    }

    pub fn literal(&self, l: Literal) -> Literal {
        if let Some(lang) = l.language() {
            if self.literals && lang.bytes().any(|b| b.is_ascii_uppercase()) {
                let lang = lang.to_ascii_lowercase();
                return Literal::new_language_tagged_literal_unchecked(l.value(), lang);
            }
            return l;
        }
        let dt = l.datatype().as_str();
        let normalized = if dt == WKT_LITERAL {
            if self.literals || self.wkt_precision.is_some() {
                normalize_wkt(l.value(), self.wkt_precision)
            } else {
                None
            }
        } else if !self.literals {
            None
        } else if let Some(local) = dt.strip_prefix(XSD) {
            let v = l.value();
            match local {
                _ if INTEGER_TYPES.contains(&local) => normalize_integer(v),
                "decimal" => normalize_decimal(v),
                "double" => normalize_double(v, false),
                "float" => normalize_double(v, true),
                "boolean" => normalize_boolean(v),
                "dateTime" | "dateTimeStamp" | "time" => normalize_time(v),
                "date" => normalize_timezone(v.trim()),
                _ => None,
            }
        } else {
            None
        };
        match normalized {
            Some(v) if v != l.value() => {
                Literal::new_typed_literal(v, NamedNode::new_unchecked(dt.to_string()))
            }
            _ => l,
        }
    }
}

/// Split an optional sign off `s`, returning (is_negative, rest).
fn split_sign(s: &str) -> (bool, &str) {
    match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn normalize_integer(v: &str) -> Option<String> {
    let (neg, digits) = split_sign(v.trim());
    if !all_digits(digits) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some("0".to_string());
    }
    Some(if neg {
        format!("-{digits}")
    } else {
        digits.to_string()
    })
}

/// Canonical decimal: no `+`, no superfluous zeros, and always one digit on
/// each side of the point (`1.0`, `-0.5`).
fn normalize_decimal(v: &str) -> Option<String> {
    let (neg, body) = split_sign(v.trim());
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    if (int.is_empty() && frac.is_empty())
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = if frac.is_empty() { "0" } else { frac };
    let zero = int == "0" && frac == "0";
    Some(format!(
        "{}{int}.{frac}",
        if neg && !zero { "-" } else { "" }
    ))
}

/// Canonical double/float as in XSD: `1.0E0`, `-2.5E-3`, `INF`, `NaN`.
fn normalize_double(v: &str, single: bool) -> Option<String> {
    let v = v.trim();
    match v {
        "INF" | "+INF" => return Some("INF".to_string()),
        "-INF" => return Some("-INF".to_string()),
        "NaN" => return Some("NaN".to_string()),
        _ => {}
    }
    // Rust also accepts "inf"/"infinity"/"nan", which XSD does not.
    if !v
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return None;
    }
    let s = if single {
        let f: f32 = v.parse().ok()?;
        if f == 0.0 {
            return Some(
                if f.is_sign_negative() {
                    "-0.0E0"
                } else {
                    "0.0E0"
                }
                .to_string(),
            );
        }
        format!("{f:E}")
    } else {
        let f: f64 = v.parse().ok()?;
        if f == 0.0 {
            return Some(
                if f.is_sign_negative() {
                    "-0.0E0"
                } else {
                    "0.0E0"
                }
                .to_string(),
            );
        }
        format!("{f:E}")
    };
    // Rust prints `1E0` / `1.5E-3`; XSD wants a fractional digit.
    let (mantissa, exp) = s.split_once('E')?;
    Some(if mantissa.contains('.') {
        format!("{mantissa}E{exp}")
    } else {
        format!("{mantissa}.0E{exp}")
    })
}

fn normalize_boolean(v: &str) -> Option<String> {
    match v.trim() {
        "true" | "1" => Some("true".to_string()),
        "false" | "0" => Some("false".to_string()),
        _ => None,
    }
}

/// `xsd:dateTime` / `xsd:time`: drop trailing zeros of fractional seconds
/// and write a UTC offset as `Z`. Other offsets are kept as written.
fn normalize_time(v: &str) -> Option<String> {
    let v = v.trim();
    let tz_start = v
        .rfind(['Z', '+'])
        .or_else(|| {
            // A `-` after the time part (`T…` or `hh:mm:ss`) starts an offset.
            let t = v.find('T').map(|i| i + 1).unwrap_or(0);
            v[t..].rfind('-').map(|i| i + t)
        })
        .unwrap_or(v.len());
    let (main, tz) = v.split_at(tz_start);
    let main = match main.rsplit_once('.') {
        Some((head, frac)) if all_digits(frac) => {
            let frac = frac.trim_end_matches('0');
            if frac.is_empty() {
                head.to_string()
            } else {
                format!("{head}.{frac}")
            }
        }
        _ => main.to_string(),
    };
    Some(format!("{main}{}", canonical_tz(tz)))
}

fn normalize_timezone(v: &str) -> Option<String> {
    // xsd:date is `YYYY-MM-DD` with an optional offset after the 10th char.
    let split = v.char_indices().nth(10).map(|(i, _)| i).unwrap_or(v.len());
    let (main, tz) = v.split_at(split);
    Some(format!("{main}{}", canonical_tz(tz)))
}

fn canonical_tz(tz: &str) -> &str {
    match tz {
        "+00:00" | "-00:00" => "Z",
        other => other,
    }
}

/// Re-serialise a WKT literal with canonical spacing, upper-case keywords and
/// shortest-form numbers, optionally rounding coordinates to `precision`
/// decimals. Returns `None` when the text does not tokenise as WKT.
fn normalize_wkt(v: &str, precision: Option<u8>) -> Option<String> {
    #[derive(PartialEq)]
    enum Prev {
        Start,
        Word,
        Number,
        Open,
        Other,
    }
    let v = v.trim();
    let mut out = String::with_capacity(v.len());
    let mut prev = Prev::Start;
    let mut chars = v.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '<' => {
                // Optional CRS IRI prefix.
                let end = v[i..].find('>')? + i;
                out.push_str(&v[i..=end]);
                out.push(' ');
                while chars.peek().is_some_and(|&(j, _)| j <= end) {
                    chars.next();
                }
                prev = Prev::Start;
            }
            '(' => {
                out.push('(');
                prev = Prev::Open;
            }
            ')' | ',' => {
                out.push(c);
                prev = Prev::Other;
            }
            c if c.is_ascii_alphabetic() => {
                let mut end = i + c.len_utf8();
                while let Some(&(j, d)) = chars.peek() {
                    if !d.is_ascii_alphabetic() {
                        break;
                    }
                    end = j + d.len_utf8();
                    chars.next();
                }
                if prev == Prev::Word || prev == Prev::Number {
                    out.push(' ');
                }
                out.push_str(&v[i..end].to_ascii_uppercase());
                prev = Prev::Word;
            }
            c if c.is_ascii_digit() || matches!(c, '-' | '+' | '.') => {
                let mut end = i + 1;
                while let Some(&(j, d)) = chars.peek() {
                    let exp_sign =
                        matches!(d, '-' | '+') && matches!(v.as_bytes()[j - 1], b'e' | b'E');
                    if !(d.is_ascii_digit() || matches!(d, '.' | 'e' | 'E') || exp_sign) {
                        break;
                    }
                    end = j + 1;
                    chars.next();
                }
                let n: f64 = v[i..end].parse().ok()?;
                if prev == Prev::Number || prev == Prev::Word {
                    out.push(' ');
                }
                out.push_str(&format_coord(n, precision));
                prev = Prev::Number;
            }
            _ => return None,
        }
    }
    Some(out)
}

fn format_coord(n: f64, precision: Option<u8>) -> String {
    let s = match precision {
        Some(p) => {
            let s = format!("{n:.*}", p as usize);
            if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                s
            }
        }
        // `Display` for f64 is the shortest string that round-trips.
        None => n.to_string(),
    };
    if s == "-0" { "0".to_string() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Normalization = Normalization {
        literals: true,
        wkt_precision: None,
    };

    fn typed(v: &str, dt: &str) -> Literal {
        Literal::new_typed_literal(v, NamedNode::new_unchecked(dt))
    }

    fn xsd(v: &str, local: &str) -> String {
        ALL.literal(typed(v, &format!("{XSD}{local}")))
            .value()
            .to_string()
    }

    #[test]
    fn integers() {
        assert_eq!(xsd("007", "integer"), "7");
        assert_eq!(xsd("+5", "int"), "5");
        assert_eq!(xsd("-000", "integer"), "0");
        assert_eq!(xsd("-012", "long"), "-12");
        assert_eq!(xsd("12a", "integer"), "12a");
    }

    #[test]
    fn decimals() {
        assert_eq!(xsd("1", "decimal"), "1.0");
        assert_eq!(xsd("01.500", "decimal"), "1.5");
        assert_eq!(xsd("-.5", "decimal"), "-0.5");
        assert_eq!(xsd("-0.0", "decimal"), "0.0");
        assert_eq!(xsd("+3.", "decimal"), "3.0");
        assert_eq!(xsd("1e3", "decimal"), "1e3");
    }

    #[test]
    fn doubles() {
        assert_eq!(xsd("1", "double"), "1.0E0");
        assert_eq!(xsd("100.0", "double"), "1.0E2");
        assert_eq!(xsd("1.5e-3", "double"), "1.5E-3");
        assert_eq!(xsd("+INF", "double"), "INF");
        assert_eq!(xsd("0", "float"), "0.0E0");
        assert_eq!(xsd("0.1", "float"), "1.0E-1");
        assert_eq!(xsd("inf", "double"), "inf");
    }

    #[test]
    fn booleans_and_times() {
        assert_eq!(xsd("1", "boolean"), "true");
        assert_eq!(xsd("0", "boolean"), "false");
        assert_eq!(
            xsd("2024-01-02T03:04:05.500+00:00", "dateTime"),
            "2024-01-02T03:04:05.5Z"
        );
        assert_eq!(
            xsd("2024-01-02T03:04:05.000-02:00", "dateTime"),
            "2024-01-02T03:04:05-02:00"
        );
        assert_eq!(
            xsd("2024-01-02T03:04:05", "dateTime"),
            "2024-01-02T03:04:05"
        );
        assert_eq!(xsd("2024-01-02-00:00", "date"), "2024-01-02Z");
        assert_eq!(xsd("10:00:00.0+00:00", "time"), "10:00:00Z");
    }

    #[test]
    fn strings_and_language_tags() {
        assert_eq!(xsd(" 007 ", "string"), " 007 ");
        let l = ALL.literal(Literal::new_language_tagged_literal_unchecked(
            "Hi", "EN-GB",
        ));
        assert_eq!(l.language(), Some("en-gb"));
        assert_eq!(l.value(), "Hi");
    }

    #[test]
    fn wkt_formatting_and_precision() {
        let wkt = |v: &str, p: Option<u8>| {
            let n = Normalization {
                literals: false,
                wkt_precision: p,
            };
            let n = if p.is_none() { ALL } else { n };
            n.literal(typed(v, WKT_LITERAL)).value().to_string()
        };
        assert_eq!(wkt("point ( 4.50  50.0 )", None), "POINT(4.5 50)");
        assert_eq!(
            wkt(
                "<http://www.opengis.net/def/crs/EPSG/0/4326>  Point(50 4)",
                None
            ),
            "<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(50 4)"
        );
        assert_eq!(
            wkt("LINESTRING Z (1 2 3, 4 5 6)", None),
            "LINESTRING Z(1 2 3,4 5 6)"
        );
        assert_eq!(
            wkt("POINT(4.123456789 -0.0000001)", Some(5)),
            "POINT(4.12346 0)"
        );
        assert_eq!(wkt("POINT(1e2 -2.5E-1)", None), "POINT(100 -0.25)");
        assert_eq!(wkt("POINT EMPTY", None), "POINT EMPTY");
        // Precision alone does not touch other literals.
        let p = Normalization {
            literals: false,
            wkt_precision: Some(2),
        };
        assert_eq!(
            p.literal(typed("007", &format!("{XSD}integer"))).value(),
            "007"
        );
    }

    #[test]
    fn default_is_noop() {
        let n = Normalization::default();
        assert!(n.is_noop());
        assert_eq!(
            n.literal(typed("007", &format!("{XSD}integer"))).value(),
            "007"
        );
    }
}
