//! Source-level Verilog AST produced by the parser.

use eda_core::logic::LogicWord;

/// An unresolved `[msb:lsb]` range; evaluated during elaboration with
/// parameter values in scope.
pub type RangeAst = Option<(Box<Expr>, Box<Expr>)>;

/// Fixed-width string subset: 64 bytes packed least-significant byte first.
/// This is broader than the original eight-byte representation, but strings
/// are not dynamically resizable yet.
pub const STRING_MAX_BYTES: usize = 64;
pub const STRING_WIDTH: usize = STRING_MAX_BYTES * 8;

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub modules: Vec<Module>,
    pub packages: Vec<Package>,
    pub imports: Vec<Import>,
    pub classes: Vec<ClassDef>,
    pub class_specializations: Vec<ClassSpecialization>,
}

#[derive(Clone, Debug)]
pub struct ClassSpecialization {
    pub base_name: String,
    pub canonical_name: String,
    /// `None` entries represent positional actuals.
    pub parameter_names: Vec<Option<String>>,
    pub values: Vec<ClassParameterValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassParameterValue {
    Constant(i64),
    Parameter(String),
}

#[derive(Clone, Debug)]
pub struct Import {
    pub package: String,
    pub names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub params: Vec<(String, Expr)>,
    pub types: Vec<TypeDef>,
    pub functions: Vec<FunctionDef>,
    pub classes: Vec<ClassDef>,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub items: Vec<Item>,
    pub is_interface: bool,
}

#[derive(Clone, Debug)]
pub enum Item {
    /// `input [msb:lsb] a, b;` / `output reg q;`
    Port {
        dir: DirSrc,
        is_reg: bool,
        is_signed: bool,
        is_virtual: bool,
        interface_name: Option<String>,
        modport_name: Option<String>,
        type_name: Option<String>,
        range: RangeAst,
        array_ranges: Vec<RangeAst>,
        names: Vec<String>,
    },
    /// Interface `modport name(...)` declaration.
    Modport {
        name: String,
        ports: Vec<ModportPort>,
    },
    /// Clocked overlapping-implication concurrent assertion.
    AssertProperty {
        kind: PropertyKind,
        label: Option<String>,
        clock: Sens,
        disable: Expr,
        antecedent: Expr,
        consequent: Expr,
        composition: Option<Box<PropertyComposition>>,
        overlapping: bool,
        delay: Box<PropertyDelay>,
        repetition: Option<Box<PropertyRepetition>>,
        message: Option<String>,
    },
    /// `wire/reg [msb:lsb] name = init;`
    Net {
        is_reg: bool,
        is_signed: bool,
        type_name: Option<String>,
        range: RangeAst,
        array_ranges: Vec<RangeAst>,
        names: Vec<(String, Option<Expr>)>,
    },
    /// `parameter N = 8, M = 4;`
    Params(Vec<(String, Expr)>),
    /// SystemVerilog local type definition.
    TypeDef(TypeDef),
    /// SystemVerilog class definition (module or package scope).
    ClassDef(ClassDef),
    /// Simple elaboration-inline function subset.
    Function(FunctionDef),
    /// One or more `import pkg::*;` / `import pkg::name;` specifications.
    Imports(Vec<Import>),
    /// One or more loop-generate control variables.
    Genvar(Vec<String>),
    /// Verilog-2001 generate region. Constructs are expanded after module
    /// parameter resolution but before port/net declaration.
    Generate(Vec<GenerateConstruct>),
    /// `assign [#delay] lv = e;`
    Assign {
        delay: u64,
        lv: Expr,
        e: Expr,
    },
    Always {
        sens: Sens,
        body: Stmt,
    },
    Initial {
        body: Stmt,
    },
    /// `and g1(y, a, b);`
    Gate {
        kind: GateKind,
        name: String,
        args: Vec<Expr>,
    },
    /// `sub #(params) u1(.a(x), .b(y));`
    Inst {
        module: String,
        name: String,
        array_ranges: Vec<(Box<Expr>, Box<Expr>)>,
        /// Parameter overrides: (name, value). Positional overrides use
        /// `None` as the name and are matched in declaration order.
        params: Vec<(Option<String>, Expr)>,
        conns: Vec<Conn>,
    },
}

#[derive(Clone, Debug)]
pub struct FunctionDef {
    pub name: String,
    pub return_range: RangeAst,
    pub args: Vec<(String, RangeAst)>,
    /// Parallel optional default-value expressions for `args`.
    pub arg_defaults: Vec<Option<Expr>>,
    /// 类类型形式参的可选类型名（句柄按引用别名绑定）。
    pub arg_types: Vec<Option<String>>,
    pub body: Vec<Stmt>,
    /// `virtual function/task`：经基类句柄调用时按运行时类型分派。
    pub is_virtual: bool,
    /// `static function/task`：可经类名调用，不携带对象句柄。
    pub is_static: bool,
}

/// Qualifiers permitted on a SystemVerilog class property.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassPropQual {
    Rand,
    Randc,
    Static,
    Local,
    Protected,
    Const,
}

/// One `int count = 0;`-style class property declaration item.
#[derive(Clone, Debug)]
pub struct ClassProperty {
    pub quals: Vec<ClassPropQual>,
    pub is_reg: bool,
    pub is_signed: bool,
    pub type_name: Option<String>,
    pub range: RangeAst,
    pub names: Vec<(String, Option<Expr>)>,
}

/// SystemVerilog `class ... endclass` definition (including `virtual class`).
#[derive(Clone, Debug)]
pub struct ClassDef {
    pub name: String,
    /// Class parameter declarations with optional default constants.
    pub params: Vec<(String, Option<Expr>)>,
    pub extends: Option<String>,
    /// Actuals forwarded to the base-class constructor by
    /// `class D extends B(args);`.
    pub extends_args: Vec<Expr>,
    pub is_virtual: bool,
    pub properties: Vec<ClassProperty>,
    pub methods: Vec<FunctionDef>,
}

#[derive(Clone, Debug)]
pub struct ModportPort {
    pub name: String,
    pub dir: DirSrc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    Assert,
    Assume,
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyCompositionOp {
    And,
    Or,
}

#[derive(Clone, Debug)]
pub struct PropertyComposition {
    pub op: PropertyCompositionOp,
    pub right: Expr,
    pub right_delay: PropertyDelay,
}

#[derive(Clone, Debug)]
pub enum PropertyDelay {
    Fixed(Expr),
    Range(Expr, Expr),
}

#[derive(Clone, Debug)]
pub enum PropertyRepetition {
    Count(Expr),
    Range(Expr, Expr),
    Goto(Expr),
    GotoRange(Expr, Expr),
    Nonconsecutive(Expr),
    NonconsecutiveRange(Expr, Expr),
}

#[derive(Clone, Debug)]
pub struct EnumTypeDef {
    pub name: String,
    pub range: RangeAst,
    pub constants: Vec<(String, Option<Expr>)>,
}

#[derive(Clone, Debug)]
pub enum TypeDef {
    Enum(EnumTypeDef),
    Struct(StructTypeDef),
    Union(StructTypeDef),
}

#[derive(Clone, Debug)]
pub struct StructTypeDef {
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Clone, Debug)]
pub struct StructField {
    pub name: String,
    pub type_name: Option<String>,
    pub range: RangeAst,
}

#[derive(Clone, Debug)]
pub struct GenerateBlock {
    pub label: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum GenerateConstruct {
    /// A declaration inside a generate region; it controls a later loop.
    Genvar(Vec<String>),
    Block(GenerateBlock),
    Case {
        selector: Expr,
        arms: Vec<GenerateCaseArm>,
        default: Option<GenerateBlock>,
    },
    If {
        cond: Expr,
        then: GenerateBlock,
        otherwise: Option<GenerateBlock>,
    },
    For {
        var: String,
        start: Expr,
        cond: Expr,
        step: Expr,
        body: GenerateBlock,
    },
}

#[derive(Clone, Debug)]
pub struct GenerateCaseArm {
    pub labels: Vec<Expr>,
    pub body: GenerateBlock,
}

#[derive(Clone, Debug)]
pub enum Conn {
    Named(String, Option<Expr>), // .port(expr); None = unconnected
    Pos(Option<Expr>),           // positional; None = unconnected
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirSrc {
    In,
    Out,
    InOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateKind {
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
    Buf,
    Not,
}

#[derive(Clone, Debug)]
pub enum Sens {
    Comb,
    Posedge(Expr),
    Negedge(Expr),
    List(Vec<Sens>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOpSrc {
    Not,
    AndR,
    NandR,
    OrR,
    NorR,
    XorR,
    XnorR,
    Plus,
    Minus,
    LogNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOpSrc {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Num {
        width: Option<usize>,
        word: LogicWord,
        orig: String,
    },
    Str(String),
    Ident(String),
    ScopedIdent {
        package: String,
        name: String,
    },
    Select(String, Box<Expr>),
    ArraySelect(String, Vec<Expr>),
    Member(String, String),
    /// Chained member selection `obj.inner.field` (two or more segments).
    MemberChain(Vec<String>),
    /// Method call through a nested object path `obj.inner.method(...)`.
    MethodCallChain {
        path: Vec<String>,
        method: String,
        arguments: Vec<Expr>,
    },
    MethodCall {
        object: String,
        method: String,
        arguments: Vec<Expr>,
    },
    Part(String, Box<Expr>, Box<Expr>),
    New(Box<Expr>),
    /// Class constructor invocation `new` or `new(actuals)`.
    NewClass(Vec<Expr>),
    /// Indexed part-select of an unpacked-array element, such as
    /// `memory[i][base +: width]`.
    ArrayIndexedPart {
        name: String,
        indexes: Vec<Expr>,
        start: Box<Expr>,
        width: Box<Expr>,
        ascending: bool,
    },
    /// SystemVerilog indexed part-select `name[start +: width]` or
    /// `name[start -: width]`.
    IndexedPart {
        name: String,
        start: Box<Expr>,
        width: Box<Expr>,
        ascending: bool,
    },
    Un(UnOpSrc, Box<Expr>),
    Bin(BinOpSrc, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    Concat(Vec<Expr>),
    Replicate(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    /// Internal constant part-select of an arbitrary expression.
    Slice {
        value: Box<Expr>,
        msb: Box<Expr>,
        lsb: Box<Expr>,
    },
    /// Internal assignment-width normalization used by function locals.
    Resize {
        width: usize,
        value: Box<Expr>,
    },
    /// Internal node produced while merging wildcard function-case arms.
    WildcardMatch {
        kind: CaseKind,
        selector: Box<Expr>,
        label: Box<Expr>,
    },
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Block(Vec<Stmt>),
    /// Local declaration inside a function's straight-line body.
    Local {
        type_name: Option<String>,
        range: RangeAst,
        array_ranges: Vec<RangeAst>,
        names: Vec<(String, Option<Expr>)>,
    },
    /// Bounded elaboration-time loop subset used by inline functions.
    For {
        var: String,
        start: Expr,
        cond: Expr,
        step: Expr,
        body: Box<Stmt>,
    },
    /// Constant-count elaboration-time repeat subset used by inline functions.
    Repeat {
        count: Expr,
        body: Box<Stmt>,
    },
    /// Bounded elaboration-time while-loop subset used by inline functions.
    While {
        cond: Expr,
        body: Box<Stmt>,
    },
    /// Immediate procedural assertion.
    Assert {
        cond: Expr,
        message: Option<String>,
    },
    /// Void method call, currently used by bounded dynamic-array methods.
    MethodCall {
        object: String,
        method: String,
        arguments: Vec<Expr>,
    },
    Forever(Box<Stmt>),
    NBL {
        lv: Expr,
        e: Expr,
    },
    BL {
        lv: Expr,
        e: Expr,
    },
    If {
        c: Expr,
        t: Box<Stmt>,
        f: Option<Box<Stmt>>,
    },
    Case {
        e: Expr,
        arms: Vec<(Vec<Expr>, Stmt)>,
        dflt: Option<Box<Stmt>>,
        kind: CaseKind,
    },
    Delay {
        ns: u64,
        body: Box<Stmt>,
    },
    Display {
        fmt: Vec<DispPartSrc>,
        args: Vec<Expr>,
        monitor: bool,
    },
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseKind {
    Exact,
    CaseZ,
    CaseX,
}

#[derive(Clone, Debug)]
pub enum DispPartSrc {
    Str(String),
    Arg(char), // b, d, h
    Time,
}
