//! Elaborated RTL IR — the typed, name-resolved design produced by the HDL
//! frontend, consumed by the simulator and the synthesizer.
//!
//! Nets are flattened to single bits: declaration `d` of width `w` occupies
//! flat ids `[base, base + w)` with the LSB at `base`. `Expr::Ref` stores the
//! declaration base, the declaration width and a range within it.

use crate::logic::LogicWord;

/// Range within a net declaration. Indices are declaration-relative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Range {
    Bit(usize),
    Span { msb: usize, lsb: usize },
}

impl Range {
    pub fn width(&self) -> usize {
        match self {
            Range::Bit(_) => 1,
            Range::Span { msb, lsb } => msb - lsb + 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct NetDecl {
    pub name: String,
    pub width: usize,
    pub is_reg: bool,
    pub is_signed: bool,
    /// Flat base id; bit `i` (LSB=0) is at `base + i`.
    pub base: usize,
}

#[derive(Clone, Debug)]
pub struct PortDecl {
    pub name: String,
    pub dir: Dir,
    pub base: usize,
    pub width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    AndR,
    OrR,
    XorR,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    LogAnd,
    LogOr,
    CaseEq,
    CaseNe,
}

#[derive(Clone, Debug)]
pub enum Expr {
    Const(LogicWord),
    Ref {
        /// Flat base id of the declaration.
        net: usize,
        /// Width of the declaration (for bounds).
        decl_width: usize,
        range: Range,
        signed: bool,
    },
    Un {
        op: UnOp,
        a: Box<Expr>,
    },
    Bin {
        op: BinOp,
        a: Box<Expr>,
        b: Box<Expr>,
    },
    Cond {
        c: Box<Expr>,
        t: Box<Expr>,
        f: Box<Expr>,
    },
    /// Verilog `{a, b, c}` — operands listed MSB-part-first.
    Concat(Vec<Expr>),
    Repeat {
        times: usize,
        e: Box<Expr>,
    },
    /// Signedness cast; width is unchanged.
    Signed {
        e: Box<Expr>,
        signed: bool,
    },
    /// Part-select of an arbitrary sub-expression: `[msb:lsb]`.
    Index {
        e: Box<Expr>,
        msb: usize,
        lsb: usize,
    },
}

impl Expr {
    /// Static bit width after Verilog sizing rules (operand-driven, minimum 1).
    pub fn width(&self) -> usize {
        match self {
            Expr::Const(w) => w.width(),
            Expr::Ref { range, .. } => range.width(),
            Expr::Un { op, a } => match op {
                UnOp::Not => a.width(),
                UnOp::AndR | UnOp::OrR | UnOp::XorR => 1,
            },
            Expr::Bin { op, a, b } => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    a.width().max(b.width())
                }
                BinOp::And | BinOp::Or | BinOp::Xor | BinOp::Shl | BinOp::Shr => a.width(),
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::LogAnd
                | BinOp::LogOr
                | BinOp::CaseEq
                | BinOp::CaseNe => 1,
            },
            Expr::Cond { t, f, .. } => t.width().max(f.width()),
            Expr::Concat(parts) => parts.iter().map(|p| p.width()).sum(),
            Expr::Repeat { times, e } => times * e.width(),
            Expr::Signed { e, .. } => e.width(),
            Expr::Index { msb, lsb, .. } => msb - lsb + 1,
        }
    }

    /// Verilog expression signedness after casts and operand propagation.
    pub fn is_signed(&self) -> bool {
        match self {
            Expr::Const(_) => false,
            Expr::Ref { signed, .. } => *signed,
            Expr::Un { op, a } => matches!(op, UnOp::Not) && a.is_signed(),
            Expr::Bin { op, a, b } => match op {
                BinOp::Shl | BinOp::Shr => a.is_signed(),
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Mod
                | BinOp::And
                | BinOp::Or
                | BinOp::Xor
                | BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge => a.is_signed() && b.is_signed(),
                BinOp::LogAnd | BinOp::LogOr | BinOp::CaseEq | BinOp::CaseNe => false,
            },
            Expr::Cond { t, f, .. } => t.is_signed() && f.is_signed(),
            Expr::Concat(_) => false,
            Expr::Repeat { e, .. } => e.is_signed(),
            Expr::Signed { signed, .. } => *signed,
            Expr::Index { .. } => false,
        }
    }
}

/// Assignment target.
#[derive(Clone, Debug)]
pub struct LValue {
    pub net: usize,
    pub decl_width: usize,
    pub range: Range,
}

#[derive(Clone, Debug)]
pub enum DispPart {
    Str(String),
    /// Index into `Stmt::Display::args`.
    Arg(usize, char), // char is the conversion specifier: b/d/h
    /// `$time` in a display format.
    Time,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Block(Vec<Stmt>),
    /// `forever stmt` (also how `always #d ...` clock generators are lowered).
    Forever(Box<Stmt>),
    /// Non-blocking `<=`.
    NBl {
        lv: LValue,
        e: Expr,
    },
    /// Blocking assignment to a runtime-indexed part-select.
    BlIndexed {
        net: usize,
        decl_width: usize,
        base_offset: usize,
        element_width: usize,
        start: Expr,
        width: usize,
        ascending: bool,
        reversed: bool,
        e: Expr,
    },
    /// Non-blocking assignment to a runtime-indexed part-select. The target
    /// start is sampled in the active region; only the selected field commits
    /// in the NBA region.
    NBlIndexed {
        net: usize,
        decl_width: usize,
        base_offset: usize,
        element_width: usize,
        start: Expr,
        width: usize,
        ascending: bool,
        reversed: bool,
        e: Expr,
    },
    /// Blocking `=`.
    Bl {
        lv: LValue,
        e: Expr,
    },
    If {
        c: Expr,
        t: Box<Stmt>,
        f: Option<Box<Stmt>>,
    },
    Case {
        e: Expr,
        arms: Vec<(Expr, Stmt)>,
        dflt: Option<Box<Stmt>>,
    },
    /// `#delay stmt`
    Delay {
        ns: u64,
        body: Box<Stmt>,
    },
    Display {
        parts: Vec<DispPart>,
        args: Vec<Expr>,
        /// `$monitor`: re-print whenever an argument changes.
        monitor: bool,
    },
    /// Immediate procedural assertion; a false/X/Z condition is an error.
    Assert {
        cond: Expr,
        message: Option<String>,
    },
    /// Immediate procedural assumption; violations reject the stimulus.
    Assume {
        cond: Expr,
        message: Option<String>,
    },
    /// Sampled cover-property bin hit.
    Cover {
        label: String,
        cond: Expr,
    },
    Finish,
}

#[derive(Clone, Debug)]
pub enum Sens {
    /// `@(*)` — combinational.
    Comb,
    Posedge(usize),
    Negedge(usize),
    List(Vec<Sens>),
}

#[derive(Clone, Debug)]
pub struct Process {
    pub sens: Sens,
    pub body: Stmt,
}

/// One elaborated module instance in the design hierarchy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HierarchicalInstance {
    /// Stable dotted instance path, such as `top.u1.u2`.
    pub path: String,
    /// Source module name before flattening.
    pub module: String,
    /// Parent instance path; `None` for the root.
    pub parent: Option<String>,
    pub depth: usize,
    /// Inclusive first flat net bit owned by this instance.
    pub net_start: usize,
    /// Exclusive final flat net bit owned by this instance.
    pub net_end: usize,
    pub ports: Vec<HierarchicalPort>,
}

/// A module port resolved to its flat net range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HierarchicalPort {
    pub name: String,
    pub direction: Dir,
    pub base: usize,
    pub width: usize,
    /// Parent flat net bit for each port bit when connectivity is direct.
    pub connections: Vec<Option<usize>>,
}

#[derive(Clone, Debug)]
pub struct Assign {
    pub lv: LValue,
    pub e: Expr,
    /// Net delay in ns (gate delay `assign #5 y = ...`).
    pub delay: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Design {
    pub name: String,
    pub net_decls: Vec<NetDecl>,
    pub n_nets: usize,
    pub ports: Vec<PortDecl>,
    pub assigns: Vec<Assign>,
    pub processes: Vec<Process>,
    pub initials: Vec<Stmt>,
    /// Elaborated module instances retained after flattening.
    pub hierarchy: Vec<HierarchicalInstance>,
}

impl Design {
    /// Display name of a flat net id, e.g. `a[3]` or `clk`.
    pub fn net_name(&self, flat: usize) -> String {
        for d in &self.net_decls {
            if flat >= d.base && flat < d.base + d.width {
                if d.width == 1 {
                    return d.name.clone();
                }
                return format!("{}[{}]", d.name, flat - d.base);
            }
        }
        format!("net{}", flat)
    }

    pub fn input_ports(&self) -> Vec<&PortDecl> {
        self.ports.iter().filter(|p| p.dir == Dir::In).collect()
    }

    pub fn output_ports(&self) -> Vec<&PortDecl> {
        self.ports.iter().filter(|p| p.dir == Dir::Out).collect()
    }

    pub fn port(&self, name: &str) -> Option<&PortDecl> {
        self.ports.iter().find(|p| p.name == name)
    }

    pub fn instance(&self, path: &str) -> Option<&HierarchicalInstance> {
        self.hierarchy.iter().find(|instance| instance.path == path)
    }

    pub fn instances_of_module(&self, module: &str) -> Vec<&HierarchicalInstance> {
        self.hierarchy
            .iter()
            .filter(|instance| instance.module == module)
            .collect()
    }

    /// Find the deepest instance that owns a flat net bit.
    pub fn owning_instance(&self, net: usize) -> Option<&HierarchicalInstance> {
        self.hierarchy
            .iter()
            .filter(|instance| net >= instance.net_start && net < instance.net_end)
            .min_by_key(|instance| instance.net_end - instance.net_start)
    }

    /// Human-readable elaborated hierarchy summary.
    pub fn hierarchy_report(&self) -> String {
        let mut out = format!("hierarchy: {} instance(s)\n", self.hierarchy.len());
        for instance in &self.hierarchy {
            out.push_str(&format!(
                "  {}{}: module '{}', nets [{}..{}), ports {}\n",
                "  ".repeat(instance.depth),
                instance.path,
                instance.module,
                instance.net_start,
                instance.net_end,
                instance.ports.len()
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expr_width_rules() {
        let c4 = Expr::Const(LogicWord::zero(4));
        assert_eq!(c4.width(), 4);
        let cat = Expr::Concat(vec![c4.clone(), c4.clone()]);
        assert_eq!(cat.width(), 8);
        let rep = Expr::Repeat {
            times: 3,
            e: Box::new(c4.clone()),
        };
        assert_eq!(rep.width(), 12);
        let sum = Expr::Bin {
            op: BinOp::Add,
            a: Box::new(c4.clone()),
            b: Box::new(Expr::Const(LogicWord::zero(8))),
        };
        assert_eq!(sum.width(), 8);
        let eq = Expr::Bin {
            op: BinOp::Eq,
            a: Box::new(c4.clone()),
            b: Box::new(c4),
        };
        assert_eq!(eq.width(), 1);
        let idx = Expr::Index {
            e: Box::new(rep),
            msb: 5,
            lsb: 2,
        };
        assert_eq!(idx.width(), 4);
    }

    #[test]
    fn net_name_lookup() {
        let mut d = Design {
            name: "m".into(),
            ..Default::default()
        };
        d.net_decls.push(NetDecl {
            name: "a".into(),
            width: 4,
            is_reg: false,
            is_signed: false,
            base: 0,
        });
        d.net_decls.push(NetDecl {
            name: "clk".into(),
            width: 1,
            is_reg: false,
            is_signed: false,
            base: 4,
        });
        d.n_nets = 5;
        assert_eq!(d.net_name(2), "a[2]");
        assert_eq!(d.net_name(4), "clk");
        assert_eq!(d.net_name(9), "net9");
    }
}
