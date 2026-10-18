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

/// One unpacked-array dimension retained by a first-class memory object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryDimension {
    /// First logical index exposed by this dimension.
    pub base: i64,
    /// +1 for ascending ranges and -1 for descending ranges.
    pub step: i64,
    pub length: usize,
}

/// Coarse storage contract inferred after elaboration.
///
/// This is not yet a vendor macro mapping, but it separates read-only storage
/// from runtime-writable storage before synthesis and physical mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryKind {
    #[default]
    Ram,
    Rom,
}

/// Technology-independent macro candidate inferred from memory port timing.
///
/// This does not select a vendor cell. A physical mapping layer can use these
/// candidates together with depth, width, library, timing, and routing
/// constraints to choose distributed logic, LUT RAM, block RAM, or a vendor
/// macro.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryTemplate {
    #[default]
    Unmapped,
    AsyncRom,
    SyncRom,
    MixedRom,
    AsyncRam,
    SyncRam,
    MixedRam,
}

/// Conservative runtime write-mask contract.
///
/// A `true` bit means some runtime write port may update that element bit.
/// Unknown dynamic starts conservatively set the complete mask. ROMs have no
/// runtime write mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryWriteMask {
    pub width: usize,
    /// LSB-first, matching `LogicWord` and flat net ordering.
    pub bits: Vec<bool>,
}

impl MemoryWriteMask {
    pub fn full(width: usize) -> Self {
        Self {
            width,
            bits: vec![true; width],
        }
    }

    pub fn is_full(&self) -> bool {
        self.width == self.bits.len() && self.bits.iter().all(|bit| *bit)
    }
}

/// Byte-lane write contract derived from a conservative bit-level mask.
///
/// This metadata is `None` when writes are narrower than a byte, cross byte
/// boundaries, or otherwise cannot be represented by aligned byte lanes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryByteEnable {
    pub lane_width: usize,
    pub lanes: Vec<bool>,
}

impl MemoryByteEnable {
    pub fn full(width: usize) -> Self {
        let lane_width = 8;
        Self {
            lane_width,
            lanes: vec![true; width / lane_width],
        }
    }

    pub fn is_full(&self) -> bool {
        !self.lanes.is_empty() && self.lanes.iter().all(|lane| *lane)
    }

    fn from_write_mask(mask: Option<&MemoryWriteMask>, width: usize) -> Option<Self> {
        let mask = mask?;
        if width == 0 || !width.is_multiple_of(8) || mask.width != width || mask.bits.len() != width
        {
            return None;
        }
        let mut lanes = Vec::with_capacity(width / 8);
        for lane in mask.bits.chunks_exact(8) {
            let enabled = lane[0];
            if lane.iter().any(|bit| *bit != enabled) {
                return None;
            }
            lanes.push(enabled);
        }
        Some(Self {
            lane_width: 8,
            lanes,
        })
    }
}

/// Logical banking derived from the outer unpacked-array dimension.
///
/// This is not a vendor macro selection. It records the natural contiguous
/// partitions already present in the source shape so synthesis can evaluate
/// conflict, mapping, and implementation choices without flattening that
/// structure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBanking {
    /// Number of logical banks; one for a one-dimensional memory.
    pub count: u64,
    /// Contiguous elements retained by each logical bank.
    pub depth: u64,
}

impl MemoryBanking {
    pub fn from_dimensions(dimensions: &[MemoryDimension], flat_length: u64) -> Self {
        let count = if dimensions.len() > 1 {
            dimensions
                .first()
                .map_or(1, |dimension| dimension.length as u64)
        } else {
            1
        }
        .max(1);
        let depth = if flat_length == 0 || count == 0 {
            0
        } else {
            flat_length / count
        };
        Self { count, depth }
    }

    pub fn bank_of_physical(&self, physical_index: u64) -> Option<u64> {
        if self.depth == 0 || physical_index >= self.count * self.depth {
            return None;
        }
        Some(physical_index / self.depth)
    }

    pub fn offset_in_bank(&self, physical_index: u64) -> Option<u64> {
        self.bank_of_physical(physical_index)
            .map(|_| physical_index % self.depth)
    }
}

/// A sparse unpacked-array object.
///
/// Memory elements do not need to occupy flat net IDs. This is the IR boundary
/// for RAM/ROM storage; read/write operations still carry source expressions
/// for their logical indexes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryDecl {
    pub name: String,
    pub element_width: usize,
    pub element_msb: i64,
    pub element_lsb: i64,
    pub dimensions: Vec<MemoryDimension>,
    /// Product of all dimension lengths.
    pub flat_length: u64,
    /// Inferred storage contract. RAM is the conservative default.
    pub kind: MemoryKind,
    /// Inferred technology-independent macro candidate.
    pub template: MemoryTemplate,
    /// Inferred runtime write-port mask. `None` for ROM.
    pub write_mask: Option<MemoryWriteMask>,
    /// Aligned byte-lane contract derived from `write_mask`.
    pub byte_enable: Option<MemoryByteEnable>,
    /// Logical banking metadata inferred from the source shape.
    pub banking: MemoryBanking,
}

/// Aliases a child instance's memory object to the parent memory supplied on
/// an `inout` unpacked-array port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryConnection {
    pub child: usize,
    pub parent: usize,
}

impl MemoryDecl {
    pub fn physical_index(&self, indexes: &[i64]) -> Option<u64> {
        if indexes.len() != self.dimensions.len() {
            return None;
        }
        let mut physical = 0_i128;
        let mut stride = i128::from(self.flat_length);
        for (index, dimension) in indexes.iter().zip(&self.dimensions) {
            let last = dimension.base + dimension.step * (dimension.length as i64 - 1);
            let lower = dimension.base.min(last);
            let upper = dimension.base.max(last);
            if *index < lower || *index > upper {
                return None;
            }
            let normalized = i128::from((*index - dimension.base) * dimension.step);
            stride /= dimension.length as i128;
            physical += normalized * stride;
        }
        u64::try_from(physical).ok()
    }

    /// Map a logical indexed part-select to physical element-bit offsets.
    ///
    /// The returned range is `(low_physical, high_physical)` and is inclusive.
    /// Ascending and descending packed element declarations both map their
    /// logical bit sequence onto contiguous physical storage.
    pub fn element_part_offsets(
        &self,
        start: i64,
        width: usize,
        ascending: bool,
    ) -> Option<(usize, usize)> {
        if width == 0 || width > self.element_width {
            return None;
        }
        let low_logical = if ascending {
            start
        } else {
            start - width as i64 + 1
        };
        let high_logical = if ascending {
            start + width as i64 - 1
        } else {
            start
        };
        let low_logical_offset = self.element_bit_offset(low_logical)?;
        let high_logical_offset = self.element_bit_offset(high_logical)?;
        let low = low_logical_offset.min(high_logical_offset);
        let high = low_logical_offset.max(high_logical_offset);
        if high < low || high - low + 1 != width {
            return None;
        }
        Some((low, high))
    }

    fn element_bit_offset(&self, logical: i64) -> Option<usize> {
        let lower = self.element_msb.min(self.element_lsb);
        let upper = self.element_msb.max(self.element_lsb);
        if logical < lower || logical > upper {
            return None;
        }
        let offset = if self.element_msb >= self.element_lsb {
            logical - self.element_lsb
        } else {
            self.element_lsb - logical
        };
        usize::try_from(offset).ok()
    }

    pub fn element_is_reversed(&self) -> bool {
        self.element_msb < self.element_lsb
    }
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
    Pow,
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
    /// Whole-element read from a first-class sparse memory.
    MemoryRead {
        memory: usize,
        indexes: Vec<Expr>,
        width: usize,
        signed: bool,
    },
    /// Indexed part-select read from a first-class sparse-memory element.
    MemoryReadPart {
        memory: usize,
        indexes: Vec<Expr>,
        start: Box<Expr>,
        width: usize,
        ascending: bool,
    },
    /// Part-select of an arbitrary sub-expression: `[msb:lsb]`.
    Index {
        e: Box<Expr>,
        msb: usize,
        lsb: usize,
    },
    /// Number of bytes before the first NUL in a fixed-width packed string.
    StringLength {
        e: Box<Expr>,
    },
    /// Bounded string concatenation in little-endian byte layout.
    StringConcat {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Reads one byte from a fixed-width packed string.
    StringGetByte {
        e: Box<Expr>,
        index: Box<Expr>,
    },
    /// Converts bounded ASCII string bytes to upper or lower case.
    StringCase {
        e: Box<Expr>,
        upper: bool,
    },
    /// Lexicographically compares two bounded ASCII strings.
    StringCompare {
        left: Box<Expr>,
        right: Box<Expr>,
        ignore_case: bool,
    },
    /// True when a named event was triggered in the current simulation time
    /// slot. `net` refers to the event's internal scalar state.
    EventTriggered {
        net: usize,
    },
    /// Updates one byte in a bounded string. This is used only as the RHS of
    /// a whole-string assignment when lowering the void `putc()` method.
    StringPutc {
        e: Box<Expr>,
        index: Box<Expr>,
        value: Box<Expr>,
    },
    /// Returns the inclusive bounded-string substring `[left, right]`, or an
    /// empty string for invalid bounds.
    StringSubstr {
        e: Box<Expr>,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Parses the valid signed prefix of a bounded string in the given radix.
    StringToRadix {
        e: Box<Expr>,
        radix: u32,
    },
    /// Formats an integer as a bounded ASCII string in decimal, hex, or
    /// binary. Used as the RHS of a whole-string assignment for void methods.
    StringFromRadix {
        value: Box<Expr>,
        radix: u32,
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
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
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
            Expr::MemoryRead { width, .. } => *width,
            Expr::MemoryReadPart { width, .. } => *width,
            Expr::Index { msb, lsb, .. } => msb - lsb + 1,
            Expr::StringLength { .. } => 32,
            Expr::StringConcat { left, .. } => left.width(),
            Expr::StringGetByte { .. } => 8,
            Expr::StringCase { e, .. } => e.width(),
            Expr::StringCompare { .. } => 32,
            Expr::EventTriggered { .. } => 1,
            Expr::StringPutc { e, .. } => e.width(),
            Expr::StringSubstr { e, .. } => e.width(),
            Expr::StringToRadix { .. } => 32,
            Expr::StringFromRadix { .. } => 512,
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
                | BinOp::Pow
                | BinOp::And
                | BinOp::Or
                | BinOp::Xor => a.is_signed() && b.is_signed(),
                // Comparison and equality expressions are one-bit unsigned results
                // even when their operands are signed.
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => false,
                BinOp::LogAnd | BinOp::LogOr | BinOp::CaseEq | BinOp::CaseNe => false,
            },
            Expr::Cond { t, f, .. } => t.is_signed() && f.is_signed(),
            Expr::Concat(_) => false,
            Expr::Repeat { e, .. } => e.is_signed(),
            Expr::Signed { signed, .. } => *signed,
            Expr::MemoryRead { signed, .. } => *signed,
            // A part-select is unsigned even when its base declaration is signed.
            Expr::MemoryReadPart { .. } => false,
            Expr::Index { .. } => false,
            Expr::StringLength { .. } => true,
            Expr::StringConcat { .. } => false,
            Expr::StringGetByte { .. } => false,
            Expr::StringCase { .. } => false,
            Expr::StringCompare { .. } => true,
            Expr::EventTriggered { .. } => false,
            Expr::StringPutc { .. } => false,
            Expr::StringSubstr { .. } => false,
            Expr::StringToRadix { .. } => true,
            Expr::StringFromRadix { .. } => false,
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
    /// Blocking whole-element write to a first-class sparse memory.
    MemoryBl {
        memory: usize,
        indexes: Vec<Expr>,
        e: Expr,
    },
    /// Non-blocking whole-element write to a first-class sparse memory. Index
    /// and data expressions are sampled in the active region.
    MemoryNBl {
        memory: usize,
        indexes: Vec<Expr>,
        e: Expr,
    },
    /// Blocking indexed part-select write to a sparse-memory element.
    MemoryBlPart {
        memory: usize,
        indexes: Vec<Expr>,
        start: Box<Expr>,
        width: usize,
        ascending: bool,
        e: Expr,
    },
    /// Non-blocking indexed part-select write. Index/start/data are sampled in
    /// the active region, and only the selected bits commit in the NBA region.
    MemoryNBlPart {
        memory: usize,
        indexes: Vec<Expr>,
        start: Box<Expr>,
        width: usize,
        ascending: bool,
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
    /// Suspend until `cond` becomes true, then execute `body`.
    Wait {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// Suspend until any value change on one explicitly listed scalar event
    /// signal, then execute `body`.
    EventWait {
        watches: Vec<EventWatch>,
        body: Box<Stmt>,
    },
    /// Suspend until scalar named events trigger in `events` order. An event
    /// appearing out of order selects the failure branch.
    WaitOrder {
        events: Vec<usize>,
        success: Option<Box<Stmt>>,
        failure: Option<Box<Stmt>>,
    },
    /// Concurrent branches joined with `join`, `join_any`, or `join_none`.
    Fork {
        branches: Vec<Stmt>,
        mode: ForkMode,
    },
    /// Execute `body` repeatedly while `cond` is true.
    While {
        cond: Expr,
        body: Box<Stmt>,
        steps: Vec<Stmt>,
    },
    /// Execute `body` the number of times specified by a count sampled once at
    /// loop entry.
    Repeat {
        count: Expr,
        body: Box<Stmt>,
    },
    /// Force `lv` to `e` until an explicit release.
    Force {
        lv: LValue,
        e: Expr,
    },
    /// Release a previously forced lvalue.
    Release {
        lv: LValue,
    },
    /// Exit the nearest runtime loop.
    Break,
    /// Start the next iteration of the nearest runtime loop.
    Continue,
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
    /// Runtime `$writememh`/`$writememb` dump. The frontend resolves the path;
    /// start/finish are sampled when the task executes.
    WriteMem {
        radix: u32,
        path: String,
        memory: usize,
        start: Option<Expr>,
        finish: Option<Expr>,
    },
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkMode {
    JoinAll,
    JoinAny,
    JoinNone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventEdge {
    Any,
    Posedge,
    Negedge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventWatch {
    pub net: usize,
    pub edge: EventEdge,
}

#[derive(Clone, Debug)]
pub enum Sens {
    /// `@(*)` — combinational.
    Comb,
    /// A procedure driven by its own procedural delay controls, such as
    /// `always #5 ...`, rather than by signal sensitivity.
    Timed,
    /// Any value change of one explicitly listed scalar signal.
    Any(usize),
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
    pub memories: Vec<MemoryDecl>,
    pub memory_connections: Vec<MemoryConnection>,
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

    /// Infer whether each sparse-memory object needs runtime write ports.
    ///
    /// Initializers and `$readmem` loads are treated as ROM initialization.
    /// Any procedural write in a non-initial process makes the canonical
    /// memory (including hierarchy aliases) a RAM.
    pub fn infer_memory_kinds(&mut self) {
        let mut accesses = MemoryAccesses::default();
        let memory_widths = self
            .memories
            .iter()
            .map(|memory| memory.element_width)
            .collect::<Vec<_>>();
        for process in &self.processes {
            let synchronous = process_is_edge_clocked(&process.sens);
            collect_memory_accesses(&process.body, synchronous, &memory_widths, &mut accesses);
        }
        for assignment in &self.assigns {
            collect_memory_expr_reads(&assignment.e, false, &mut accesses);
        }
        let aliases = self.memory_alias_roots();
        let accesses = accesses.canonicalize(&aliases);
        for (index, memory) in self.memories.iter_mut().enumerate() {
            let root = aliases.get(index).copied().unwrap_or(index);
            memory.kind =
                if accesses.sync_writes.contains(&root) || accesses.async_writes.contains(&root) {
                    MemoryKind::Ram
                } else {
                    MemoryKind::Rom
                };
            memory.template = accesses.template(root);
            let write_mask = accesses.write_mask(root, memory.element_width);
            let byte_enable =
                MemoryByteEnable::from_write_mask(write_mask.as_ref(), memory.element_width);
            let banking = MemoryBanking::from_dimensions(&memory.dimensions, memory.flat_length);
            memory.write_mask = write_mask;
            memory.byte_enable = byte_enable;
            memory.banking = banking;
        }
    }

    fn memory_alias_roots(&self) -> Vec<usize> {
        let mut aliases = (0..self.memories.len()).collect::<Vec<_>>();
        for _ in 0..self.memories.len() {
            for connection in &self.memory_connections {
                if let Some(parent) = aliases.get(connection.parent).copied() {
                    if let Some(alias) = aliases.get_mut(connection.child) {
                        *alias = parent;
                    }
                }
            }
        }
        aliases
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

#[derive(Default)]
struct MemoryAccesses {
    sync_reads: std::collections::HashSet<usize>,
    async_reads: std::collections::HashSet<usize>,
    sync_writes: std::collections::HashSet<usize>,
    async_writes: std::collections::HashSet<usize>,
    write_observations: std::collections::HashMap<usize, Vec<MemoryWriteObservation>>,
}

#[derive(Clone, Copy)]
struct MemoryWriteObservation {
    start: Option<i64>,
    width: usize,
    ascending: bool,
}

impl MemoryAccesses {
    fn canonicalize(self, aliases: &[usize]) -> Self {
        fn canonicalize_set(
            values: std::collections::HashSet<usize>,
            aliases: &[usize],
        ) -> std::collections::HashSet<usize> {
            values
                .into_iter()
                .map(|memory| aliases.get(memory).copied().unwrap_or(memory))
                .collect()
        }

        Self {
            sync_reads: canonicalize_set(self.sync_reads, aliases),
            async_reads: canonicalize_set(self.async_reads, aliases),
            sync_writes: canonicalize_set(self.sync_writes, aliases),
            async_writes: canonicalize_set(self.async_writes, aliases),
            write_observations: self
                .write_observations
                .into_iter()
                .map(|(memory, observations)| {
                    (aliases.get(memory).copied().unwrap_or(memory), observations)
                })
                .collect(),
        }
    }

    fn template(&self, memory: usize) -> MemoryTemplate {
        let sync_write = self.sync_writes.contains(&memory);
        let async_write = self.async_writes.contains(&memory);
        if sync_write || async_write {
            return match (sync_write, async_write) {
                (true, true) => MemoryTemplate::MixedRam,
                (true, false) => MemoryTemplate::SyncRam,
                (false, _) => MemoryTemplate::AsyncRam,
            };
        }

        let sync_read = self.sync_reads.contains(&memory);
        let async_read = self.async_reads.contains(&memory);
        match (sync_read, async_read) {
            (true, true) => MemoryTemplate::MixedRom,
            (true, false) => MemoryTemplate::SyncRom,
            (false, true) => MemoryTemplate::AsyncRom,
            (false, false) => MemoryTemplate::Unmapped,
        }
    }

    fn write_mask(&self, memory: usize, width: usize) -> Option<MemoryWriteMask> {
        let observations = self.write_observations.get(&memory)?;
        let mut mask = vec![false; width];
        for observation in observations {
            let Some(start) = observation.start else {
                return Some(MemoryWriteMask::full(width));
            };
            let (low, high) = if observation.ascending {
                (start, start + observation.width as i64 - 1)
            } else {
                (start - observation.width as i64 + 1, start)
            };
            if low < 0 || high < low || high as usize >= width {
                return Some(MemoryWriteMask::full(width));
            }
            mask[low as usize..=high as usize].fill(true);
        }
        Some(MemoryWriteMask { width, bits: mask })
    }
}

fn process_is_edge_clocked(sensitivity: &Sens) -> bool {
    match sensitivity {
        Sens::Comb | Sens::Timed => false,
        Sens::Any(_) => false,
        Sens::Posedge(_) | Sens::Negedge(_) => true,
        Sens::List(list) => list
            .iter()
            .any(|item| matches!(item, Sens::Posedge(_) | Sens::Negedge(_))),
    }
}

fn constant_expression_i64(expression: &Expr) -> Option<i64> {
    match expression {
        Expr::Const(word) => word.to_u64().map(|value| value as i64),
        Expr::Signed { e, .. } => constant_expression_i64(e),
        _ => None,
    }
}

fn collect_memory_accesses(
    statement: &Stmt,
    synchronous: bool,
    memory_widths: &[usize],
    accesses: &mut MemoryAccesses,
) {
    match statement {
        Stmt::Block(statements) => statements.iter().for_each(|statement| {
            collect_memory_accesses(statement, synchronous, memory_widths, accesses)
        }),
        Stmt::Forever(body) | Stmt::Delay { body, .. } => {
            collect_memory_accesses(body, synchronous, memory_widths, accesses)
        }
        Stmt::Wait { cond, body } => {
            collect_memory_expr_reads(cond, synchronous, accesses);
            collect_memory_accesses(body, synchronous, memory_widths, accesses)
        }
        Stmt::EventWait { body, .. } => {
            collect_memory_accesses(body, synchronous, memory_widths, accesses)
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(success) = success {
                collect_memory_accesses(success, synchronous, memory_widths, accesses);
            }
            if let Some(failure) = failure {
                collect_memory_accesses(failure, synchronous, memory_widths, accesses);
            }
        }
        Stmt::Fork { branches, .. } => branches.iter().for_each(|branch| {
            collect_memory_accesses(branch, synchronous, memory_widths, accesses)
        }),
        Stmt::While { cond, body, steps } => {
            collect_memory_expr_reads(cond, synchronous, accesses);
            collect_memory_accesses(body, synchronous, memory_widths, accesses);
            for step in steps {
                collect_memory_accesses(step, synchronous, memory_widths, accesses);
            }
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Repeat { count, body } => {
            collect_memory_expr_reads(count, synchronous, accesses);
            collect_memory_accesses(body, synchronous, memory_widths, accesses)
        }
        Stmt::Force { e, .. } => collect_memory_expr_reads(e, synchronous, accesses),
        Stmt::Release { .. } => {}
        Stmt::MemoryBl { memory, indexes, e } | Stmt::MemoryNBl { memory, indexes, e } => {
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, accesses);
            }
            collect_memory_expr_reads(e, synchronous, accesses);
            if synchronous {
                accesses.sync_writes.insert(*memory);
            } else {
                accesses.async_writes.insert(*memory);
            }
            accesses
                .write_observations
                .entry(*memory)
                .or_default()
                .push(MemoryWriteObservation {
                    start: Some(0),
                    width: memory_widths.get(*memory).copied().unwrap_or(0),
                    ascending: true,
                });
        }
        Stmt::MemoryBlPart {
            memory,
            indexes,
            start,
            width,
            ascending,
            e,
        }
        | Stmt::MemoryNBlPart {
            memory,
            indexes,
            start,
            width,
            ascending,
            e,
        } => {
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, accesses);
            }
            collect_memory_expr_reads(start, synchronous, accesses);
            collect_memory_expr_reads(e, synchronous, accesses);
            if synchronous {
                accesses.sync_writes.insert(*memory);
            } else {
                accesses.async_writes.insert(*memory);
            }
            accesses
                .write_observations
                .entry(*memory)
                .or_default()
                .push(MemoryWriteObservation {
                    start: constant_expression_i64(start),
                    width: *width,
                    ascending: *ascending,
                });
        }
        Stmt::If { t, f, .. } => {
            collect_memory_accesses(t, synchronous, memory_widths, accesses);
            if let Some(f) = f {
                collect_memory_accesses(f, synchronous, memory_widths, accesses);
            }
        }
        Stmt::Case { arms, dflt, .. } => {
            for (_, body) in arms {
                collect_memory_accesses(body, synchronous, memory_widths, accesses);
            }
            if let Some(body) = dflt {
                collect_memory_accesses(body, synchronous, memory_widths, accesses);
            }
        }
        Stmt::NBl { e, .. } | Stmt::Bl { e, .. } => {
            collect_memory_expr_reads(e, synchronous, accesses);
        }
        Stmt::BlIndexed { start, e, .. } | Stmt::NBlIndexed { start, e, .. } => {
            collect_memory_expr_reads(start, synchronous, accesses);
            collect_memory_expr_reads(e, synchronous, accesses);
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            collect_memory_expr_reads(cond, synchronous, accesses)
        }
        Stmt::Display { args, .. } => {
            for argument in args {
                collect_memory_expr_reads(argument, synchronous, accesses);
            }
        }
        Stmt::WriteMem { .. } | Stmt::Finish => {}
    }
}

fn collect_memory_expr_reads(expression: &Expr, synchronous: bool, accesses: &mut MemoryAccesses) {
    match expression {
        Expr::Const(_) | Expr::Ref { .. } => {}
        Expr::MemoryRead {
            memory, indexes, ..
        } => {
            if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, accesses);
            }
        }
        Expr::MemoryReadPart {
            memory,
            indexes,
            start,
            ..
        } => {
            if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, accesses);
            }
            collect_memory_expr_reads(start, synchronous, accesses);
        }
        Expr::Un { a, .. } => collect_memory_expr_reads(a, synchronous, accesses),
        Expr::Bin { a, b, .. } => {
            collect_memory_expr_reads(a, synchronous, accesses);
            collect_memory_expr_reads(b, synchronous, accesses);
        }
        Expr::Cond { c, t, f } => {
            collect_memory_expr_reads(c, synchronous, accesses);
            collect_memory_expr_reads(t, synchronous, accesses);
            collect_memory_expr_reads(f, synchronous, accesses);
        }
        Expr::Concat(parts) => parts
            .iter()
            .for_each(|part| collect_memory_expr_reads(part, synchronous, accesses)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } | Expr::Index { e, .. } => {
            collect_memory_expr_reads(e, synchronous, accesses)
        }
        Expr::StringLength { e } => collect_memory_expr_reads(e, synchronous, accesses),
        Expr::StringConcat { left, right } => {
            collect_memory_expr_reads(left, synchronous, accesses);
            collect_memory_expr_reads(right, synchronous, accesses);
        }
        Expr::StringGetByte { e, index } => {
            collect_memory_expr_reads(e, synchronous, accesses);
            collect_memory_expr_reads(index, synchronous, accesses);
        }
        Expr::StringCase { e, .. } => collect_memory_expr_reads(e, synchronous, accesses),
        Expr::StringCompare { left, right, .. } => {
            collect_memory_expr_reads(left, synchronous, accesses);
            collect_memory_expr_reads(right, synchronous, accesses);
        }
        Expr::EventTriggered { .. } => {}
        Expr::StringPutc { e, index, value } => {
            collect_memory_expr_reads(e, synchronous, accesses);
            collect_memory_expr_reads(index, synchronous, accesses);
            collect_memory_expr_reads(value, synchronous, accesses);
        }
        Expr::StringSubstr { e, left, right } => {
            collect_memory_expr_reads(e, synchronous, accesses);
            collect_memory_expr_reads(left, synchronous, accesses);
            collect_memory_expr_reads(right, synchronous, accesses);
        }
        Expr::StringToRadix { e, .. } => {
            collect_memory_expr_reads(e, synchronous, accesses);
        }
        Expr::StringFromRadix { value, .. } => {
            collect_memory_expr_reads(value, synchronous, accesses);
        }
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

    #[test]
    fn memory_physical_indices_preserve_dimensions_and_bounds() {
        let memory = MemoryDecl {
            name: "mem".into(),
            element_width: 8,
            element_msb: 7,
            element_lsb: 0,
            dimensions: vec![
                MemoryDimension {
                    base: -2,
                    step: 1,
                    length: 4,
                },
                MemoryDimension {
                    base: 3,
                    step: -1,
                    length: 2,
                },
            ],
            flat_length: 8,
            kind: MemoryKind::Ram,
            template: MemoryTemplate::Unmapped,
            write_mask: None,
            byte_enable: None,
            banking: MemoryBanking::default(),
        };
        assert_eq!(memory.physical_index(&[-2, 3]), Some(0));
        assert_eq!(memory.physical_index(&[-2, 2]), Some(1));
        assert_eq!(memory.physical_index(&[-1, 3]), Some(2));
        assert_eq!(memory.physical_index(&[1, 2]), Some(7));
        assert_eq!(memory.physical_index(&[-3, 3]), None);
        assert_eq!(memory.physical_index(&[0, 1]), None);
        assert_eq!(memory.physical_index(&[0]), None);
        let banking = MemoryBanking::from_dimensions(&memory.dimensions, memory.flat_length);
        assert_eq!(banking.count, 4);
        assert_eq!(banking.depth, 2);
        assert_eq!(banking.bank_of_physical(0), Some(0));
        assert_eq!(banking.bank_of_physical(3), Some(1));
        assert_eq!(banking.offset_in_bank(3), Some(1));
        assert_eq!(banking.bank_of_physical(8), None);
    }

    #[test]
    fn reversed_memory_element_maps_indexed_part_selects() {
        let memory = MemoryDecl {
            name: "mem".into(),
            element_width: 8,
            element_msb: 0,
            element_lsb: 7,
            dimensions: vec![MemoryDimension {
                base: 0,
                step: 1,
                length: 513,
            }],
            flat_length: 513,
            kind: MemoryKind::Ram,
            template: MemoryTemplate::Unmapped,
            write_mask: None,
            byte_enable: None,
            banking: MemoryBanking::default(),
        };
        assert_eq!(memory.element_part_offsets(0, 4, true), Some((4, 7)));
        assert_eq!(memory.element_part_offsets(4, 4, true), Some((0, 3)));
        assert_eq!(memory.element_part_offsets(4, 4, false), Some((3, 6)));
        assert_eq!(memory.element_part_offsets(8, 1, true), None);
        assert_eq!(memory.element_part_offsets(0, 9, true), None);
        let banking = MemoryBanking::from_dimensions(&memory.dimensions, memory.flat_length);
        assert_eq!(banking.count, 1);
        assert_eq!(banking.depth, 513);
        assert_eq!(banking.bank_of_physical(512), Some(0));
        assert_eq!(banking.offset_in_bank(512), Some(512));
    }
}
