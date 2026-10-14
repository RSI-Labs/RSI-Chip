//! Four-valued logic and multi-bit word test cases.

use crate::{Case, CaseList};
use eda_core::logic::{LogicValue as LV, LogicWord as LW};

pub fn register(c: &mut CaseList) {
    truth_tables(c);
    word_arith(c);
    word_shifts(c);
    parse_format(c);
    reductions_misc(c);
}

fn truth_tables(c: &mut CaseList) {
    let vals = [LV::Zero, LV::One, LV::X, LV::Z];
    for (name, f) in [
        ("and", (|a: LV, b: LV| a.and(b)) as fn(LV, LV) -> LV),
        ("or", |a: LV, b: LV| a.or(b)),
        ("xor", |a: LV, b: LV| a.xor(b)),
        ("resolve", |a: LV, b: LV| a.resolve(b)),
        ("caseeq", |a: LV, b: LV| a.case_eq(b)),
    ] {
        for a in vals {
            for b in vals {
                // Reference semantics per Verilog: recompute independently.
                let expected = match name {
                    "and" => ref_and(a, b),
                    "or" => ref_or(a, b),
                    "xor" => ref_xor(a, b),
                    "resolve" => ref_resolve(a, b),
                    _ => LV::bit(a == b),
                };
                c.add(format!("logic_{}_{}{}", name, ch(a), ch(b)), move || {
                    let got = f(a, b);
                    if got != expected {
                        Err(format!(
                            "{}({:?},{:?}) = {:?}, want {:?}",
                            name, a, b, got, expected
                        ))
                    } else {
                        Ok(())
                    }
                });
            }
        }
    }
    // Not over all values.
    for a in vals {
        let expected = match a {
            LV::Zero => LV::One,
            LV::One => LV::Zero,
            _ => LV::X,
        };
        c.add(format!("logic_not_{}", ch(a)), move || {
            if a.not() != expected {
                Err(format!("not({:?})", a))
            } else {
                Ok(())
            }
        });
    }
    // Predicates.
    for a in vals {
        let unk = matches!(a, LV::X | LV::Z);
        c.add(format!("logic_pred_{}", ch(a)), move || {
            if a.is_unknown() != unk || a.is_01() == unk || a.as_bool().is_some() == unk {
                Err(format!("predicates({:?})", a))
            } else {
                Ok(())
            }
        });
    }
    // char round-trips.
    for a in vals {
        c.add(format!("logic_char_roundtrip_{}", ch(a)), move || {
            let cc = a.to_char();
            if LV::from_char(cc) != Some(a) {
                Err(format!("char roundtrip {:?}", a))
            } else {
                Ok(())
            }
        });
    }
    if LV::from_char('q').is_some() {
        c.add("logic_char_invalid", || {
            Err("from_char('q') should be None".into())
        });
    } else {
        c.add("logic_char_invalid", || Ok(()));
    }
}

fn ch(v: LV) -> char {
    v.to_char();
    match v {
        LV::Zero => '0',
        LV::One => '1',
        LV::X => 'x',
        LV::Z => 'z',
    }
}

fn norm(v: LV) -> LV {
    if matches!(v, LV::X | LV::Z) {
        LV::X
    } else {
        v
    }
}
fn ref_and(a: LV, b: LV) -> LV {
    let (a, b) = (norm(a), norm(b));
    if a == LV::Zero || b == LV::Zero {
        LV::Zero
    } else if a == LV::One && b == LV::One {
        LV::One
    } else {
        LV::X
    }
}
fn ref_or(a: LV, b: LV) -> LV {
    let (a, b) = (norm(a), norm(b));
    if a == LV::One || b == LV::One {
        LV::One
    } else if a == LV::Zero && b == LV::Zero {
        LV::Zero
    } else {
        LV::X
    }
}
fn ref_xor(a: LV, b: LV) -> LV {
    match (norm(a).as_bool(), norm(b).as_bool()) {
        (Some(x), Some(y)) => LV::bit(x ^ y),
        _ => LV::X,
    }
}
fn ref_resolve(a: LV, b: LV) -> LV {
    match (a, b) {
        (LV::Z, x) => x,
        (x, LV::Z) => x,
        (x, y) if x == y => x,
        _ => LV::X,
    }
}

fn word_arith(c: &mut CaseList) {
    // Exhaustive 4-bit add/sub/mul (256+256+256 cases worth of checks,
    // emitted as one case per value pair for a 4x4 grid + sampled wider).
    for a in 0..16u64 {
        for b in 0..16u64 {
            let (a, b) = (a, b);
            c.add(format!("word_add_{}_{}", a, b), move || {
                let x = LW::from_u64(4, a);
                let y = LW::from_u64(4, b);
                let want = (a.wrapping_add(b)) & 0xF;
                if x.add(&y).to_u64() != Some(want) {
                    Err(format!("{}+{} wrong", a, b))
                } else {
                    Ok(())
                }
            });
        }
    }
    for a in 0..16u64 {
        for b in 0..16u64 {
            c.add(format!("word_sub_{}_{}", a, b), move || {
                let x = LW::from_u64(4, a);
                let y = LW::from_u64(4, b);
                let want = (a.wrapping_sub(b)) & 0xF;
                if x.sub(&y).to_u64() != Some(want) {
                    Err(format!("{}-{} wrong", a, b))
                } else {
                    Ok(())
                }
            });
        }
    }
    for a in 0..16u64 {
        for b in 0..16u64 {
            c.add(format!("word_mul_{}_{}", a, b), move || {
                let x = LW::from_u64(4, a);
                let y = LW::from_u64(4, b);
                let want = (a.wrapping_mul(b)) & 0xF;
                if x.mul(&y).to_u64() != Some(want) {
                    Err(format!("{}*{} wrong", a, b))
                } else {
                    Ok(())
                }
            });
        }
    }
    // Unknown propagation on arithmetic.
    for op in 0..3 {
        c.add(format!("word_arith_x_{}", op), move || {
            let x = LW::xes(4);
            let y = LW::from_u64(4, 3);
            let r = match op {
                0 => x.add(&y),
                1 => x.sub(&y),
                _ => x.mul(&y),
            };
            if r.is_fully_known() || r.width() != 4 {
                Err("arith with X should be all-X".into())
            } else {
                Ok(())
            }
        });
    }
    // Width extension rule: result width = max operand width.
    c.add("word_add_width", || {
        let a = LW::from_u64(4, 3);
        let b = LW::from_u64(8, 5);
        let r = a.add(&b);
        if r.width() != 8 || r.to_u64() != Some(8) {
            Err(format!("width/val wrong: {:?}", r))
        } else {
            Ok(())
        }
    });
    for (msb, lsb, want) in [
        (3usize, 1usize, 0b101u64),
        (2, 0, 0b011),
        (3, 3, 1),
        (0, 0, 1),
    ] {
        c.add(format!("word_slice_{}_{}", msb, lsb), move || {
            let w = LW::from_u64(4, 0b1011);
            let s = w.slice(msb, lsb);
            if s.width() != msb - lsb + 1 || s.to_u64() != Some(want) {
                Err(format!("slice [{},{}] = {:?}", msb, lsb, s))
            } else {
                Ok(())
            }
        });
    }
    for (n, want) in [(2usize, 0b11u64), (6, 0b01011), (3, 0b011)] {
        c.add(format!("word_sized_{}", n), move || {
            let w = LW::from_u64(4, 0b1011);
            let s = w.sized(n);
            if s.width() != n || s.to_u64() != Some(want) {
                Err(format!("sized({}) = {:?}", n, s))
            } else {
                Ok(())
            }
        });
    }
    // Out-of-range bit reads X.
    c.add("word_oob_bit_is_x", || {
        let w = LW::from_u64(4, 0xF);
        if w.bit(7) != LV::X || w.bit(3) != LV::One {
            Err("oob bit".into())
        } else {
            Ok(())
        }
    });
}

fn word_shifts(c: &mut CaseList) {
    for v in [0u64, 1, 0b1011, 0xF] {
        for s in 0..5u64 {
            let (v, s) = (v, s);
            c.add(format!("word_shl_{}_{}", v, s), move || {
                let w = LW::from_u64(4, v);
                let want = if s >= 4 { 0 } else { (v << s) & 0xF };
                if w.shl(&LW::from_u64(3, s)).to_u64() != Some(want) {
                    Err(format!("{}<<{}", v, s))
                } else {
                    Ok(())
                }
            });
            c.add(format!("word_shr_{}_{}", v, s), move || {
                let w = LW::from_u64(4, v);
                let want = if s >= 4 { 0 } else { v >> s };
                if w.shr(&LW::from_u64(3, s)).to_u64() != Some(want) {
                    Err(format!("{}>>{}", v, s))
                } else {
                    Ok(())
                }
            });
        }
    }
    c.add("word_shl_x_amount", || {
        let w = LW::from_u64(4, 0b1010);
        let r = w.shl(&LW::xes(2));
        if r.is_fully_known() {
            Err("shift by X must be X".into())
        } else {
            Ok(())
        }
    });
    // Concatenation MSB-first semantics.
    for (hi, lo, want) in [(0xAu64, 0x5, 0xA5u64), (0, 0, 0), (0xF, 1, 0xF1)] {
        c.add(format!("word_concat_{}_{}", hi, lo), move || {
            let h = LW::from_u64(4, hi);
            let l = LW::from_u64(4, lo);
            let r = LW::concat(&[&h, &l]);
            if r.width() != 8 || r.to_u64() != Some(want) {
                Err(format!("concat = {:?}", r.to_hex_str()))
            } else {
                Ok(())
            }
        });
    }
    c.add("word_concat_three", || {
        let a = LW::from_u64(2, 0b10);
        let b = LW::ones(1);
        let d = LW::zero(1);
        let r = LW::concat(&[&a, &b, &d]);
        if r.to_bin_str() != "1010" {
            Err(r.to_bin_str())
        } else {
            Ok(())
        }
    });
}

fn parse_format(c: &mut CaseList) {
    for (text, want) in [
        ("0", 0u64),
        ("1", 1),
        ("10", 10),
        ("255", 255),
        ("4", 4),
        ("0x1f", 31),
        ("0xFF", 255),
        ("'h1f", 31),
        ("'d42", 42),
        ("'b1010", 10),
    ] {
        c.add(format!("parse_ok_{}", text), move || {
            match LW::parse(8, text) {
                Some(w) if w.to_u64() == Some(want) => Ok(()),
                other => Err(format!(
                    "parse({}) = {:?}",
                    text,
                    other.map(|w| w.to_hex_str())
                )),
            }
        });
    }
    for (text, width, want_hex) in [
        ("'hzz", 8usize, "zz"),
        ("'b1x01", 4, "x"),
        ("'h2x", 8, "2x"),
        ("'bxx", 2, "x"),
        ("'hz_z", 8, "zz"),
    ] {
        c.add(format!("parse_xz_{}", text), move || {
            match LW::parse(width, text) {
                Some(w) if w.to_hex_str() == want_hex => Ok(()),
                other => Err(format!(
                    "parse({}) = {:?}",
                    text,
                    other.map(|w| w.to_hex_str())
                )),
            }
        });
    }
    for text in ["abc", "0x", "'q12", "-5", "1.5"] {
        c.add(format!("parse_err_{}", text), move || {
            if LW::parse(8, text).is_some() {
                Err(format!("parse({}) should fail", text))
            } else {
                Ok(())
            }
        });
    }
    for (v, bin) in [(0u64, "0000"), (5, "0101"), (0xA, "1010"), (0xF, "1111")] {
        c.add(format!("fmt_bin_{}", v), move || {
            if LW::from_u64(4, v).to_bin_str() != bin {
                Err("bin str".into())
            } else {
                Ok(())
            }
        });
    }
    for (v, hex) in [(0u64, "0"), (5, "5"), (0xAB, "ab"), (0xF0, "f0")] {
        c.add(format!("fmt_hex_{}", v), move || {
            if LW::from_u64(8, v).to_hex_str() != hex {
                Err("hex str".into())
            } else {
                Ok(())
            }
        });
    }
    // Hex of odd width rounds up to nibbles.
    if LW::from_u64(5, 0b10111).to_hex_str() != "17" {
        c.add("fmt_hex_odd_width", || {
            Err("5-bit 10111 should print 17".into())
        });
    } else {
        c.add("fmt_hex_odd_width", || Ok(()));
    }
    // 64-bit values.
    c.add("word_u64_roundtrip", || {
        let v = u64::MAX;
        let w = LW::from_u64(64, v);
        if w.to_u64() != Some(v) {
            Err("u64 roundtrip".into())
        } else {
            Ok(())
        }
    });
}

fn reductions_misc(c: &mut CaseList) {
    for v in 0..16u64 {
        c.add(format!("word_reduce_{}", v), move || {
            let w = LW::from_u64(4, v);
            let ones = v.count_ones();
            let and = if ones == 4 { LV::One } else { LV::Zero };
            let or = if ones > 0 { LV::One } else { LV::Zero };
            let xor = LV::bit(ones % 2 == 1);
            if w.reduce_and() != and || w.reduce_or() != or || w.reduce_xor() != xor {
                Err(format!("reduce {}", v))
            } else {
                Ok(())
            }
        });
    }
    // Empty-word reductions (identity elements).
    c.add("word_reduce_empty", || {
        let w = LW::zero(0);
        if w.reduce_and() != LV::One || w.reduce_or() != LV::Zero || w.reduce_xor() != LV::Zero {
            Err("empty reduce".into())
        } else {
            Ok(())
        }
    });
    // map_bits / zip_bits width handling.
    c.add("word_zip_width", || {
        let a = LW::from_u64(4, 0b1010);
        let b = LW::from_u64(2, 0b01);
        let z = a.zip_bits(&b, |x, y| x.and(y));
        if z.width() != 4 || z.to_bin_str() != "0000" {
            Err(format!(
                "{} a={} b={}",
                z.to_bin_str(),
                a.to_bin_str(),
                b.to_bin_str()
            ))
        } else {
            Ok(())
        }
    });
    // Z hex formatting.
    if LW::parse(8, "'hzz").unwrap().to_hex_str() != "zz" {
        c.add("word_hex_z", || Err("z hex".into()));
    } else {
        c.add("word_hex_z", || Ok(()));
    }
    let _ = Case {
        name: String::new(),
        run: Box::new(|| Ok(())),
    };
}
