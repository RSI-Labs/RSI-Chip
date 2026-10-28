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

pub fn plusarg_net_name(name: &str) -> String {
    let encoded = name
        .bytes()
        .map(|byte| format!("_{byte:02x}"))
        .collect::<String>();
    format!("__eda_plusarg{encoded}")
}

pub fn plusarg_value_net_name(name: &str) -> String {
    let encoded = name
        .bytes()
        .map(|byte| format!("_{byte:02x}"))
        .collect::<String>();
    format!("__eda_plusarg_value{encoded}")
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

/// Execution and mapping contract for an unpacked-array object.
///
/// Ordinary arrays carry aggregate/value semantics and do not participate in
/// sparse RAM/ROM macro planning. Sparse objects use first-class memory
/// storage and memory-port inference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryStorage {
    #[default]
    Ordinary,
    Sparse,
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
    /// Dynamic arrays use runtime allocation semantics over a fixed maximum
    /// elaboration capacity; fixed arrays and sparse memories do not.
    pub dynamic: bool,
    /// Dynamic FIFO storage with logical indexes remapped after each pop.
    pub queue: bool,
    /// Declared inclusive upper index for a bounded queue, or `None` for an
    /// unbounded queue. The runtime capacity is `queue_bound + 1`.
    pub queue_bound: Option<usize>,
    /// Sparse key/value storage; logical keys are not physical array indexes.
    pub associative: bool,
    pub index_width: usize,
    pub index_signed: bool,
    pub storage: MemoryStorage,
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

/// Aliases a child instance's ordinary unpacked-array object to the parent
/// aggregate supplied on an array port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayConnection {
    pub child: usize,
    pub parent: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryPortDirection {
    In,
    Out,
    InOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryPortDecl {
    pub name: String,
    pub direction: MemoryPortDirection,
    pub memory: usize,
}

/// A top-level ordinary unpacked-array port.
///
/// Ordinary arrays use aggregate/value semantics and must not acquire sparse
/// RAM/ROM port contracts. The direction representation is shared with memory
/// ports, but the port object and storage contract are deliberately separate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayPortDecl {
    pub name: String,
    pub direction: MemoryPortDirection,
    pub array: usize,
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
    /// Current runtime length of a dynamic array.
    MemorySize {
        memory: usize,
        width: usize,
        signed: bool,
    },
    /// Read an associative-array element; missing or unknown keys yield X.
    AssocRead {
        memory: usize,
        index: Box<Expr>,
        width: usize,
    },
    /// Test whether an associative-array key exists.
    AssocExists {
        memory: usize,
        index: Box<Expr>,
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
    /// Current simulation time as an integral value. `$time` uses the signed
    /// 64-bit `time` type; `$stime` uses its unsigned 32-bit form.
    SimulationTime {
        width: usize,
        signed: bool,
        unit_exponent: i8,
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
            Expr::MemorySize { width, .. } => *width,
            Expr::AssocRead { width, .. } => *width,
            Expr::AssocExists { .. } => 1,
            Expr::Index { msb, lsb, .. } => msb - lsb + 1,
            Expr::StringLength { .. } => 32,
            Expr::StringConcat { left, .. } => left.width(),
            Expr::StringGetByte { .. } => 8,
            Expr::StringCase { e, .. } => e.width(),
            Expr::StringCompare { .. } => 32,
            Expr::EventTriggered { .. } => 1,
            Expr::SimulationTime { width, .. } => *width,
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
            Expr::MemorySize { signed, .. } => *signed,
            Expr::AssocRead { .. } => false,
            Expr::AssocExists { .. } => false,
            Expr::Index { .. } => false,
            Expr::StringLength { .. } => true,
            Expr::StringConcat { .. } => false,
            Expr::StringGetByte { .. } => false,
            Expr::StringCase { .. } => false,
            Expr::StringCompare { .. } => true,
            Expr::EventTriggered { .. } => false,
            Expr::SimulationTime { signed, .. } => *signed,
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
    /// A time-typed display argument, indexed into `Stmt::Display::args`.
    Time(usize),
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
    /// Whole-object copy selected by `kind`. Source values are sampled before
    /// blocking destination updates; nonblocking copies commit in the NBA region.
    MemoryCopy {
        source: usize,
        destination: usize,
        nonblocking: bool,
        kind: MemoryCopyKind,
    },
    /// Allocate a dynamic array. Blocking allocation replaces all elements with
    /// X; nonblocking allocation samples size and commits in the NBA region.
    MemoryNew {
        memory: usize,
        size: Expr,
        nonblocking: bool,
    },
    /// Delete a dynamic array and set its runtime size to zero.
    MemoryDelete {
        memory: usize,
    },
    /// Push a sampled value onto either end of a queue. A full queue errors.
    QueuePush {
        memory: usize,
        value: Expr,
        front: bool,
    },
    /// Pop from either end of a queue into a scalar/vector destination. An
    /// empty queue writes X and leaves the queue empty.
    QueuePop {
        memory: usize,
        destination: LValue,
        front: bool,
    },
    /// Insert a sampled value before a queue index. Unknown, negative, and
    /// greater-than-size indexes leave the queue unchanged.
    QueueInsert {
        memory: usize,
        index: Expr,
        value: Expr,
    },
    /// Delete one queue index. Unknown and out-of-range indexes leave the
    /// queue unchanged.
    QueueDelete {
        memory: usize,
        index: Expr,
    },
    /// Write one associative-array key. Unknown keys and capacity overflow
    /// fail closed.
    AssocWrite {
        memory: usize,
        index: Expr,
        value: Expr,
        nonblocking: bool,
    },
    /// Delete one key, or clear the associative array when index is `None`.
    AssocDelete {
        memory: usize,
        index: Option<Expr>,
    },
    /// Execute an associative-array traversal method, assign the found key to
    /// the caller's lvalue, and assign the one-bit success result.
    AssocTraversal {
        memory: usize,
        operation: AssocTraversalOp,
        key: LValue,
        destination: LValue,
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
        /// Historical field name; the value is canonical simulation ticks.
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
    /// An inlined subroutine invocation with its own return boundary.
    Scope(Box<Stmt>),
    /// Marks the outer edge of a subroutine return boundary.
    ReturnBarrier,
    /// Return from the current subroutine or, outside a subroutine scope,
    /// from the current procedural activation.
    Return,
    Display {
        parts: Vec<DispPart>,
        args: Vec<Expr>,
        /// `$monitor`: re-print whenever an argument changes.
        monitor: bool,
    },
    /// Configure `%t` formatting for subsequent display tasks.
    TimeFormat {
        /// Decimal exponent from seconds; for example `-9` means nanoseconds.
        units: i8,
        precision: u8,
        suffix: String,
        min_width: usize,
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

/// Distinguishes whole-object assignment semantics for storage families that
/// share the first-class memory representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryCopyKind {
    FixedArray,
    DynamicArray,
    Queue,
    Associative,
}

/// Associative-array traversal direction for built-in `first`, `last`,
/// `next`, and `prev` methods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssocTraversalOp {
    First,
    Last,
    Next,
    Prev,
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
    /// Net delay in canonical simulation ticks (gate delay `assign #5 y = ...`).
    pub delay: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LintIssueKind {
    UnusedName,
    LatchRisk,
    IncompleteCase,
    CombinationalLoop,
    MultipleContinuousDrivers,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LintIssue {
    pub kind: LintIssueKind,
    pub net: String,
    pub count: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Design {
    pub name: String,
    /// Canonical simulator tick precision as a decimal second exponent.
    pub time_scale: TimeScale,
    /// First-class top-level sparse-memory port contracts.
    pub memory_ports: Vec<MemoryPortDecl>,
    /// First-class top-level ordinary-array port contracts.
    pub array_ports: Vec<ArrayPortDecl>,
    pub net_decls: Vec<NetDecl>,
    pub memories: Vec<MemoryDecl>,
    pub memory_connections: Vec<MemoryConnection>,
    /// First-class aliases for child ordinary-array ports.
    pub array_connections: Vec<ArrayConnection>,
    pub n_nets: usize,
    pub ports: Vec<PortDecl>,
    pub assigns: Vec<Assign>,
    pub processes: Vec<Process>,
    pub initials: Vec<Stmt>,
    /// SystemVerilog `final` procedures executed at simulation shutdown.
    pub final_blocks: Vec<Stmt>,
    /// Clocking-block input snapshots used by the event scheduler.
    pub clocking_blocks: Vec<ClockingBlock>,
    /// Elaborated module instances retained after flattening.
    pub hierarchy: Vec<HierarchicalInstance>,
    /// One-bit state initialized from simulator plusargs.
    pub plusargs: std::collections::BTreeMap<String, usize>,
    /// Bounded string state containing the value part of a plusarg.
    pub plusarg_values: std::collections::BTreeMap<String, usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeScale {
    /// Decimal second exponent of one simulation tick; -9 means nanoseconds.
    pub precision_exponent: i8,
}

impl Default for TimeScale {
    fn default() -> Self {
        Self {
            precision_exponent: -9,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DfsState {
    White,
    Gray,
    Black,
}

fn lint_lvalue_bits(lvalue: &LValue) -> Vec<usize> {
    match lvalue.range {
        Range::Bit(bit) => vec![lvalue.net + bit],
        Range::Span { msb, lsb } => (lsb..=msb).map(|bit| lvalue.net + bit).collect(),
    }
}

fn lint_expression_nets(expression: &Expr, output: &mut Vec<usize>) {
    match expression {
        Expr::Const(_) | Expr::SimulationTime { .. } | Expr::MemorySize { .. } => {}
        Expr::Ref { net, range, .. } => match range {
            Range::Bit(bit) => output.push(net + bit),
            Range::Span { msb, lsb } => output.extend((*lsb..=*msb).map(|bit| net + bit)),
        },
        Expr::Un { a, .. }
        | Expr::Signed { e: a, .. }
        | Expr::Index { e: a, .. }
        | Expr::StringLength { e: a }
        | Expr::StringCase { e: a, .. }
        | Expr::StringToRadix { e: a, .. } => lint_expression_nets(a, output),
        Expr::Bin { a, b, .. }
        | Expr::StringConcat {
            left: a, right: b, ..
        }
        | Expr::StringCompare {
            left: a, right: b, ..
        } => {
            lint_expression_nets(a, output);
            lint_expression_nets(b, output);
        }
        Expr::Cond { c, t, f } => {
            lint_expression_nets(c, output);
            lint_expression_nets(t, output);
            lint_expression_nets(f, output);
        }
        Expr::Concat(parts) => parts
            .iter()
            .for_each(|part| lint_expression_nets(part, output)),
        Expr::Repeat { e, .. } => lint_expression_nets(e, output),
        Expr::MemoryRead { indexes, .. } => indexes
            .iter()
            .for_each(|index| lint_expression_nets(index, output)),
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes
                .iter()
                .for_each(|index| lint_expression_nets(index, output));
            lint_expression_nets(start, output);
        }
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => {
            lint_expression_nets(index, output)
        }
        Expr::StringGetByte { e, index } => {
            lint_expression_nets(e, output);
            lint_expression_nets(index, output);
        }
        Expr::StringPutc { e, index, value } => {
            lint_expression_nets(e, output);
            lint_expression_nets(index, output);
            lint_expression_nets(value, output);
        }
        Expr::StringSubstr { e, left, right } => {
            lint_expression_nets(e, output);
            lint_expression_nets(left, output);
            lint_expression_nets(right, output);
        }
        Expr::StringFromRadix { value, .. } => lint_expression_nets(value, output),
        Expr::EventTriggered { net } => output.push(*net),
    }
}

type LintDependencyValues = std::collections::BTreeMap<usize, std::collections::BTreeSet<usize>>;

fn lint_expression_dependencies(
    expression: &Expr,
    values: &LintDependencyValues,
) -> std::collections::BTreeSet<usize> {
    let mut references = Vec::new();
    lint_expression_nets(expression, &mut references);
    references
        .into_iter()
        .flat_map(|reference| {
            values
                .get(&reference)
                .cloned()
                .unwrap_or_else(|| std::collections::BTreeSet::from([reference]))
        })
        .collect()
}

fn lint_merge_dependency_values(left: &mut LintDependencyValues, right: LintDependencyValues) {
    for (bit, right_dependencies) in right {
        left.entry(bit).or_default().extend(right_dependencies);
    }
}

fn lint_execute_dependency_statement(statement: &Stmt, values: &mut LintDependencyValues) {
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|nested| lint_execute_dependency_statement(nested, values)),
        Stmt::Scope(body) => lint_execute_dependency_statement(body, values),
        Stmt::Bl { lv, e } | Stmt::NBl { lv, e } => {
            let dependencies = lint_expression_dependencies(e, values);
            for bit in lint_lvalue_bits(lv) {
                values.insert(bit, dependencies.clone());
            }
        }
        Stmt::If { c, t, f } => {
            let condition = lint_expression_dependencies(c, values);
            let entry = values.clone();

            let mut true_values = entry.clone();
            lint_execute_dependency_statement(t, &mut true_values);
            for bit in lint_guaranteed_assignment_bits(t) {
                true_values
                    .entry(bit)
                    .or_default()
                    .extend(condition.iter().copied());
            }

            let mut false_values = entry;
            if let Some(f) = f {
                lint_execute_dependency_statement(f, &mut false_values);
                for bit in lint_guaranteed_assignment_bits(f) {
                    false_values
                        .entry(bit)
                        .or_default()
                        .extend(condition.iter().copied());
                }
            }

            lint_merge_dependency_values(values, true_values);
            lint_merge_dependency_values(values, false_values);
        }
        Stmt::Case { e, arms, dflt } => {
            let selector = lint_expression_dependencies(e, values);
            let entry = values.clone();
            let mut merged = entry.clone();
            for (_, body) in arms {
                let mut branch = entry.clone();
                lint_execute_dependency_statement(body, &mut branch);
                for bit in lint_guaranteed_assignment_bits(body) {
                    branch
                        .entry(bit)
                        .or_default()
                        .extend(selector.iter().copied());
                }
                lint_merge_dependency_values(&mut merged, branch);
            }
            if let Some(body) = dflt {
                let mut branch = entry;
                lint_execute_dependency_statement(body, &mut branch);
                for bit in lint_guaranteed_assignment_bits(body) {
                    branch
                        .entry(bit)
                        .or_default()
                        .extend(selector.iter().copied());
                }
                lint_merge_dependency_values(&mut merged, branch);
            }
            *values = merged;
        }
        _ => {}
    }
}

fn lint_visit_loop_node(
    node: usize,
    graph: &std::collections::BTreeMap<usize, std::collections::BTreeSet<usize>>,
    state: &mut std::collections::BTreeMap<usize, DfsState>,
    path: &mut Vec<usize>,
    issues: &mut std::collections::BTreeMap<String, LintIssue>,
    design: &Design,
) {
    *state.entry(node).or_insert(DfsState::White) = DfsState::Gray;
    path.push(node);
    let neighbors = graph
        .get(&node)
        .map(|neighbors| neighbors.iter().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for neighbor in neighbors {
        match state.get(&neighbor) {
            None | Some(DfsState::White) => {
                lint_visit_loop_node(neighbor, graph, state, path, issues, design);
            }
            Some(DfsState::Gray) => {
                let start = path
                    .iter()
                    .position(|candidate| *candidate == neighbor)
                    .unwrap_or(path.len().saturating_sub(1));
                let cycle = &path[start..];
                let first_name = design.net_name(*cycle.first().unwrap_or(&neighbor));
                let net = first_name
                    .split('[')
                    .next()
                    .unwrap_or(&first_name)
                    .to_string();
                issues
                    .entry(net.clone())
                    .and_modify(|issue| issue.count = issue.count.max(cycle.len()))
                    .or_insert(LintIssue {
                        kind: LintIssueKind::CombinationalLoop,
                        net,
                        count: cycle.len(),
                    });
            }
            Some(DfsState::Black) => {}
        }
    }
    path.pop();
    state.insert(node, DfsState::Black);
}

fn lint_visit_case_statement(
    statement: &Stmt,
    design: &Design,
    issues: &mut std::collections::BTreeMap<String, LintIssue>,
) {
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|nested| lint_visit_case_statement(nested, design, issues)),
        Stmt::Forever(body) | Stmt::Delay { body, .. } | Stmt::Scope(body) => {
            lint_visit_case_statement(body, design, issues)
        }
        Stmt::If { t, f, .. } => {
            lint_visit_case_statement(t, design, issues);
            if let Some(f) = f {
                lint_visit_case_statement(f, design, issues);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            lint_record_case(e, arms, dflt.as_deref(), design, issues);
            for (_, body) in arms {
                lint_visit_case_statement(body, design, issues);
            }
            if let Some(body) = dflt {
                lint_visit_case_statement(body, design, issues);
            }
        }
        Stmt::Wait { body, .. } | Stmt::EventWait { body, .. } | Stmt::Repeat { body, .. } => {
            lint_visit_case_statement(body, design, issues)
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(success) = success {
                lint_visit_case_statement(success, design, issues);
            }
            if let Some(failure) = failure {
                lint_visit_case_statement(failure, design, issues);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|nested| lint_visit_case_statement(nested, design, issues)),
        Stmt::While { body, steps, .. } => {
            lint_visit_case_statement(body, design, issues);
            steps
                .iter()
                .for_each(|nested| lint_visit_case_statement(nested, design, issues));
        }
        _ => {}
    }
}

fn lint_record_case(
    selector: &Expr,
    arms: &[(Expr, Stmt)],
    default: Option<&Stmt>,
    design: &Design,
    issues: &mut std::collections::BTreeMap<String, LintIssue>,
) {
    if default.is_some() {
        return;
    }

    let width = selector.width();
    let mut missing = 1_usize;
    if width <= 12 {
        let total = 1_usize << width;
        let mut covered = std::collections::BTreeSet::new();
        let mut labels_are_constant = true;
        for (label, _) in arms {
            match label {
                Expr::Const(value) => {
                    let Some(value) = value.to_u64() else {
                        labels_are_constant = false;
                        break;
                    };
                    covered.insert(value);
                }
                _ => {
                    labels_are_constant = false;
                    break;
                }
            }
        }
        if !labels_are_constant {
            missing = 1;
        } else {
            missing = total.saturating_sub(covered.len());
        }
    }
    if missing == 0 {
        return;
    }

    let mut nets = Vec::new();
    lint_expression_nets(selector, &mut nets);
    let selector_name = nets
        .first()
        .map(|net| {
            let bit_name = design.net_name(*net);
            bit_name.split('[').next().unwrap_or(&bit_name).to_string()
        })
        .unwrap_or_else(|| format!("{selector:?}"));
    issues
        .entry(selector_name.clone())
        .and_modify(|issue| issue.count = issue.count.saturating_add(missing))
        .or_insert(LintIssue {
            kind: LintIssueKind::IncompleteCase,
            net: selector_name,
            count: missing,
        });
}

fn lint_guaranteed_assignment_bits(statement: &Stmt) -> std::collections::BTreeSet<usize> {
    match statement {
        Stmt::Bl { lv, .. } | Stmt::NBl { lv, .. } => lint_lvalue_bits(lv).into_iter().collect(),
        Stmt::Block(statements) => statements.iter().map(lint_guaranteed_assignment_bits).fold(
            Default::default(),
            |mut all: std::collections::BTreeSet<_>, nested| {
                all.extend(nested);
                all
            },
        ),
        Stmt::If { t, f, .. } => {
            let true_bits = lint_guaranteed_assignment_bits(t);
            let Some(f) = f else {
                return Default::default();
            };
            let false_bits = lint_guaranteed_assignment_bits(f);
            true_bits.intersection(&false_bits).copied().collect()
        }
        Stmt::Scope(body) => lint_guaranteed_assignment_bits(body),
        _ => Default::default(),
    }
}

fn lint_visit_latch_statement(
    statement: &Stmt,
    defaults: &std::collections::BTreeSet<usize>,
    design: &Design,
    issues: &mut std::collections::BTreeMap<String, LintIssue>,
) {
    match statement {
        Stmt::Block(statements) => {
            let mut branch_defaults = defaults.clone();
            for nested in statements {
                lint_visit_latch_statement(nested, &branch_defaults, design, issues);
                branch_defaults.extend(lint_guaranteed_assignment_bits(nested));
            }
        }
        Stmt::If { t, f, .. } => {
            lint_record_latch_risk(t, f.as_deref(), defaults, design, issues);
            lint_visit_latch_statement(t, defaults, design, issues);
            if let Some(f) = f {
                lint_visit_latch_statement(f, defaults, design, issues);
            }
        }
        Stmt::Forever(body) | Stmt::Delay { body, .. } | Stmt::Scope(body) => {
            lint_visit_latch_statement(body, defaults, design, issues)
        }
        Stmt::Wait { body, .. } | Stmt::EventWait { body, .. } | Stmt::Repeat { body, .. } => {
            lint_visit_latch_statement(body, defaults, design, issues)
        }
        Stmt::WaitOrder {
            success, failure, ..
        } => {
            if let Some(success) = success {
                lint_visit_latch_statement(success, defaults, design, issues);
            }
            if let Some(failure) = failure {
                lint_visit_latch_statement(failure, defaults, design, issues);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|nested| lint_visit_latch_statement(nested, defaults, design, issues)),
        Stmt::While { body, steps, .. } => {
            lint_visit_latch_statement(body, defaults, design, issues);
            steps
                .iter()
                .for_each(|nested| lint_visit_latch_statement(nested, defaults, design, issues));
        }
        _ => {}
    }
}

fn lint_record_latch_risk(
    true_branch: &Stmt,
    false_branch: Option<&Stmt>,
    defaults: &std::collections::BTreeSet<usize>,
    design: &Design,
    issues: &mut std::collections::BTreeMap<String, LintIssue>,
) {
    let true_bits = lint_guaranteed_assignment_bits(true_branch);
    let false_bits = false_branch
        .map(lint_guaranteed_assignment_bits)
        .unwrap_or_default();
    let mut risks = true_bits
        .difference(&false_bits)
        .copied()
        .chain(false_bits.difference(&true_bits).copied())
        .filter(|bit| !defaults.contains(bit))
        .collect::<Vec<_>>();
    risks.sort_unstable();
    if risks.is_empty() {
        return;
    }

    let first_name = design.net_name(risks[0]);
    let net = first_name
        .split('[')
        .next()
        .unwrap_or(&first_name)
        .to_string();
    issues
        .entry(net.clone())
        .and_modify(|issue| issue.count = issue.count.max(risks.len()))
        .or_insert(LintIssue {
            kind: LintIssueKind::LatchRisk,
            net,
            count: risks.len(),
        });
}

fn lint_statement_touched_nets(statement: &Stmt, touched: &mut std::collections::BTreeSet<usize>) {
    match statement {
        Stmt::Block(statements) => statements
            .iter()
            .for_each(|nested| lint_statement_touched_nets(nested, touched)),
        Stmt::Forever(body) | Stmt::Scope(body) => lint_statement_touched_nets(body, touched),
        Stmt::Bl { lv, e } | Stmt::NBl { lv, e } => {
            let mut bits = lint_lvalue_bits(lv);
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
        }
        Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
            for index in indexes {
                let mut bits = Vec::new();
                lint_expression_nets(index, &mut bits);
                touched.extend(bits);
            }
            let mut bits = Vec::new();
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
        }
        Stmt::MemoryCopy { .. } | Stmt::MemoryDelete { .. } => {}
        Stmt::MemoryNew { size, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(size, &mut bits);
            touched.extend(bits);
        }
        Stmt::QueuePush { value, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(value, &mut bits);
            touched.extend(bits);
        }
        Stmt::QueuePop { destination, .. } => touched.extend(lint_lvalue_bits(destination)),
        Stmt::QueueInsert { index, value, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(index, &mut bits);
            lint_expression_nets(value, &mut bits);
            touched.extend(bits);
        }
        Stmt::QueueDelete { index, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(index, &mut bits);
            touched.extend(bits);
        }
        Stmt::AssocDelete {
            index: Some(index), ..
        } => {
            let mut bits = Vec::new();
            lint_expression_nets(index, &mut bits);
            touched.extend(bits);
        }
        Stmt::AssocDelete { index: None, .. } => {}
        Stmt::AssocWrite { index, value, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(index, &mut bits);
            lint_expression_nets(value, &mut bits);
            touched.extend(bits);
        }
        Stmt::AssocTraversal {
            key, destination, ..
        } => {
            touched.extend(lint_lvalue_bits(key));
            touched.extend(lint_lvalue_bits(destination));
        }
        Stmt::MemoryBlPart {
            indexes, start, e, ..
        }
        | Stmt::MemoryNBlPart {
            indexes, start, e, ..
        } => {
            for index in indexes {
                let mut bits = Vec::new();
                lint_expression_nets(index, &mut bits);
                touched.extend(bits);
            }
            let mut bits = Vec::new();
            lint_expression_nets(start, &mut bits);
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
        }
        Stmt::BlIndexed {
            net,
            decl_width,
            start,
            e,
            ..
        }
        | Stmt::NBlIndexed {
            net,
            decl_width,
            start,
            e,
            ..
        } => {
            touched.extend(*net..*net + *decl_width);
            let mut bits = Vec::new();
            lint_expression_nets(start, &mut bits);
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
        }
        Stmt::If { c, t, f } => {
            let mut bits = Vec::new();
            lint_expression_nets(c, &mut bits);
            touched.extend(bits);
            lint_statement_touched_nets(t, touched);
            if let Some(f) = f {
                lint_statement_touched_nets(f, touched);
            }
        }
        Stmt::Case { e, arms, dflt } => {
            let mut bits = Vec::new();
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
            for (label, body) in arms {
                let mut bits = Vec::new();
                lint_expression_nets(label, &mut bits);
                touched.extend(bits);
                lint_statement_touched_nets(body, touched);
            }
            if let Some(body) = dflt {
                lint_statement_touched_nets(body, touched);
            }
        }
        Stmt::Delay { body, .. } => lint_statement_touched_nets(body, touched),
        Stmt::Repeat { count, body } => {
            let mut bits = Vec::new();
            lint_expression_nets(count, &mut bits);
            touched.extend(bits);
            lint_statement_touched_nets(body, touched)
        }
        Stmt::Wait { cond, body } => {
            let mut bits = Vec::new();
            lint_expression_nets(cond, &mut bits);
            touched.extend(bits);
            lint_statement_touched_nets(body, touched);
        }
        Stmt::EventWait { watches, body } => {
            touched.extend(watches.iter().map(|watch| watch.net));
            lint_statement_touched_nets(body, touched);
        }
        Stmt::WaitOrder {
            events,
            success,
            failure,
        } => {
            touched.extend(events.iter().copied());
            for body in success.iter().chain(failure.iter()) {
                lint_statement_touched_nets(body, touched);
            }
        }
        Stmt::Fork { branches, .. } => branches
            .iter()
            .for_each(|nested| lint_statement_touched_nets(nested, touched)),
        Stmt::While { cond, body, steps } => {
            let mut bits = Vec::new();
            lint_expression_nets(cond, &mut bits);
            touched.extend(bits);
            lint_statement_touched_nets(body, touched);
            for nested in steps {
                lint_statement_touched_nets(nested, touched);
            }
        }
        Stmt::Force { lv, e } => {
            touched.extend(lint_lvalue_bits(lv));
            let mut bits = Vec::new();
            lint_expression_nets(e, &mut bits);
            touched.extend(bits);
        }
        Stmt::Release { lv } => touched.extend(lint_lvalue_bits(lv)),
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Display { args, .. } => {
            for argument in args {
                let mut bits = Vec::new();
                lint_expression_nets(argument, &mut bits);
                touched.extend(bits);
            }
        }
        Stmt::TimeFormat { .. } | Stmt::Finish => {}
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            let mut bits = Vec::new();
            lint_expression_nets(cond, &mut bits);
            touched.extend(bits);
        }
        Stmt::WriteMem { start, finish, .. } => {
            for expression in start.iter().chain(finish.iter()) {
                let mut bits = Vec::new();
                lint_expression_nets(expression, &mut bits);
                touched.extend(bits);
            }
        }
    }
}

fn lint_sensitivity_touched_nets(
    sensitivity: &Sens,
    touched: &mut std::collections::BTreeSet<usize>,
) {
    match sensitivity {
        Sens::Comb | Sens::Timed => {}
        Sens::Any(net) | Sens::Posedge(net) | Sens::Negedge(net) => {
            touched.insert(*net);
        }
        Sens::List(items) => items
            .iter()
            .for_each(|item| lint_sensitivity_touched_nets(item, touched)),
    }
}

#[derive(Clone, Debug)]
pub struct ClockingBlock {
    pub name: String,
    pub event: EventWatch,
    pub inputs: Vec<ClockingInput>,
}

#[derive(Clone, Debug)]
pub struct ClockingInput {
    pub target: LValue,
    pub sample: LValue,
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

    /// Report bits driven by more than one continuous assignment.
    ///
    /// This diagnostic does not change net resolution; the simulator retains
    /// its existing multi-driver behavior.
    pub fn lint_multiple_continuous_drivers(&self) -> Vec<LintIssue> {
        let mut driver_counts: std::collections::BTreeMap<usize, usize> =
            std::collections::BTreeMap::new();
        for assignment in &self.assigns {
            let bits = match assignment.lv.range {
                Range::Bit(bit) => vec![assignment.lv.net + bit],
                Range::Span { msb, lsb } => (lsb..=msb)
                    .map(|bit| assignment.lv.net + bit)
                    .collect::<Vec<_>>(),
            };
            for bit in bits {
                *driver_counts.entry(bit).or_insert(0) += 1;
            }
        }

        let mut issues: std::collections::BTreeMap<String, LintIssue> =
            std::collections::BTreeMap::new();
        for (bit, driver_count) in driver_counts {
            if driver_count <= 1 {
                continue;
            }
            let bit_name = self.net_name(bit);
            let net = bit_name.split('[').next().unwrap_or(&bit_name).to_string();
            issues
                .entry(net.clone())
                .and_modify(|issue| issue.count = issue.count.max(driver_count))
                .or_insert(LintIssue {
                    kind: LintIssueKind::MultipleContinuousDrivers,
                    net,
                    count: driver_count,
                });
        }
        issues.into_values().collect()
    }

    /// Report cycles among zero-delay continuous assignments.
    ///
    /// The graph also includes simple guaranteed assignments from combinational
    /// processes. Event-driven, delayed, and unsupported process constructs are
    /// deliberately ignored rather than guessed.
    pub fn lint_combinational_loops(&self) -> Vec<LintIssue> {
        let mut graph: std::collections::BTreeMap<usize, std::collections::BTreeSet<usize>> =
            std::collections::BTreeMap::new();
        for assignment in &self.assigns {
            if assignment.delay != 0 {
                continue;
            }
            let mut reads = Vec::new();
            lint_expression_nets(&assignment.e, &mut reads);
            let writes = lint_lvalue_bits(&assignment.lv);
            for read in reads {
                graph
                    .entry(read)
                    .or_default()
                    .extend(writes.iter().copied());
            }
            for write in writes {
                graph.entry(write).or_default();
            }
        }
        for process in &self.processes {
            if !matches!(process.sens, Sens::Comb) {
                continue;
            }
            let mut values = (0..self.n_nets)
                .map(|bit| (bit, std::collections::BTreeSet::from([bit])))
                .collect::<LintDependencyValues>();
            lint_execute_dependency_statement(&process.body, &mut values);
            for bit in lint_guaranteed_assignment_bits(&process.body) {
                graph.entry(bit).or_default();
                if let Some(dependencies) = values.get(&bit) {
                    for dependency in dependencies {
                        graph.entry(*dependency).or_default().insert(bit);
                    }
                }
            }
        }

        let mut state: std::collections::BTreeMap<usize, DfsState> =
            std::collections::BTreeMap::new();
        let mut path = Vec::new();
        let mut issues: std::collections::BTreeMap<String, LintIssue> =
            std::collections::BTreeMap::new();
        for node in graph.keys().copied().collect::<Vec<_>>() {
            if state
                .get(&node)
                .is_some_and(|state| *state != DfsState::White)
            {
                continue;
            }
            lint_visit_loop_node(node, &graph, &mut state, &mut path, &mut issues, self);
        }
        issues.into_values().collect()
    }

    /// Report case statements whose label set is not provably exhaustive.
    ///
    /// A `default` arm proves exhaustiveness. Without one, only a fully
    /// covered small constant selector is considered complete.
    pub fn lint_incomplete_cases(&self) -> Vec<LintIssue> {
        let mut issues: std::collections::BTreeMap<String, LintIssue> =
            std::collections::BTreeMap::new();
        for process in &self.processes {
            lint_visit_case_statement(&process.body, self, &mut issues);
        }
        for statement in self.initials.iter().chain(self.final_blocks.iter()) {
            lint_visit_case_statement(statement, self, &mut issues);
        }
        issues.into_values().collect()
    }

    /// Report combinational-process targets assigned on only some paths.
    ///
    /// This lint recognizes unconditional defaults, matching if/else paths,
    /// and indexed part writes. More complete abstract interpretation is still
    /// future work; unknown constructs are ignored rather than guessed.
    pub fn lint_latch_risks(&self) -> Vec<LintIssue> {
        let mut issues: std::collections::BTreeMap<String, LintIssue> =
            std::collections::BTreeMap::new();
        for process in &self.processes {
            if !matches!(process.sens, Sens::Comb) {
                continue;
            }
            lint_visit_latch_statement(
                &process.body,
                &std::collections::BTreeSet::new(),
                self,
                &mut issues,
            );
        }
        issues.into_values().collect()
    }

    /// Report top-level user-named nets that no supported IR object touches.
    ///
    /// Ports, memories, hierarchy-flattened dotted names, and compiler-generated
    /// names are excluded because external or object references do not prove
    /// that a declaration is unused.
    pub fn lint_unused_names(&self) -> Vec<LintIssue> {
        let mut touched = std::collections::BTreeSet::new();
        for assignment in &self.assigns {
            let mut bits = Vec::new();
            lint_expression_nets(&assignment.e, &mut bits);
            touched.extend(bits);
            touched.extend(lint_lvalue_bits(&assignment.lv));
        }
        for process in &self.processes {
            lint_sensitivity_touched_nets(&process.sens, &mut touched);
            lint_statement_touched_nets(&process.body, &mut touched);
        }
        for statement in self.initials.iter().chain(self.final_blocks.iter()) {
            lint_statement_touched_nets(statement, &mut touched);
        }
        for block in &self.clocking_blocks {
            for input in &block.inputs {
                touched.extend(lint_lvalue_bits(&input.target));
                touched.extend(lint_lvalue_bits(&input.sample));
            }
        }

        let port_bits = self
            .ports
            .iter()
            .flat_map(|port| port.base..port.base + port.width)
            .collect::<std::collections::BTreeSet<_>>();
        let memory_names = self
            .memories
            .iter()
            .map(|memory| memory.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        self.net_decls
            .iter()
            .filter(|declaration| {
                !declaration.name.starts_with("__")
                    && !declaration.name.contains('.')
                    && !port_bits.contains(&declaration.base)
                    && !memory_names.contains(declaration.name.as_str())
            })
            .filter_map(|declaration| {
                let all_bits_unused = (declaration.base..declaration.base + declaration.width)
                    .all(|bit| !touched.contains(&bit));
                all_bits_unused.then_some(LintIssue {
                    kind: LintIssueKind::UnusedName,
                    net: declaration.name.clone(),
                    count: declaration.width,
                })
            })
            .collect()
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
        for final_block in &self.final_blocks {
            collect_memory_accesses(final_block, false, &memory_widths, &mut accesses);
        }
        for assignment in &self.assigns {
            collect_memory_expr_reads(&assignment.e, false, true, &mut accesses);
        }
        let aliases = self.memory_alias_roots();
        let accesses = accesses.canonicalize(&aliases);
        for (index, memory) in self.memories.iter_mut().enumerate() {
            if memory.associative {
                memory.storage = MemoryStorage::Ordinary;
                memory.kind = MemoryKind::Ram;
                memory.template = MemoryTemplate::Unmapped;
                memory.write_mask = None;
                memory.byte_enable = None;
                memory.banking = MemoryBanking::default();
                continue;
            }
            if memory.dynamic {
                memory.storage = MemoryStorage::Ordinary;
                memory.kind = MemoryKind::Ram;
                memory.template = MemoryTemplate::Unmapped;
                memory.write_mask = None;
                memory.byte_enable = None;
                memory.banking = MemoryBanking::default();
                continue;
            }
            let root = aliases.get(index).copied().unwrap_or(index);
            let total_bits =
                u128::from(memory.flat_length) * u128::try_from(memory.element_width).unwrap_or(0);
            let sparse = total_bits > 4096
                || accesses.sync_writes.contains(&root)
                || accesses.async_writes.contains(&root)
                || accesses.sync_reads.contains(&root)
                || accesses.async_reads.contains(&root);
            memory.storage = if sparse {
                MemoryStorage::Sparse
            } else {
                MemoryStorage::Ordinary
            };
            if !sparse {
                memory.kind = MemoryKind::Ram;
                memory.template = MemoryTemplate::Unmapped;
                memory.write_mask = None;
                memory.byte_enable = None;
                memory.banking = MemoryBanking::default();
                continue;
            }
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

        let ports = std::mem::take(&mut self.memory_ports)
            .into_iter()
            .map(|port| (port.name, port.direction, port.memory))
            .chain(
                std::mem::take(&mut self.array_ports)
                    .into_iter()
                    .map(|port| (port.name, port.direction, port.array)),
            )
            .collect::<Vec<_>>();
        self.array_ports.clear();
        for (name, direction, memory) in ports {
            let storage = self
                .memories
                .get(memory)
                .map(|memory| memory.storage)
                .unwrap_or(MemoryStorage::Sparse);
            if storage == MemoryStorage::Ordinary {
                self.array_ports.push(ArrayPortDecl {
                    name,
                    direction,
                    array: memory,
                });
            } else {
                self.memory_ports.push(MemoryPortDecl {
                    name,
                    direction,
                    memory,
                });
            }
        }

        let connections = std::mem::take(&mut self.memory_connections)
            .into_iter()
            .map(|connection| (connection.child, connection.parent))
            .chain(
                std::mem::take(&mut self.array_connections)
                    .into_iter()
                    .map(|connection| (connection.child, connection.parent)),
            )
            .collect::<Vec<_>>();
        for (child, parent) in connections {
            let storage = self
                .memories
                .get(child)
                .or_else(|| self.memories.get(parent))
                .map(|memory| memory.storage)
                .unwrap_or(MemoryStorage::Sparse);
            if storage == MemoryStorage::Ordinary {
                self.array_connections
                    .push(ArrayConnection { child, parent });
            } else {
                self.memory_connections
                    .push(MemoryConnection { child, parent });
            }
        }
    }

    fn memory_alias_roots(&self) -> Vec<usize> {
        let mut aliases = (0..self.memories.len()).collect::<Vec<_>>();
        for _ in 0..self.memories.len() {
            for connection in &self.memory_connections {
                let (child, parent) = (connection.child, connection.parent);
                Self::alias_memory(&mut aliases, child, parent);
            }
            for connection in &self.array_connections {
                let (child, parent) = (connection.child, connection.parent);
                Self::alias_memory(&mut aliases, child, parent);
            }
        }
        aliases
    }

    fn alias_memory(aliases: &mut [usize], child: usize, parent: usize) {
        if let Some(parent) = aliases.get(parent).copied() {
            if let Some(alias) = aliases.get_mut(child) {
                *alias = parent;
            }
        }
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
    continuous_reads: std::collections::HashSet<usize>,
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
            continuous_reads: canonicalize_set(self.continuous_reads, aliases),
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
        let async_read =
            self.async_reads.contains(&memory) || self.continuous_reads.contains(&memory);
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
            collect_memory_expr_reads(cond, synchronous, false, accesses);
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
            collect_memory_expr_reads(cond, synchronous, false, accesses);
            collect_memory_accesses(body, synchronous, memory_widths, accesses);
            for step in steps {
                collect_memory_accesses(step, synchronous, memory_widths, accesses);
            }
        }
        Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
        Stmt::Scope(body) => collect_memory_accesses(body, synchronous, memory_widths, accesses),
        Stmt::Repeat { count, body } => {
            collect_memory_expr_reads(count, synchronous, false, accesses);
            collect_memory_accesses(body, synchronous, memory_widths, accesses)
        }
        Stmt::Force { e, .. } => collect_memory_expr_reads(e, synchronous, false, accesses),
        Stmt::Release { .. } => {}
        Stmt::MemoryBl { memory, indexes, e } | Stmt::MemoryNBl { memory, indexes, e } => {
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, false, accesses);
            }
            collect_memory_expr_reads(e, synchronous, false, accesses);
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
        Stmt::MemoryCopy {
            source,
            destination,
            ..
        } => {
            if synchronous {
                accesses.sync_reads.insert(*source);
                accesses.sync_writes.insert(*destination);
            } else {
                accesses.async_reads.insert(*source);
                accesses.async_writes.insert(*destination);
            }
            accesses
                .write_observations
                .entry(*destination)
                .or_default()
                .push(MemoryWriteObservation {
                    start: Some(0),
                    width: memory_widths.get(*destination).copied().unwrap_or(0),
                    ascending: true,
                });
        }
        Stmt::MemoryNew { size, .. } => {
            collect_memory_expr_reads(size, synchronous, false, accesses)
        }
        Stmt::MemoryDelete { .. } => {}
        Stmt::QueuePush { value, .. } => {
            collect_memory_expr_reads(value, synchronous, false, accesses)
        }
        Stmt::QueueInsert { index, value, .. } => {
            collect_memory_expr_reads(index, synchronous, false, accesses);
            collect_memory_expr_reads(value, synchronous, false, accesses)
        }
        Stmt::QueueDelete { index, .. } => {
            collect_memory_expr_reads(index, synchronous, false, accesses)
        }
        Stmt::QueuePop { .. } => {}
        Stmt::AssocWrite { index, value, .. } => {
            collect_memory_expr_reads(index, synchronous, false, accesses);
            collect_memory_expr_reads(value, synchronous, false, accesses);
        }
        Stmt::AssocDelete { index, .. } => {
            if let Some(index) = index {
                collect_memory_expr_reads(index, synchronous, false, accesses);
            }
        }
        Stmt::AssocTraversal { memory, .. } => {
            if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
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
                collect_memory_expr_reads(index, synchronous, false, accesses);
            }
            collect_memory_expr_reads(start, synchronous, false, accesses);
            collect_memory_expr_reads(e, synchronous, false, accesses);
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
            collect_memory_expr_reads(e, synchronous, false, accesses);
        }
        Stmt::BlIndexed { start, e, .. } | Stmt::NBlIndexed { start, e, .. } => {
            collect_memory_expr_reads(start, synchronous, false, accesses);
            collect_memory_expr_reads(e, synchronous, false, accesses);
        }
        Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
            collect_memory_expr_reads(cond, synchronous, false, accesses)
        }
        Stmt::Display { args, .. } => {
            for argument in args {
                collect_memory_expr_reads(argument, synchronous, false, accesses);
            }
        }
        Stmt::TimeFormat { .. } | Stmt::WriteMem { .. } | Stmt::Finish => {}
    }
}

fn collect_memory_expr_reads(
    expression: &Expr,
    synchronous: bool,
    continuous: bool,
    accesses: &mut MemoryAccesses,
) {
    match expression {
        Expr::Const(_)
        | Expr::Ref { .. }
        | Expr::SimulationTime { .. }
        | Expr::MemorySize { .. } => {}
        Expr::AssocRead { memory, index, .. } | Expr::AssocExists { memory, index } => {
            if continuous {
                accesses.continuous_reads.insert(*memory);
            } else if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
            collect_memory_expr_reads(index, synchronous, continuous, accesses);
        }
        Expr::MemoryRead {
            memory, indexes, ..
        } => {
            if continuous {
                accesses.continuous_reads.insert(*memory);
            } else if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, continuous, accesses);
            }
        }
        Expr::MemoryReadPart {
            memory,
            indexes,
            start,
            ..
        } => {
            if continuous {
                accesses.continuous_reads.insert(*memory);
            } else if synchronous {
                accesses.sync_reads.insert(*memory);
            } else {
                accesses.async_reads.insert(*memory);
            }
            for index in indexes {
                collect_memory_expr_reads(index, synchronous, continuous, accesses);
            }
            collect_memory_expr_reads(start, synchronous, continuous, accesses);
        }
        Expr::Un { a, .. } => collect_memory_expr_reads(a, synchronous, continuous, accesses),
        Expr::Bin { a, b, .. } => {
            collect_memory_expr_reads(a, synchronous, continuous, accesses);
            collect_memory_expr_reads(b, synchronous, continuous, accesses);
        }
        Expr::Cond { c, t, f } => {
            collect_memory_expr_reads(c, synchronous, continuous, accesses);
            collect_memory_expr_reads(t, synchronous, continuous, accesses);
            collect_memory_expr_reads(f, synchronous, continuous, accesses);
        }
        Expr::Concat(parts) => parts
            .iter()
            .for_each(|part| collect_memory_expr_reads(part, synchronous, continuous, accesses)),
        Expr::Repeat { e, .. } | Expr::Signed { e, .. } | Expr::Index { e, .. } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses)
        }
        Expr::StringLength { e } => collect_memory_expr_reads(e, synchronous, continuous, accesses),
        Expr::StringConcat { left, right } => {
            collect_memory_expr_reads(left, synchronous, continuous, accesses);
            collect_memory_expr_reads(right, synchronous, continuous, accesses);
        }
        Expr::StringGetByte { e, index } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses);
            collect_memory_expr_reads(index, synchronous, continuous, accesses);
        }
        Expr::StringCase { e, .. } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses)
        }
        Expr::StringCompare { left, right, .. } => {
            collect_memory_expr_reads(left, synchronous, continuous, accesses);
            collect_memory_expr_reads(right, synchronous, continuous, accesses);
        }
        Expr::EventTriggered { .. } => {}
        Expr::StringPutc { e, index, value } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses);
            collect_memory_expr_reads(index, synchronous, continuous, accesses);
            collect_memory_expr_reads(value, synchronous, continuous, accesses);
        }
        Expr::StringSubstr { e, left, right } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses);
            collect_memory_expr_reads(left, synchronous, continuous, accesses);
            collect_memory_expr_reads(right, synchronous, continuous, accesses);
        }
        Expr::StringToRadix { e, .. } => {
            collect_memory_expr_reads(e, synchronous, continuous, accesses);
        }
        Expr::StringFromRadix { value, .. } => {
            collect_memory_expr_reads(value, synchronous, continuous, accesses);
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
    fn lint_reports_overlapping_continuous_drivers_only() {
        let mut design = Design {
            name: "drivers".into(),
            ..Default::default()
        };
        design.net_decls.push(NetDecl {
            name: "bus".into(),
            width: 4,
            is_reg: false,
            is_signed: false,
            base: 0,
        });
        design.n_nets = 4;
        let constant = || Expr::Const(LogicWord::zero(1));
        let driver = |msb: usize, lsb: usize| Assign {
            lv: LValue {
                net: 0,
                decl_width: 4,
                range: Range::Span { msb, lsb },
            },
            e: constant(),
            delay: 0,
        };
        design.assigns.push(driver(1, 0));
        design.assigns.push(driver(3, 2));
        assert!(design.lint_multiple_continuous_drivers().is_empty());

        design.assigns.push(driver(3, 1));
        assert_eq!(
            design.lint_multiple_continuous_drivers(),
            vec![LintIssue {
                kind: LintIssueKind::MultipleContinuousDrivers,
                net: "bus".into(),
                count: 2,
            }]
        );
    }

    #[test]
    fn lint_reports_continuous_assignment_cycles() {
        let mut design = Design {
            name: "loop".into(),
            ..Default::default()
        };
        for name in ["x", "y", "z"] {
            let base = design.n_nets;
            design.net_decls.push(NetDecl {
                name: name.into(),
                width: 1,
                is_reg: false,
                is_signed: false,
                base,
            });
            design.n_nets += 1;
        }
        let reference = |net: usize| Expr::Ref {
            net,
            decl_width: 1,
            range: Range::Bit(0),
            signed: false,
        };
        let assignment = |target: usize, source: usize| Assign {
            lv: LValue {
                net: target,
                decl_width: 1,
                range: Range::Bit(0),
            },
            e: reference(source),
            delay: 0,
        };
        design.assigns.push(assignment(0, 1));
        design.assigns.push(assignment(1, 2));
        assert!(design.lint_combinational_loops().is_empty());

        design.assigns.push(assignment(2, 0));
        assert_eq!(
            design.lint_combinational_loops(),
            vec![LintIssue {
                kind: LintIssueKind::CombinationalLoop,
                net: "x".into(),
                count: 3,
            }]
        );
    }

    #[test]
    fn lint_reports_process_combinational_cycles_without_feedthrough_false_positive() {
        let make_design = |body: Stmt| {
            let mut design = Design {
                name: "process_loop".into(),
                ..Default::default()
            };
            for name in ["a", "b"] {
                let base = design.n_nets;
                design.net_decls.push(NetDecl {
                    name: name.into(),
                    width: 1,
                    is_reg: true,
                    is_signed: false,
                    base,
                });
                design.n_nets += 1;
            }
            design.processes.push(Process {
                sens: Sens::Comb,
                body,
            });
            design
        };
        let reference = |net: usize| Expr::Ref {
            net,
            decl_width: 1,
            range: Range::Bit(0),
            signed: false,
        };
        let assignment = |target: usize, source: Expr| Stmt::Bl {
            lv: LValue {
                net: target,
                decl_width: 1,
                range: Range::Bit(0),
            },
            e: source,
        };

        let cyclic = make_design(Stmt::Block(vec![
            assignment(0, reference(1)),
            assignment(1, reference(0)),
        ]));
        assert_eq!(
            cyclic.lint_combinational_loops(),
            vec![LintIssue {
                kind: LintIssueKind::CombinationalLoop,
                net: "b".into(),
                count: 1,
            }]
        );

        let feedthrough = make_design(Stmt::Block(vec![
            assignment(0, reference(1)),
            assignment(1, Expr::Const(LogicWord::from_u64(1, 0))),
        ]));
        assert!(feedthrough.lint_combinational_loops().is_empty());
    }

    #[test]
    fn lint_reports_only_uncovered_constant_cases() {
        let mut design = Design {
            name: "cases".into(),
            ..Default::default()
        };
        let selector = || Expr::Ref {
            net: 0,
            decl_width: 2,
            range: Range::Span { msb: 1, lsb: 0 },
            signed: false,
        };
        let label = |value: u64| Expr::Const(LogicWord::from_u64(2, value));
        let assignment = || Stmt::Bl {
            lv: LValue {
                net: 0,
                decl_width: 2,
                range: Range::Span { msb: 1, lsb: 0 },
            },
            e: label(0),
        };
        design.processes.push(Process {
            sens: Sens::Comb,
            body: Stmt::Case {
                e: selector(),
                arms: vec![(label(0), assignment()), (label(1), assignment())],
                dflt: None,
            },
        });
        assert_eq!(
            design.lint_incomplete_cases(),
            vec![LintIssue {
                kind: LintIssueKind::IncompleteCase,
                net: "net0".into(),
                count: 2,
            }]
        );

        let mut complete = design;
        if let Stmt::Case { arms, .. } = &mut complete.processes[0].body {
            arms.push((label(2), assignment()));
            arms.push((label(3), assignment()));
        }
        assert!(complete.lint_incomplete_cases().is_empty());
    }

    #[test]
    fn lint_reports_latch_risks_only_without_full_path_assignment() {
        let mut design = Design {
            name: "latches".into(),
            ..Default::default()
        };
        design.net_decls.push(NetDecl {
            name: "q".into(),
            width: 2,
            is_reg: true,
            is_signed: false,
            base: 0,
        });
        design.net_decls.push(NetDecl {
            name: "en".into(),
            width: 1,
            is_reg: false,
            is_signed: false,
            base: 2,
        });
        design.n_nets = 3;
        let enable = || Expr::Ref {
            net: 2,
            decl_width: 1,
            range: Range::Bit(0),
            signed: false,
        };
        let assignment = |value: u64| Stmt::Bl {
            lv: LValue {
                net: 0,
                decl_width: 2,
                range: Range::Span { msb: 1, lsb: 0 },
            },
            e: Expr::Const(LogicWord::from_u64(2, value)),
        };
        design.processes.push(Process {
            sens: Sens::Comb,
            body: Stmt::If {
                c: enable(),
                t: Box::new(assignment(2)),
                f: None,
            },
        });
        assert_eq!(
            design.lint_latch_risks(),
            vec![LintIssue {
                kind: LintIssueKind::LatchRisk,
                net: "q".into(),
                count: 2,
            }]
        );

        if let Stmt::If { f, .. } = &mut design.processes[0].body {
            *f = Some(Box::new(assignment(3)));
        }
        assert!(design.lint_latch_risks().is_empty());

        if let Stmt::If { f, .. } = &mut design.processes[0].body {
            *f = None;
        }
        let process = &mut design.processes[0];
        process.body = Stmt::Block(vec![assignment(0), process.body.clone()]);
        assert!(design.lint_latch_risks().is_empty());
    }

    #[test]
    fn lint_reports_completely_unused_user_names_only() {
        let mut design = Design {
            name: "unused_names".into(),
            ..Default::default()
        };
        for (name, width, base) in [
            ("unused", 2, 0),
            ("partial", 2, 2),
            ("used", 1, 4),
            ("sensitive", 1, 5),
            ("repeat_count", 1, 6),
            ("forced", 1, 7),
            ("hier.signal", 1, 8),
            ("__generated", 1, 9),
        ] {
            design.net_decls.push(NetDecl {
                name: name.into(),
                width,
                is_reg: false,
                is_signed: false,
                base,
            });
            design.n_nets += width;
        }
        design.ports.push(PortDecl {
            name: "y".into(),
            dir: Dir::Out,
            base: 6,
            width: 1,
        });
        design.n_nets += 1;
        design.processes.push(Process {
            sens: Sens::Any(5),
            body: Stmt::Block(vec![
                Stmt::Repeat {
                    count: Expr::Ref {
                        net: 6,
                        decl_width: 1,
                        range: Range::Bit(0),
                        signed: false,
                    },
                    body: Box::new(Stmt::Finish),
                },
                Stmt::Force {
                    lv: LValue {
                        net: 7,
                        decl_width: 1,
                        range: Range::Bit(0),
                    },
                    e: Expr::Const(LogicWord::from_u64(1, 1)),
                },
            ]),
        });
        design.assigns.push(Assign {
            lv: LValue {
                net: 6,
                decl_width: 1,
                range: Range::Bit(0),
            },
            e: Expr::Ref {
                net: 4,
                decl_width: 1,
                range: Range::Bit(0),
                signed: false,
            },
            delay: 0,
        });
        design.assigns.push(Assign {
            lv: LValue {
                net: 2,
                decl_width: 2,
                range: Range::Bit(1),
            },
            e: Expr::Const(LogicWord::from_u64(1, 1)),
            delay: 0,
        });

        assert_eq!(
            design.lint_unused_names(),
            vec![LintIssue {
                kind: LintIssueKind::UnusedName,
                net: "unused".into(),
                count: 2,
            }]
        );
    }

    #[test]
    fn infer_memory_kinds_partitions_and_repartitions_ports() {
        let mut design = Design {
            name: "ports".into(),
            ..Default::default()
        };
        design.memories.push(MemoryDecl {
            name: "storage".into(),
            element_width: 8,
            element_msb: 7,
            element_lsb: 0,
            dimensions: vec![MemoryDimension {
                base: 0,
                step: 1,
                length: 4,
            }],
            flat_length: 4,
            dynamic: false,
            queue: false,
            queue_bound: None,
            associative: false,
            index_width: 0,
            index_signed: false,
            storage: MemoryStorage::Ordinary,
            kind: MemoryKind::Ram,
            template: MemoryTemplate::Unmapped,
            write_mask: None,
            byte_enable: None,
            banking: MemoryBanking::default(),
        });
        design.memory_ports.push(MemoryPortDecl {
            name: "storage".into(),
            direction: MemoryPortDirection::InOut,
            memory: 0,
        });
        design.memory_connections.push(MemoryConnection {
            child: 0,
            parent: 0,
        });
        design.processes.push(Process {
            sens: Sens::Comb,
            body: Stmt::MemoryBl {
                memory: 0,
                indexes: vec![Expr::Const(LogicWord::from_u64(8, 1))],
                e: Expr::Const(LogicWord::from_u64(8, 0x5a)),
            },
        });

        design.infer_memory_kinds();
        assert_eq!(design.memories[0].storage, MemoryStorage::Sparse);
        assert_eq!(design.memory_ports.len(), 1);
        assert_eq!(design.array_ports.len(), 0);
        assert_eq!(design.memory_connections.len(), 1);
        assert_eq!(design.array_connections.len(), 0);

        design.processes.clear();
        design.infer_memory_kinds();
        assert_eq!(design.memories[0].storage, MemoryStorage::Ordinary);
        assert_eq!(design.memory_ports.len(), 0);
        assert_eq!(design.memory_connections.len(), 0);
        assert_eq!(
            design.array_connections,
            vec![ArrayConnection {
                child: 0,
                parent: 0,
            }]
        );
        assert_eq!(
            design.array_ports,
            vec![ArrayPortDecl {
                name: "storage".into(),
                direction: MemoryPortDirection::InOut,
                array: 0,
            }]
        );

        design.infer_memory_kinds();
        assert_eq!(design.memory_ports.len(), 0);
        assert_eq!(design.array_ports.len(), 1);
        assert_eq!(design.memory_connections.len(), 0);
        assert_eq!(design.array_connections.len(), 1);
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
            dynamic: false,
            queue: false,
            queue_bound: None,
            associative: false,
            index_width: 0,
            index_signed: false,
            storage: MemoryStorage::Ordinary,
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
            dynamic: false,
            queue: false,
            queue_bound: None,
            associative: false,
            index_width: 0,
            index_signed: false,
            storage: MemoryStorage::Ordinary,
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
