//! JSON output with ECMAScript-compatible number rendering.
//!
//! Numbers must come out exactly as the reference `unity` CLI prints them, i.e. with
//! `JSON.stringify`'s rules: no trailing `.0` on integral values, and plain decimal notation
//! between 1e-6 and 1e21 (outside that band it switches to exponential). serde_json's own float
//! rendering differs on both counts — `1.0` instead of `1`, `1e-6` instead of `0.000001` — so
//! every response this client emits is serialized through here.

use std::io;

use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};
use serde_json::ser::CompactFormatter;

/// Compact JSON, the form used on the MCP wire.
pub fn compact<T: Serialize>(value: &T) -> String {
    let mut out = Vec::with_capacity(256);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, JsNumbers::new(CompactFormatter));
    value
        .serialize(&mut ser)
        .expect("serializing to a Vec cannot fail");
    String::from_utf8(out).expect("serde_json emits UTF-8")
}

/// 2-space indented JSON, the human/agent-facing form.
pub fn pretty<T: Serialize>(value: &T) -> String {
    let mut out = Vec::with_capacity(256);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, JsNumbers::new(PrettyFormatter::new()));
    value
        .serialize(&mut ser)
        .expect("serializing to a Vec cannot fail");
    String::from_utf8(out).expect("serde_json emits UTF-8")
}

/// A [`Formatter`] that delegates everything to an inner formatter and renders numbers the way
/// JavaScript does.
struct JsNumbers<F> {
    inner: F,
}

impl<F: Formatter> JsNumbers<F> {
    fn new(inner: F) -> Self {
        Self { inner }
    }
}

/// Forward a method to the wrapped formatter, so only the number rendering below differs.
macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name<W>(&mut self, writer: &mut W $(, $arg: $ty)*) -> io::Result<()>
            where
                W: ?Sized + io::Write,
            {
                self.inner.$name(writer $(, $arg)*)
            }
        )*
    };
}

impl<F: Formatter> Formatter for JsNumbers<F> {
    fn write_f64<W>(&mut self, writer: &mut W, value: f64) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(js_number(&format!("{:e}", value.abs()), value < 0.0, value == 0.0).as_bytes())
    }

    fn write_f32<W>(&mut self, writer: &mut W, value: f32) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        writer.write_all(js_number(&format!("{:e}", value.abs()), value < 0.0, value == 0.0).as_bytes())
    }

    delegate! {
        write_null();
        write_bool(value: bool);
        write_i8(value: i8);
        write_i16(value: i16);
        write_i32(value: i32);
        write_i64(value: i64);
        write_i128(value: i128);
        write_u8(value: u8);
        write_u16(value: u16);
        write_u32(value: u32);
        write_u64(value: u64);
        write_u128(value: u128);
        write_number_str(value: &str);
        begin_string();
        end_string();
        write_string_fragment(fragment: &str);
        write_char_escape(char_escape: serde_json::ser::CharEscape);
        write_byte_array(value: &[u8]);
        begin_array();
        end_array();
        begin_array_value(first: bool);
        end_array_value();
        begin_object();
        end_object();
        begin_object_key(first: bool);
        end_object_key();
        begin_object_value();
        end_object_value();
        write_raw_fragment(fragment: &str);
    }
}

/// Render a finite number per ECMAScript's `Number::toString` (radix 10), which is what
/// `JSON.stringify` uses. `shortest` is any shortest-round-trip scientific rendering of the
/// absolute value, e.g. `"1.2345e3"`; the sign is carried separately by `negative`.
fn js_number(shortest: &str, negative: bool, is_zero: bool) -> String {
    // JSON has no NaN/Infinity, and `JSON.stringify` turns them into null.
    if shortest.contains("inf") || shortest.contains("NaN") {
        return "null".to_owned();
    }
    // Covers -0 as well: JavaScript prints it as "0".
    if is_zero {
        return "0".to_owned();
    }

    let (mantissa, exponent) = match shortest.split_once('e') {
        Some(parts) => parts,
        None => return shortest.to_owned(),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    // value = 0.<digits> × 10^n
    let n: i64 = exponent.parse::<i64>().unwrap_or(0) + 1;
    let k = digits.len() as i64;
    let sign = if negative { "-" } else { "" };

    if n > 21 || n <= -6 {
        // Exponential: d.ddd e±(n-1)
        let mut out = String::with_capacity(digits.len() + 8);
        out.push_str(sign);
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let exp = n - 1;
        out.push('e');
        if exp >= 0 {
            out.push('+');
        }
        out.push_str(&exp.to_string());
        return out;
    }

    if n >= k {
        // Integral: digits followed by the remaining zeros (e.g. 1e20 → 21 digits).
        let mut out = String::with_capacity(n as usize + 1);
        out.push_str(sign);
        out.push_str(&digits);
        out.push_str(&"0".repeat((n - k) as usize));
        out
    } else if n > 0 {
        // A point falls inside the digits.
        format!("{sign}{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else {
        // Small: 0.00…digits
        format!("{sign}0.{}{}", "0".repeat((-n) as usize), digits)
    }
}
