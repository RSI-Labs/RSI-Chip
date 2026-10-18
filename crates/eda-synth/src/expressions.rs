//! Expression bit-blasting and primitive-cone construction.

use super::Synth;
use eda_core::ir::{BinOp, Expr, Range, UnOp};
use eda_core::logic::LogicValue;
use eda_core::netlist::{CellKind, NetId};

impl<'a> Synth<'a> {
    // ---------- expression bit-blasting ----------

    pub(crate) fn blast(&mut self, e: &Expr, width: usize) -> Result<Vec<NetId>, String> {
        let mut out = self.blast_nat(e)?;
        if out.len() < width && e.is_signed() {
            if let Some(&sign) = out.last() {
                while out.len() < width {
                    out.push(sign);
                }
            }
        }
        out.truncate(width);
        while out.len() < width {
            out.push(self.const_net(LogicValue::Zero));
        }
        Ok(out)
    }

    pub(crate) fn blast_nat(&mut self, e: &Expr) -> Result<Vec<NetId>, String> {
        Ok(match e {
            Expr::Const(w) => {
                let mut nets = Vec::new();
                for b in &w.bits {
                    nets.push(self.const_net(*b));
                }
                nets
            }
            Expr::Ref { net, range, .. } => {
                let decl = self.decl_of_flat(*net)?;
                let base = decl.base;
                let w = decl.width;
                let source = self.read_sym.as_ref().unwrap_or(&self.sym);
                if !source.contains_key(&base) {
                    let nets: Vec<NetId> = (0..w).map(|i| self.flat2net[base + i]).collect();
                    if self.read_sym.is_none() {
                        self.sym.insert(base, nets.clone());
                    }
                    if let Some(read) = self.read_sym.as_mut() {
                        read.insert(base, nets.clone());
                    }
                }
                let sym = self
                    .read_sym
                    .as_ref()
                    .unwrap_or(&self.sym)
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| (0..w).map(|i| self.flat2net[base + i]).collect());
                match range {
                    Range::Bit(b) => vec![sym[*b]],
                    Range::Span { msb, lsb } => sym[*lsb..=*msb].to_vec(),
                }
            }
            Expr::Index { e, msb, lsb } => {
                let v = self.blast_nat(e)?;
                let hi = (*msb + 1).min(v.len());
                let lo = (*lsb).min(v.len());
                let mut out = if lo < hi { v[lo..hi].to_vec() } else { vec![] };
                while out.len() < msb - lsb + 1 {
                    out.push(self.const_net(LogicValue::Zero));
                }
                out
            }
            Expr::Un { op, a } => {
                let av = self.blast_nat(a)?;
                match op {
                    UnOp::Not => {
                        let mut out = Vec::with_capacity(av.len());
                        for b in av {
                            if let Some(c) = self.const_val(b) {
                                out.push(self.const_net(c.not()));
                            } else {
                                out.push(self.gate1("not", CellKind::Not, b));
                            }
                        }
                        out
                    }
                    UnOp::AndR => vec![self.reduce(&av, CellKind::And, LogicValue::One)],
                    UnOp::OrR => vec![self.reduce(&av, CellKind::Or, LogicValue::Zero)],
                    UnOp::XorR => vec![self.reduce(&av, CellKind::Xor, LogicValue::Zero)],
                }
            }
            Expr::Bin { op, a, b } => self.blast_bin(*op, a, b)?,
            Expr::Cond { c, t, f } => {
                let w = t.width().max(f.width());
                let cv = self.blast(c, 1)?;
                let tv = self.blast(t, w)?;
                let fv = self.blast(f, w)?;
                let mut out = Vec::with_capacity(w);
                for i in 0..w {
                    out.push(self.mux(cv[0], tv[i], fv[i]));
                }
                out
            }
            Expr::Concat(parts) => {
                let mut msb_first: Vec<NetId> = Vec::new();
                for p in parts {
                    let v = self.blast_nat(p)?;
                    msb_first.extend(v.iter().rev());
                }
                msb_first.reverse();
                msb_first
            }
            Expr::Repeat { times, e } => {
                let v = self.blast_nat(e)?;
                let mut out: Vec<NetId> = Vec::new();
                for _ in 0..*times {
                    out.extend(v.iter().rev());
                }
                out.reverse();
                out
            }
            Expr::Signed { e, .. } => self.blast_nat(e)?,
            Expr::MemoryRead {
                memory, indexes, ..
            } => self.blast_distributed_rom(*memory, indexes)?,
            Expr::MemoryReadPart { .. } => {
                return Err(
                    "first-class sparse-memory part-select reads are not synthesizable yet".into(),
                )
            }
            Expr::StringLength { .. } => {
                return Err("bounded string methods are not synthesizable yet".into())
            }
            Expr::StringConcat { .. } => {
                return Err("bounded string concatenation is not synthesizable yet".into())
            }
            Expr::StringGetByte { .. } => {
                return Err("bounded string indexing is not synthesizable yet".into())
            }
            Expr::StringCase { .. } => {
                return Err("bounded string case conversion is not synthesizable yet".into())
            }
            Expr::StringCompare { .. } => {
                return Err("bounded string comparison is not synthesizable yet".into())
            }
            Expr::StringPutc { .. } => {
                return Err("bounded string byte writes are not synthesizable yet".into())
            }
            Expr::StringSubstr { .. } => {
                return Err("bounded string substrings are not synthesizable yet".into())
            }
        })
    }

    pub(crate) fn decl_of_flat(&self, flat: usize) -> Result<&eda_core::ir::NetDecl, String> {
        self.design
            .net_decls
            .iter()
            .find(|d| flat >= d.base && flat < d.base + d.width)
            .ok_or_else(|| format!("flat net {} not in any declaration", flat))
    }

    pub(crate) fn mux(&mut self, s: NetId, t: NetId, f: NetId) -> NetId {
        if t == f {
            return t;
        }
        if let Some(sv) = self.const_val(s) {
            if let Some(b) = sv.as_bool() {
                return if b { t } else { f };
            }
        }
        if let (Some(tv), Some(fv)) = (self.const_val(t), self.const_val(f)) {
            if tv == LogicValue::One && fv == LogicValue::Zero {
                return s;
            }
            if tv == LogicValue::Zero && fv == LogicValue::One {
                return self.gate1("not", CellKind::Not, s);
            }
        }
        // CellKind::Mux is [sel, a, b] => sel ? b : a
        self.gate3("mux", CellKind::Mux, s, f, t)
    }

    pub(crate) fn reduce(&mut self, bits: &[NetId], kind: CellKind, unit: LogicValue) -> NetId {
        let mut acc = self.const_net(unit);
        for b in bits {
            acc = self.combine(kind.clone(), acc, *b);
        }
        acc
    }

    pub(crate) fn combine(&mut self, kind: CellKind, a: NetId, b: NetId) -> NetId {
        let (ca, cb) = (self.const_val(a), self.const_val(b));
        if let (Some(x), Some(y)) = (ca, cb) {
            return self.const_net(kind.eval(&[x, y]));
        }
        let use_x = ca.map(|v| v.as_bool());
        let use_y = cb.map(|v| v.as_bool());
        if let Some(Some(v)) = use_x {
            match (&kind, v) {
                (CellKind::And, false) => return self.const_net(LogicValue::Zero),
                (CellKind::And, true) => return b,
                (CellKind::Or, true) => return self.const_net(LogicValue::One),
                (CellKind::Or, false) => return b,
                (CellKind::Xor, false) => return b,
                (CellKind::Xor, true) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Nand, false) => return self.const_net(LogicValue::One),
                (CellKind::Nand, true) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Nor, true) => return self.const_net(LogicValue::Zero),
                (CellKind::Nor, false) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Xnor, false) => return self.gate1("not", CellKind::Not, b),
                (CellKind::Xnor, true) => return b,
                _ => {}
            }
        }
        if let Some(Some(v)) = use_y {
            match (&kind, v) {
                (CellKind::And, false) => return self.const_net(LogicValue::Zero),
                (CellKind::And, true) => return a,
                (CellKind::Or, true) => return self.const_net(LogicValue::One),
                (CellKind::Or, false) => return a,
                (CellKind::Xor, false) => return a,
                (CellKind::Xor, true) => return self.gate1("not", CellKind::Not, a),
                _ => {}
            }
        }
        if a == b {
            match &kind {
                CellKind::And | CellKind::Or => return a,
                CellKind::Xor => return self.const_net(LogicValue::Zero),
                CellKind::Xnor => return self.const_net(LogicValue::One),
                CellKind::Nand | CellKind::Nor => return self.gate1("not", CellKind::Not, a),
                _ => {}
            }
        }
        let out = self.new_tmp("g");
        self.add_cell("g", kind, vec![a, b], out);
        out
    }

    pub(crate) fn blast_bin(
        &mut self,
        op: BinOp,
        a: &Expr,
        b: &Expr,
    ) -> Result<Vec<NetId>, String> {
        let (wa, wb) = (a.width(), b.width());
        match op {
            BinOp::Div | BinOp::Mod => {
                Err("synthesis does not support division/modulo operators".into())
            }
            BinOp::And | BinOp::Or | BinOp::Xor => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let kind = match op {
                    BinOp::And => CellKind::And,
                    BinOp::Or => CellKind::Or,
                    _ => CellKind::Xor,
                };
                let mut out = Vec::with_capacity(w);
                for i in 0..w {
                    out.push(self.combine(kind.clone(), av[i], bv[i]));
                }
                Ok(out)
            }
            BinOp::Add | BinOp::Sub => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let cin = if op == BinOp::Sub {
                    let comp: Vec<NetId> = bv.iter().map(|n| self.not(*n)).collect();
                    let one = self.const_net(LogicValue::One);
                    let (sum, _) = self.adder(&av, &comp, one);
                    return Ok(sum);
                } else {
                    self.const_net(LogicValue::Zero)
                };
                let (sum, _) = self.adder(&av, &bv, cin);
                Ok(sum)
            }
            BinOp::Mul => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                Ok(self.multiply_nets(&av, &bv, w))
            }
            BinOp::Pow => {
                let w = wa.max(wb);
                if w > 16 {
                    return Err("synthesis supports power operators only up to 16 bits".to_string());
                }
                let base = self.blast(a, w)?;
                let exponent = self.blast(b, w)?;
                let mut result: Vec<NetId> = (0..w)
                    .map(|bit| {
                        if bit == 0 {
                            self.const_net(LogicValue::One)
                        } else {
                            self.const_net(LogicValue::Zero)
                        }
                    })
                    .collect();
                let mut power = base;
                for (bit_index, exponent_bit) in exponent.iter().enumerate() {
                    let product = self.multiply_nets(&result, &power, w);
                    let mut selected = Vec::with_capacity(w);
                    for bit in 0..w {
                        selected.push(self.mux(*exponent_bit, product[bit], result[bit]));
                    }
                    result = selected;
                    if bit_index + 1 < exponent.len() {
                        power = self.multiply_nets(&power, &power, w);
                    }
                }
                if b.is_signed() {
                    let negative = exponent[w - 1];
                    let zero = self.const_net(LogicValue::Zero);
                    let mut truncated = Vec::with_capacity(w);
                    for bit in result {
                        truncated.push(self.mux(negative, zero, bit));
                    }
                    result = truncated;
                }
                Ok(result)
            }
            BinOp::Shl | BinOp::Shr => {
                let av = self.blast(a, wa)?;
                let bbits = b.width().min(6);
                let bv = self.blast(b, bbits)?;
                let right = op == BinOp::Shr;
                let sign = if right && a.is_signed() {
                    Some(av[av.len() - 1])
                } else {
                    None
                };
                let mut cur = av;
                for (k, bit) in bv.iter().enumerate() {
                    let sh = 1usize << k;
                    let zero = self.const_net(LogicValue::Zero);
                    let fill = sign.unwrap_or(zero);
                    let mut shifted = vec![zero; cur.len()];
                    for i in 0..cur.len() {
                        if right {
                            if i + sh < cur.len() {
                                shifted[i] = cur[i + sh];
                            } else {
                                shifted[i] = fill;
                            }
                        } else if i >= sh {
                            shifted[i] = cur[i - sh];
                        }
                    }
                    let mut next = Vec::with_capacity(cur.len());
                    for i in 0..cur.len() {
                        next.push(self.mux(*bit, shifted[i], cur[i]));
                    }
                    cur = next;
                }
                Ok(cur)
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let eq = self.eq_net(&av, &bv);
                let signed_compare = a.is_signed() && b.is_signed();
                let net = match op {
                    BinOp::Eq => eq,
                    BinOp::Ne => self.not(eq),
                    BinOp::Lt => {
                        if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        }
                    }
                    BinOp::Le => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        self.or(eq, lt)
                    }
                    BinOp::Gt => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        let le = self.or(eq, lt);
                        self.not(le)
                    }
                    BinOp::Ge => {
                        let lt = if signed_compare {
                            self.lt_signed_net(&av, &bv)
                        } else {
                            self.lt_net(&av, &bv)
                        };
                        self.not(lt)
                    }
                    _ => unreachable!(),
                };
                Ok(vec![net])
            }
            BinOp::CaseEq | BinOp::CaseNe => {
                let w = wa.max(wb);
                let av = self.blast(a, w)?;
                let bv = self.blast(b, w)?;
                let one = self.const_net(LogicValue::One);
                let mut acc = one;
                for i in 0..w {
                    let xnor = self.combine(CellKind::Xnor, av[i], bv[i]);
                    acc = self.combine(CellKind::And, acc, xnor);
                }
                if op == BinOp::CaseNe {
                    Ok(vec![self.not(acc)])
                } else {
                    Ok(vec![acc])
                }
            }
            BinOp::LogAnd | BinOp::LogOr => {
                let av = self.blast(a, 1)?;
                let bv = self.blast(b, 1)?;
                let kind = if op == BinOp::LogAnd {
                    CellKind::And
                } else {
                    CellKind::Or
                };
                Ok(vec![self.combine(kind, av[0], bv[0])])
            }
        }
    }

    fn multiply_nets(&mut self, av: &[NetId], bv: &[NetId], w: usize) -> Vec<NetId> {
        let mut acc: Vec<NetId> = (0..w).map(|_| self.const_net(LogicValue::Zero)).collect();
        for (i, bbit) in bv.iter().enumerate() {
            if i >= w {
                break;
            }
            let mut partial: Vec<NetId> = Vec::with_capacity(w);
            for k in 0..w {
                if k >= i {
                    partial.push(self.combine(CellKind::And, av[k - i], *bbit));
                } else {
                    partial.push(self.const_net(LogicValue::Zero));
                }
            }
            let zero = self.const_net(LogicValue::Zero);
            let (sum, _) = self.adder(&acc, &partial, zero);
            acc = sum;
        }
        acc
    }

    pub(crate) fn not(&mut self, n: NetId) -> NetId {
        if let Some(c) = self.const_val(n) {
            self.const_net(c.not())
        } else {
            self.gate1("not", CellKind::Not, n)
        }
    }

    pub(crate) fn or(&mut self, a: NetId, b: NetId) -> NetId {
        self.combine(CellKind::Or, a, b)
    }

    pub(crate) fn eq_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let one = self.const_net(LogicValue::One);
        let mut acc = one;
        for i in 0..av.len() {
            let x = self.combine(CellKind::Xnor, av[i], bv[i]);
            acc = self.combine(CellKind::And, acc, x);
        }
        acc
    }

    pub(crate) fn lt_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let comp: Vec<NetId> = bv.iter().map(|n| self.not(*n)).collect();
        let one = self.const_net(LogicValue::One);
        let (_, carry) = self.adder(av, &comp, one);
        self.not(carry)
    }

    pub(crate) fn lt_signed_net(&mut self, av: &[NetId], bv: &[NetId]) -> NetId {
        let ax: Vec<NetId> = av
            .iter()
            .enumerate()
            .map(|(i, n)| if i + 1 == av.len() { self.not(*n) } else { *n })
            .collect();
        let bx: Vec<NetId> = bv
            .iter()
            .enumerate()
            .map(|(i, n)| if i + 1 == bv.len() { self.not(*n) } else { *n })
            .collect();
        self.lt_net(&ax, &bx)
    }

    pub(crate) fn adder(&mut self, av: &[NetId], bv: &[NetId], cin: NetId) -> (Vec<NetId>, NetId) {
        let w = av.len().max(bv.len());
        let mut sum = Vec::with_capacity(w);
        let mut carry = cin;
        for i in 0..w {
            let a = if i < av.len() {
                av[i]
            } else {
                self.const_net(LogicValue::Zero)
            };
            let b = if i < bv.len() {
                bv[i]
            } else {
                self.const_net(LogicValue::Zero)
            };
            let ax = self.combine(CellKind::Xor, a, b);
            let s = self.combine(CellKind::Xor, ax, carry);
            let ab = self.combine(CellKind::And, a, b);
            let xc = self.combine(CellKind::And, ax, carry);
            carry = self.or(ab, xc);
            sum.push(s);
        }
        (sum, carry)
    }

    // ---------- statements ----------
}
