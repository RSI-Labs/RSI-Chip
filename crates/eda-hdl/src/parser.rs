//! Recursive-descent Verilog parser (subset, see README for the supported
//! construct list).

use crate::ast::*;
use crate::lexer::{Lexer, Tok, Token};
use eda_core::logic::{LogicValue, LogicWord};

pub fn parse(source: &str) -> Result<SourceFile, String> {
    let toks = Lexer::tokenize(source)?;
    let mut p = Parser {
        toks,
        pos: 0,
        block_le: false,
        known_classes: Default::default(),
        class_specializations: Vec::new(),
        allow_symbolic_class_actual: false,
    };
    let mut modules = Vec::new();
    let mut packages = Vec::new();
    let mut imports = Vec::new();
    let mut classes = Vec::new();
    while !p.at_eof() {
        if p.at_kw("package") {
            packages.push(p.parse_package()?);
        } else if p.at_kw("import") {
            imports.extend(p.parse_import()?);
        } else if p.at_kw("interface") {
            modules.push(p.parse_interface()?);
        } else if p.at_kw("class")
            || (p.at_kw("virtual")
                && matches!(
                    p.toks.get(p.pos + 1).map(|token| &token.tok),
                    Some(Tok::Ident(word)) if word == "class"
                ))
        {
            let is_virtual = p.eat_kw("virtual");
            if let Item::ClassDef(definition) = p.parse_class(p.line(), is_virtual)? {
                classes.push(definition);
            }
        } else {
            modules.push(p.parse_module()?);
        }
    }
    Ok(SourceFile {
        modules,
        packages,
        imports,
        classes,
        class_specializations: p.class_specializations,
    })
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// While parsing a procedural assignment target, `<=` must be kept for
    /// the statement layer instead of being consumed as the `Le` operator.
    block_le: bool,
    known_classes: std::collections::HashSet<String>,
    class_specializations: Vec<crate::ast::ClassSpecialization>,
    allow_symbolic_class_actual: bool,
}

const DIRS: &[&str] = &["input", "output", "inout"];
const GATES: &[&str] = &["and", "nand", "or", "nor", "xor", "xnor", "buf", "not"];
const NET_TYPES: &[&str] = &[
    "wire", "tri", "reg", "logic", "bit", "byte", "shortint", "int", "longint", "integer", "string",
];
type PackedRange = (Box<Expr>, Box<Expr>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NetTypeInfo {
    is_variable: bool,
    is_signed: bool,
    default_width: usize,
}

mod declarations;
mod items;
mod statements;
mod utility;

fn is_reserved(s: &str) -> bool {
    matches!(
        s,
        "module"
            | "endmodule"
            | "interface"
            | "endinterface"
            | "virtual"
            | "modport"
            | "assert"
            | "property"
            | "disable"
            | "iff"
            | "package"
            | "endpackage"
            | "import"
            | "typedef"
            | "enum"
            | "union"
            | "function"
            | "endfunction"
            | "automatic"
            | "input"
            | "output"
            | "inout"
            | "wire"
            | "reg"
            | "tri"
            | "logic"
            | "bit"
            | "byte"
            | "shortint"
            | "int"
            | "longint"
            | "integer"
            | "signed"
            | "unsigned"
            | "parameter"
            | "localparam"
            | "generate"
            | "endgenerate"
            | "genvar"
            | "assign"
            | "always"
            | "initial"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "case"
            | "casez"
            | "casex"
            | "endcase"
            | "default"
            | "posedge"
            | "negedge"
            | "or"
            | "forever"
            | "and"
            | "nand"
            | "nor"
            | "xor"
            | "xnor"
            | "buf"
            | "not"
            | "always_comb"
            | "always_ff"
    )
}

fn default_range(width: usize) -> PackedRange {
    let msb = Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, width as u64 - 1),
        orig: (width - 1).to_string(),
    };
    let lsb = Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, 0),
        orig: "0".into(),
    };
    (Box::new(msb), Box::new(lsb))
}

fn numeric_class_actual(expression: &Expr) -> Option<i64> {
    match expression {
        Expr::Num { word, .. } => word.to_u64().map(|value| value as i64),
        Expr::Un(UnOpSrc::Minus, inner) => numeric_class_actual(inner).map(|value| -value),
        _ => None,
    }
}

fn symbolic_class_actual(expression: &Expr) -> Option<ClassParameterValue> {
    match expression {
        Expr::Num { .. } => numeric_class_actual(expression).map(ClassParameterValue::Constant),
        Expr::Un(UnOpSrc::Minus, inner) => numeric_class_actual(expression)
            .map(ClassParameterValue::Constant)
            .or_else(|| {
                symbolic_class_actual(inner).map(|value| match value {
                    ClassParameterValue::Constant(value) => ClassParameterValue::Constant(-value),
                    parameter => parameter,
                })
            }),
        Expr::Ident(name) => Some(ClassParameterValue::Parameter(name.clone())),
        _ => None,
    }
}

fn class_specialization_name(base_name: &str, values: &[ClassParameterValue]) -> String {
    let mut name = format!("__eda_class_{base_name}");
    for value in values {
        name.push('_');
        match value {
            ClassParameterValue::Constant(value) => {
                if *value < 0 {
                    name.push('n');
                }
                name.push_str(&value.unsigned_abs().to_string());
            }
            ClassParameterValue::Parameter(parameter) => name.push_str(parameter),
        }
    }
    name
}

fn parse_format(
    fmt: &str,
    default_spec: char,
    args: &[Expr],
    line: usize,
) -> Result<Vec<DispPartSrc>, String> {
    let mut parts = Vec::new();
    let mut lit = String::new();
    let mut argi = 0usize;
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.peek() == Some(&'%') {
                chars.next();
                lit.push('%');
                continue;
            }
            let mut spec = chars.next().unwrap_or(default_spec);
            // 跳过 %0 之类的宽度修饰（本子集无填充语义差异）。
            if spec == '0' {
                spec = chars.next().unwrap_or(default_spec);
            }
            let spec = match spec {
                'b' | 'B' => 'b',
                'd' | 'D' => 'd',
                'h' | 'H' => 'h',
                's' | 'S' => 's',
                't' | 'T' => {
                    parts.push(DispPartSrc::Str(std::mem::take(&mut lit)));
                    parts.push(DispPartSrc::Time);
                    continue;
                }
                _ => default_spec,
            };
            parts.push(DispPartSrc::Str(std::mem::take(&mut lit)));
            parts.push(DispPartSrc::Arg(spec));
            argi += 1;
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        parts.push(DispPartSrc::Str(lit));
    }
    if argi != args.len() {
        return Err(format!(
            "line {}: $display has {} format specifiers but {} arguments",
            line,
            argi,
            args.len()
        ));
    }
    Ok(parts)
}

/// Build a sized [`LogicWord`] from a based literal digit string.
pub fn build_word(width: usize, radix: u32, text: &str) -> Result<LogicWord, String> {
    if radix == 10 {
        if text
            .chars()
            .any(|c| matches!(c, 'x' | 'X' | 'z' | 'Z' | '?'))
        {
            return Err("decimal literals cannot contain x/z".into());
        }
        let v: u64 = text
            .parse()
            .map_err(|_| format!("bad decimal literal '{}'", text))?;
        return Ok(LogicWord::from_u64(width, v));
    }
    let mut flat: Vec<LogicValue> = Vec::new(); // big-endian bit list
    for c in text.chars() {
        let n = match radix {
            2 => 1,
            8 => 3,
            16 => 4,
            _ => 1,
        };
        if matches!(c, 'x' | 'X' | 'z' | 'Z' | '?') {
            let v = if c == '?' {
                LogicValue::Z
            } else {
                LogicValue::from_char(c).unwrap()
            };
            for _ in 0..n {
                flat.push(v);
            }
        } else {
            let d = c
                .to_digit(radix)
                .ok_or_else(|| format!("digit '{}' invalid for radix {}", c, radix))?;
            for k in (0..n).rev() {
                flat.push(LogicValue::bit((d >> k) & 1 == 1));
            }
        }
    }
    let mut bits = vec![LogicValue::Zero; width];
    for i in 0..width.min(flat.len()) {
        bits[i] = flat[flat.len() - 1 - i];
    }
    Ok(LogicWord { bits })
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;
