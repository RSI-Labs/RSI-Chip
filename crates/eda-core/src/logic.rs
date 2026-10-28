//! Four-valued logic (0/1/X/Z) with Verilog operator semantics, plus a
//! multi-bit [`LogicWord`] stored LSB-first.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LogicValue {
    Zero,
    One,
    X,
    Z,
}

impl LogicValue {
    pub fn bit(b: bool) -> Self {
        if b {
            LogicValue::One
        } else {
            LogicValue::Zero
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '0' => Some(LogicValue::Zero),
            '1' => Some(LogicValue::One),
            'x' | 'X' => Some(LogicValue::X),
            'z' | 'Z' => Some(LogicValue::Z),
            _ => None,
        }
    }

    pub fn to_char(self) -> char {
        match self {
            LogicValue::Zero => '0',
            LogicValue::One => '1',
            LogicValue::X => 'x',
            LogicValue::Z => 'z',
        }
    }

    /// True for X and Z (both treated as "unknown" by binary operators).
    pub fn is_unknown(self) -> bool {
        matches!(self, LogicValue::X | LogicValue::Z)
    }

    pub fn is_01(self) -> bool {
        !self.is_unknown()
    }

    pub fn as_bool(self) -> Option<bool> {
        match self {
            LogicValue::Zero => Some(false),
            LogicValue::One => Some(true),
            _ => None,
        }
    }

    /// Verilog `&`: 0 dominates, X is pessimistic.
    pub fn and(self, o: LogicValue) -> LogicValue {
        use LogicValue::*;
        let a = self.xz_as_x();
        let b = o.xz_as_x();
        match (a, b) {
            (Zero, _) | (_, Zero) => Zero,
            (One, One) => One,
            _ => X,
        }
    }

    /// Verilog `|`: 1 dominates.
    pub fn or(self, o: LogicValue) -> LogicValue {
        use LogicValue::*;
        let a = self.xz_as_x();
        let b = o.xz_as_x();
        match (a, b) {
            (One, _) | (_, One) => One,
            (Zero, Zero) => Zero,
            _ => X,
        }
    }

    pub fn xor(self, o: LogicValue) -> LogicValue {
        match (self.xz_as_x().as_bool(), o.xz_as_x().as_bool()) {
            (Some(a), Some(b)) => LogicValue::bit(a ^ b),
            _ => LogicValue::X,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> LogicValue {
        match self.xz_as_x() {
            LogicValue::Zero => LogicValue::One,
            LogicValue::One => LogicValue::Zero,
            _ => LogicValue::X,
        }
    }

    /// Resolution of two drivers on one net (tri-state-ish, no strengths).
    pub fn resolve(self, o: LogicValue) -> LogicValue {
        use LogicValue::*;
        match (self, o) {
            (Z, b) => b,
            (a, Z) => a,
            (a, b) if a == b => a,
            _ => X,
        }
    }

    fn xz_as_x(self) -> LogicValue {
        if self.is_unknown() {
            LogicValue::X
        } else {
            self
        }
    }

    /// Case equality (`===`): always yields 0/1.
    pub fn case_eq(self, o: LogicValue) -> LogicValue {
        LogicValue::bit(self == o)
    }
}

impl fmt::Display for LogicValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_char())
    }
}

impl std::ops::Not for LogicValue {
    type Output = Self;

    fn not(self) -> Self {
        match self {
            Self::Zero => Self::One,
            Self::One => Self::Zero,
            _ => Self::X,
        }
    }
}

/// A packed vector of logic values. Index 0 is the LSB.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct LogicWord {
    pub bits: Vec<LogicValue>,
}

impl LogicWord {
    pub fn new_const(width: usize, v: LogicValue) -> Self {
        LogicWord {
            bits: vec![v; width],
        }
    }

    pub fn zero(width: usize) -> Self {
        Self::new_const(width, LogicValue::Zero)
    }

    pub fn ones(width: usize) -> Self {
        Self::new_const(width, LogicValue::One)
    }

    pub fn xes(width: usize) -> Self {
        Self::new_const(width, LogicValue::X)
    }

    pub fn from_u64(width: usize, val: u64) -> Self {
        let mut bits = Vec::with_capacity(width);
        for i in 0..width {
            let bit_is_set = i < u64::BITS as usize && (val >> i) & 1 == 1;
            bits.push(LogicValue::bit(bit_is_set));
        }
        LogicWord { bits }
    }

    /// Pack bytes into a fixed-width little-endian byte vector. Missing high
    /// bytes are zero-filled.
    pub fn from_bytes_le(width: usize, bytes: &[u8]) -> Self {
        let mut bits = Vec::with_capacity(width);
        for byte in bytes {
            for bit in 0..8 {
                bits.push(LogicValue::bit((byte >> bit) & 1 == 1));
            }
        }
        bits.resize(width, LogicValue::Zero);
        LogicWord { bits }
    }

    /// Decode the low bytes of a NUL-terminated packed string. Unknown bits
    /// contribute zero to their byte, matching the existing display subset.
    pub fn to_ascii_string_nul_terminated(&self) -> String {
        let mut out = String::new();
        for byte_bits in self.bits.chunks(8) {
            let mut byte = 0_u8;
            for (bit, value) in byte_bits.iter().enumerate() {
                if value.as_bool() == Some(true) {
                    byte |= 1 << bit;
                }
            }
            if byte == 0 {
                break;
            }
            out.push(byte as char);
        }
        out
    }

    pub fn width(&self) -> usize {
        self.bits.len()
    }

    /// None if any bit is X/Z.
    pub fn to_u64(&self) -> Option<u64> {
        if self.width() > 64 {
            return None;
        }
        let mut v = 0u64;
        for (i, b) in self.bits.iter().enumerate() {
            match b.as_bool() {
                Some(true) => v |= 1 << i,
                Some(false) => {}
                None => return None,
            }
        }
        Some(v)
    }

    pub fn is_fully_known(&self) -> bool {
        self.bits.iter().all(|b| b.is_01())
    }

    /// MSB-first binary string, e.g. width 4 value 5 -> "0101".
    pub fn to_bin_str(&self) -> String {
        self.bits.iter().rev().map(|b| b.to_char()).collect()
    }

    /// MSB-first hex string (width rounded up to nibbles).
    pub fn to_hex_str(&self) -> String {
        if self.bits.is_empty() {
            return "0".into();
        }
        if !self.is_fully_known() {
            let mut s = String::new();
            for nib in nibbles_msb_first(self.width()) {
                let slice = &self.bits[nib.start..nib.end];
                if slice.iter().all(|b| b.is_01()) {
                    let mut v = 0u8;
                    for (k, b) in slice.iter().enumerate() {
                        if b.as_bool() == Some(true) {
                            v |= 1 << k;
                        }
                    }
                    s.push(char::from_digit(v as u32, 16).unwrap());
                } else if slice.iter().all(|b| *b == LogicValue::Z) {
                    s.push('z');
                } else {
                    s.push('x');
                }
            }
            return s;
        }
        format!("{:x}", self.to_u64().unwrap_or(0))
    }

    /// Parse a user-supplied literal: "10" (decimal), "0b1010", "0x1f",
    /// "'h1f" or "'b1010". Truncates / zero-extends to `width`.
    pub fn parse(width: usize, text: &str) -> Option<Self> {
        let t = text.trim();
        let (radix, digits) = if let Some(rest) = t.strip_prefix("0b").or(t.strip_prefix("0B")) {
            (2, rest)
        } else if let Some(rest) = t.strip_prefix("0x").or(t.strip_prefix("0X")) {
            (16, rest)
        } else if let Some(rest) = t.strip_prefix("'b").or(t.strip_prefix("'B")) {
            (2, rest)
        } else if let Some(rest) = t.strip_prefix("'h").or(t.strip_prefix("'H")) {
            (16, rest)
        } else if let Some(rest) = t.strip_prefix("'d").or(t.strip_prefix("'D")) {
            (10, rest)
        } else {
            (10, t)
        };
        if digits.is_empty() {
            return None;
        }
        let mut bits = vec![LogicValue::Zero; width];
        if radix == 10 {
            let v: u64 = digits.parse().ok()?;
            for (i, bit) in bits.iter_mut().enumerate().take(width) {
                let set = i < u64::BITS as usize && (v >> i) & 1 == 1;
                *bit = LogicValue::bit(set);
            }
            return Some(LogicWord { bits });
        }
        // Binary / hex digit strings may contain x/z (and '_' separators).
        let mut flat: Vec<LogicValue> = Vec::new();
        for c in digits.chars() {
            if c == '_' {
                continue;
            }
            let n = if radix == 2 { 1 } else { 4 };
            if matches!(c, 'x' | 'X' | 'z' | 'Z') {
                let v = LogicValue::from_char(c).unwrap();
                for _ in 0..n {
                    flat.push(v);
                }
            } else {
                let d = c.to_digit(radix)?;
                // push MSB-first so the whole string is a big-endian bit list
                for k in (0..n).rev() {
                    flat.push(LogicValue::bit((d >> k) & 1 == 1));
                }
            }
        }
        let n = flat.len();
        for i in 0..width {
            if i < n {
                bits[i] = flat[n - 1 - i];
            }
        }
        Some(LogicWord { bits })
    }

    pub fn bit(&self, i: usize) -> LogicValue {
        if i < self.bits.len() {
            self.bits[i]
        } else {
            LogicValue::X
        }
    }

    pub fn slice(&self, msb: usize, lsb: usize) -> LogicWord {
        let mut b = Vec::new();
        let mut i = lsb;
        while i <= msb && i < self.bits.len() {
            b.push(self.bits[i]);
            i += 1;
        }
        while b.len() < msb.saturating_sub(lsb) + 1 {
            b.push(LogicValue::X);
        }
        LogicWord { bits: b }
    }

    /// Truncate or zero-extend to exactly `width` bits.
    pub fn sized(&self, width: usize) -> LogicWord {
        let mut bits: Vec<LogicValue> = self.bits.clone();
        bits.truncate(width);
        while bits.len() < width {
            bits.push(LogicValue::Zero);
        }
        LogicWord { bits }
    }

    pub fn map_bits(&self, mut f: impl FnMut(LogicValue) -> LogicValue) -> LogicWord {
        LogicWord {
            bits: self.bits.iter().map(|b| f(*b)).collect(),
        }
    }

    pub fn zip_bits(
        &self,
        o: &LogicWord,
        mut f: impl FnMut(LogicValue, LogicValue) -> LogicValue,
    ) -> LogicWord {
        let w = self.width().max(o.width());
        let mut bits = Vec::with_capacity(w);
        for i in 0..w {
            let a = self.bit(i);
            let b = o.bit(i);
            // Zipping is a sized vector operation: operands shorter than the
            // result are zero-extended, not treated as X beyond their width.
            bits.push(f(
                if a.is_unknown() && i >= self.width() {
                    LogicValue::Zero
                } else {
                    a
                },
                if b.is_unknown() && i >= o.width() {
                    LogicValue::Zero
                } else {
                    b
                },
            ));
        }
        LogicWord { bits }
    }

    pub fn reduce_and(&self) -> LogicValue {
        self.bits
            .iter()
            .copied()
            .reduce(|a, b| a.and(b))
            .unwrap_or(LogicValue::One)
    }

    pub fn reduce_or(&self) -> LogicValue {
        self.bits
            .iter()
            .copied()
            .reduce(|a, b| a.or(b))
            .unwrap_or(LogicValue::Zero)
    }

    pub fn reduce_xor(&self) -> LogicValue {
        self.bits
            .iter()
            .copied()
            .reduce(|a, b| a.xor(b))
            .unwrap_or(LogicValue::Zero)
    }

    /// Verilog `+`: if any input bit is unknown the whole result is X.
    pub fn add(&self, o: &LogicWord) -> LogicWord {
        if !self.is_fully_known() || !o.is_fully_known() {
            return LogicWord::xes(self.width().max(o.width()));
        }
        let w = self.width().max(o.width());
        let a = zext(self, w);
        let b = zext(o, w);
        let (av, bv) = (a.to_u64().unwrap_or(0), b.to_u64().unwrap_or(0));
        LogicWord::from_u64(w, av.wrapping_add(bv))
    }

    pub fn sub(&self, o: &LogicWord) -> LogicWord {
        if !self.is_fully_known() || !o.is_fully_known() {
            return LogicWord::xes(self.width().max(o.width()));
        }
        let w = self.width().max(o.width());
        let a = zext(self, w);
        let b = zext(o, w);
        let (av, bv) = (a.to_u64().unwrap_or(0), b.to_u64().unwrap_or(0));
        LogicWord::from_u64(w, av.wrapping_sub(bv))
    }

    pub fn mul(&self, o: &LogicWord) -> LogicWord {
        if !self.is_fully_known() || !o.is_fully_known() {
            return LogicWord::xes(self.width().max(o.width()));
        }
        let w = self.width().max(o.width());
        let a = zext(self, w);
        let b = zext(o, w);
        let (av, bv) = (a.to_u64().unwrap_or(0), b.to_u64().unwrap_or(0));
        LogicWord::from_u64(w, av.wrapping_mul(bv))
    }

    /// Integer power modulo the result width. A signed negative exponent is
    /// truncated to zero, matching integral SystemVerilog semantics.
    pub fn pow(&self, exponent: &LogicWord, exponent_signed: bool) -> LogicWord {
        let width = self.width().max(exponent.width());
        if !self.is_fully_known() || !exponent.is_fully_known() {
            return LogicWord::xes(width);
        }
        if width > u64::BITS as usize {
            // Existing packed arithmetic is deliberately built around 64-bit
            // integer evaluation. Fail closed rather than silently truncating
            // a wider exponent or result.
            return LogicWord::xes(width);
        }
        if exponent_signed && exponent.bit(exponent.width() - 1) == LogicValue::One {
            return LogicWord::zero(width);
        }
        let Some(mut remaining) = exponent.to_u64() else {
            return LogicWord::xes(width);
        };
        let mut result = LogicWord::from_u64(width, 1);
        let mut power = self.sized(width);
        while remaining != 0 {
            if remaining & 1 == 1 {
                result = result.mul(&power);
            }
            remaining >>= 1;
            if remaining != 0 {
                power = power.mul(&power);
            }
        }
        result
    }

    pub fn shl(&self, o: &LogicWord) -> LogicWord {
        if !o.is_fully_known() {
            return LogicWord::xes(self.width());
        }
        let sh = o.to_u64().unwrap_or(0) as usize;
        if sh >= self.width() {
            return LogicWord::zero(self.width());
        }
        let mut bits = vec![LogicValue::Zero; self.width()];
        bits[sh..self.width()].copy_from_slice(&self.bits[..self.width() - sh]);
        LogicWord { bits }
    }

    pub fn shr(&self, o: &LogicWord) -> LogicWord {
        if !o.is_fully_known() {
            return LogicWord::xes(self.width());
        }
        let sh = o.to_u64().unwrap_or(0) as usize;
        if sh >= self.width() {
            return LogicWord::zero(self.width());
        }
        let mut bits = vec![LogicValue::Zero; self.width()];
        bits[..self.width() - sh].copy_from_slice(&self.bits[sh..]);
        LogicWord { bits }
    }

    pub fn concat(parts: &[&LogicWord]) -> LogicWord {
        // parts are MSB-first per Verilog concatenation.
        let mut bits = Vec::new();
        for p in parts.iter().rev() {
            bits.extend_from_slice(&p.bits);
        }
        LogicWord { bits }
    }
}

fn zext(w: &LogicWord, width: usize) -> LogicWord {
    let mut bits = w.bits.clone();
    while bits.len() < width {
        bits.push(LogicValue::Zero);
    }
    LogicWord { bits }
}

struct NibbleSpan {
    start: usize,
    end: usize,
}

fn nibbles_msb_first(width: usize) -> Vec<NibbleSpan> {
    let mut out = Vec::new();
    let mut end = width;
    while end > 0 {
        let start = end.saturating_sub(4);
        out.push(NibbleSpan { start, end });
        end = start;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_valued_ops() {
        let z = LogicValue::Zero;
        let o = LogicValue::One;
        let x = LogicValue::X;
        assert_eq!(z.and(x), z);
        assert_eq!(o.and(x), x);
        assert_eq!(z.or(x), x);
        assert_eq!(o.or(x), o);
        assert_eq!(x.xor(o), x);
        assert_eq!(LogicValue::Z.and(z), z);
        assert_eq!(LogicValue::Z.and(o), x);
        assert_eq!(z.not(), o);
        assert_eq!(x.not(), x);
    }

    #[test]
    fn resolve() {
        use LogicValue::*;
        assert_eq!(Z.resolve(One), One);
        assert_eq!(One.resolve(Z), One);
        assert_eq!(Zero.resolve(One), X);
        assert_eq!(X.resolve(One), X);
    }

    #[test]
    fn word_arith() {
        let a = LogicWord::from_u64(8, 250);
        let b = LogicWord::from_u64(8, 10);
        assert_eq!(a.add(&b).to_u64(), Some(4)); // wraps
        assert_eq!(a.sub(&b).to_u64(), Some(240));
        let c = LogicWord::from_u64(4, 15);
        assert_eq!(c.mul(&c).to_u64(), Some(1)); // 15*15=225 truncated to 4 bits
        let x = LogicWord::xes(4);
        assert!(!x.add(&c).is_fully_known());
    }

    #[test]
    fn word_power() {
        let base = LogicWord::from_u64(8, 3);
        let exponent = LogicWord::from_u64(8, 4);
        assert_eq!(base.pow(&exponent, false).to_u64(), Some(81));
        assert_eq!(base.pow(&LogicWord::zero(8), false).to_u64(), Some(1));
        assert_eq!(
            base.pow(&LogicWord::from_u64(8, 0xff), true).to_u64(),
            Some(0)
        );
        assert!(!base.pow(&LogicWord::xes(8), false).is_fully_known());
    }

    #[test]
    fn word_shift_concat() {
        let a = LogicWord::from_u64(8, 0b1011_0010);
        let s1 = LogicWord::from_u64(4, 1);
        assert_eq!(a.shl(&s1).to_u64(), Some(0b0110_0100));
        assert_eq!(a.shr(&s1).to_u64(), Some(0b0101_1001));
        let hi = LogicWord::from_u64(4, 0xA);
        let lo = LogicWord::from_u64(4, 0x5);
        assert_eq!(LogicWord::concat(&[&hi, &lo]).to_hex_str(), "a5");
    }

    #[test]
    fn parse_literals() {
        assert_eq!(LogicWord::parse(8, "10").unwrap().to_u64(), Some(10));
        assert_eq!(LogicWord::parse(8, "0x1f").unwrap().to_u64(), Some(31));
        assert_eq!(LogicWord::parse(8, "'b1010").unwrap().to_u64(), Some(10));
        assert_eq!(LogicWord::parse(8, "'hzz").unwrap().to_hex_str(), "zz");
        assert_eq!(LogicWord::parse(4, "20").unwrap().to_u64(), Some(4)); // truncate
        assert_eq!(
            LogicWord::parse(65, "0").unwrap().to_bin_str(),
            LogicWord::zero(65).to_bin_str()
        );
        assert!(LogicWord::parse(4, "abc").is_none());
    }

    #[test]
    fn reductions_and_hex() {
        let w = LogicWord::parse(8, "'b1011_0010").unwrap();
        assert_eq!(w.reduce_and(), LogicValue::Zero);
        assert_eq!(w.reduce_or(), LogicValue::One);
        assert_eq!(w.reduce_xor(), LogicValue::Zero); // 4 ones -> even parity
        assert_eq!(w.to_hex_str(), "b2");
        let xx = LogicWord::parse(8, "'h2x").unwrap();
        assert_eq!(xx.to_hex_str(), "2x");
        assert!(!xx.is_fully_known());
    }
}
