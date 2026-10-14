//! Standard-cell library model: boolean function, area (in placement sites)
//! and timing arcs for every cell.

use crate::logic::LogicValue;
use std::collections::HashMap;

pub type LibCellId = usize;

/// Boolean function of a combinational library cell, over input pin indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibFunc {
    Const(LogicValue),
    Id(usize),
    Not(Box<LibFunc>),
    And(Vec<LibFunc>),
    Or(Vec<LibFunc>),
    Xor(Vec<LibFunc>),
    Nand(Vec<LibFunc>),
    Nor(Vec<LibFunc>),
    Xnor(Vec<LibFunc>),
    Mux {
        sel: Box<LibFunc>,
        a: Box<LibFunc>,
        b: Box<LibFunc>,
    },
}

impl LibFunc {
    pub fn eval(&self, ins: &[LogicValue]) -> LogicValue {
        match self {
            LibFunc::Const(v) => *v,
            LibFunc::Id(i) => ins.get(*i).copied().unwrap_or(LogicValue::X),
            LibFunc::Not(e) => e.eval(ins).not(),
            LibFunc::And(es) => es
                .iter()
                .map(|e| e.eval(ins))
                .reduce(|a, b| a.and(b))
                .unwrap_or(LogicValue::One),
            LibFunc::Or(es) => es
                .iter()
                .map(|e| e.eval(ins))
                .reduce(|a, b| a.or(b))
                .unwrap_or(LogicValue::Zero),
            LibFunc::Xor(es) => es
                .iter()
                .map(|e| e.eval(ins))
                .reduce(|a, b| a.xor(b))
                .unwrap_or(LogicValue::Zero),
            LibFunc::Nand(es) => LibFunc::And(es.clone()).eval(ins).not(),
            LibFunc::Nor(es) => LibFunc::Or(es.clone()).eval(ins).not(),
            LibFunc::Xnor(es) => LibFunc::Xor(es.clone()).eval(ins).not(),
            LibFunc::Mux { sel, a, b } => {
                let s = sel.eval(ins);
                match s.as_bool() {
                    Some(false) => a.eval(ins),
                    Some(true) => b.eval(ins),
                    None => a.eval(ins).resolve(b.eval(ins)),
                }
            }
        }
    }
}

/// Sequential timing metadata (for a single-clock, single-edge library).
/// Rise/fall constraint tables for one setup or hold check.
///
/// For sequential constraints, `index_1` is the constrained data-pin
/// transition and `index_2` is the related clock-pin transition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibConstraintTables {
    pub rise: Option<LibertyLut>,
    pub fall: Option<LibertyLut>,
}

impl LibConstraintTables {
    /// Evaluate the worst rise/fall constraint at the supplied transitions.
    pub fn evaluate_worst(
        &self,
        data_transition: f64,
        clock_transition: f64,
    ) -> Result<Option<f64>, String> {
        let rise = self
            .rise
            .as_ref()
            .map(|lut| lut.evaluate(data_transition, clock_transition))
            .transpose()?;
        let fall = self
            .fall
            .as_ref()
            .map(|lut| lut.evaluate(data_transition, clock_transition))
            .transpose()?;
        Ok(match (rise, fall) {
            (Some(rise), Some(fall)) => Some(rise.max(fall)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        })
    }
}

/// Sequential timing metadata (for a single-clock, single-edge library).
#[derive(Clone, Debug, Default)]
pub struct SeqTiming {
    pub clk_pin: usize,
    pub clk_to_q: f64,
    pub setup: f64,
    pub hold: f64,
    /// Load/slew-dependent setup constraints; the scalar is the fallback.
    pub setup_tables: Option<LibConstraintTables>,
    /// Load/slew-dependent hold constraints; the scalar is the fallback.
    pub hold_tables: Option<LibConstraintTables>,
    /// Transition-dependent recovery constraints; the scalar is the fallback.
    pub recovery_tables: Option<LibConstraintTables>,
    /// Transition-dependent removal constraints; the scalar is the fallback.
    pub removal_tables: Option<LibConstraintTables>,
    pub recovery: f64,
    pub removal: f64,
    pub min_pulse_high: f64,
    pub min_pulse_low: f64,
    pub reset_pin: Option<usize>,
}

/// A one- or two-dimensional Liberty lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct LibertyLut {
    pub index_1: Vec<f64>,
    pub index_2: Option<Vec<f64>>,
    pub values: Vec<f64>,
}

impl LibertyLut {
    /// Linearly interpolate with endpoint clamping.
    pub fn evaluate(&self, x: f64, y: f64) -> Result<f64, String> {
        if !x.is_finite() || !y.is_finite() {
            return Err("Liberty LUT coordinate must be finite".into());
        }
        let Some((i0, i1, fraction)) = interpolate_index(&self.index_1, x)? else {
            return Err("Liberty LUT has no index_1 values".into());
        };
        match &self.index_2 {
            None => {
                let lower = self.values[i0];
                let upper = self.values[i1];
                Ok(lower + (upper - lower) * fraction)
            }
            Some(index_2) => {
                let Some((j0, j1, y_fraction)) = interpolate_index(index_2, y)? else {
                    return Err("Liberty LUT has no index_2 values".into());
                };
                let width = index_2.len();
                let v00 = self.values[i0 * width + j0];
                let v01 = self.values[i0 * width + j1];
                let v10 = self.values[i1 * width + j0];
                let v11 = self.values[i1 * width + j1];
                let top = v00 + (v01 - v00) * y_fraction;
                let bottom = v10 + (v11 - v10) * y_fraction;
                Ok(top + (bottom - top) * fraction)
            }
        }
    }
}

fn interpolate_index(index: &[f64], value: f64) -> Result<Option<(usize, usize, f64)>, String> {
    if index.is_empty() {
        return Ok(None);
    }
    if index.iter().any(|value| !value.is_finite())
        || index.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err("Liberty LUT index must be finite and strictly increasing".into());
    }
    if value <= index[0] {
        return Ok(Some((0, 0, 0.0)));
    }
    if value >= index[index.len() - 1] {
        let last = index.len() - 1;
        return Ok(Some((last, last, 0.0)));
    }
    let position = index
        .windows(2)
        .position(|pair| value < pair[1])
        .ok_or("Liberty LUT interpolation interval not found")?;
    let lower = index[position];
    let upper = index[position + 1];
    Ok(Some((
        position,
        position + 1,
        (value - lower) / (upper - lower),
    )))
}

/// Indexed cell delay tables attached to one related input pin.
#[derive(Clone, Debug, PartialEq)]
pub struct LibTimingTables {
    pub input_pin: usize,
    pub rise: Option<LibertyLut>,
    pub fall: Option<LibertyLut>,
    pub rise_transition: Option<LibertyLut>,
    pub fall_transition: Option<LibertyLut>,
}

#[derive(Clone, Debug)]
pub struct LibCellDef {
    pub name: String,
    /// Input pin names, in port order.
    pub inputs: Vec<String>,
    pub output: String,
    pub func: LibFunc,
    /// Width in placement sites (1 site = 1 unit height row slot).
    pub width_sites: usize,
    /// (input pin index, delay to output) combinational arcs.
    pub arcs: Vec<(usize, f64)>,
    /// Indexed NLDM-style tables for combinational arcs.
    pub timing_tables: Vec<LibTimingTables>,
    pub seq: Option<SeqTiming>,
}

#[derive(Clone, Debug, Default)]
pub struct CellLibrary {
    pub name: String,
    pub cells: Vec<LibCellDef>,
    index: HashMap<String, LibCellId>,
    pub(crate) metadata: HashMap<String, crate::liberty::LibCellMetadata>,
}

impl CellLibrary {
    pub fn new(name: impl Into<String>) -> Self {
        CellLibrary {
            name: name.into(),
            cells: Vec::new(),
            index: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn add(&mut self, def: LibCellDef) -> LibCellId {
        let id = self.cells.len();
        self.index.insert(def.name.clone(), id);
        self.cells.push(def);
        id
    }

    pub fn get(&self, id: LibCellId) -> &LibCellDef {
        &self.cells[id]
    }

    pub fn find(&self, name: &str) -> Option<LibCellId> {
        self.index.get(name).copied()
    }

    /// Copy optional cell metadata when a definition is renamed.
    ///
    /// Instance-specific SDF annotation creates renamed cell definitions. The
    /// electrical and physical metadata must follow that stable new name so
    /// capacitance, transition limits, and power data remain associated.
    pub fn copy_metadata(&mut self, source_name: &str, target_name: &str) -> bool {
        if source_name == target_name {
            return true;
        }
        let Some(metadata) = self.metadata.get(source_name).cloned() else {
            return false;
        };
        self.metadata.insert(target_name.to_string(), metadata);
        true
    }

    /// Optional physical/electrical metadata imported from Liberty.
    pub fn metadata(&self, cell_name: &str) -> crate::liberty::LibCellMetadata {
        self.metadata.get(cell_name).cloned().unwrap_or_default()
    }

    pub fn eval(&self, id: LibCellId, ins: &[LogicValue]) -> LogicValue {
        let def = &self.cells[id];
        if def.seq.is_some() {
            // Next-state value for DFF-like cells: q_next = d.
            if def.name.starts_with("DFF") && ins.len() >= 2 {
                return ins[1];
            }
            return LogicValue::X;
        }
        def.func.eval(ins)
    }

    /// The default technology library used by synthesis and PnR.
    ///
    /// Delays are in nanoseconds, widths in placement sites.
    pub fn std() -> Self {
        let mut lib = CellLibrary::new("lib1");
        let add_comb = |lib: &mut CellLibrary,
                        name: &str,
                        ins: Vec<&str>,
                        out: &str,
                        func: LibFunc,
                        w: usize,
                        delay: f64| {
            let arcs: Vec<(usize, f64)> = (0..ins.len()).map(|i| (i, delay)).collect();
            lib.add(LibCellDef {
                name: name.into(),
                inputs: ins.into_iter().map(String::from).collect(),
                output: out.into(),
                func,
                width_sites: w,
                arcs,
                timing_tables: Vec::new(),
                seq: None,
            });
        };
        add_comb(
            &mut lib,
            "TIE0",
            vec![],
            "y",
            LibFunc::Const(LogicValue::Zero),
            1,
            0.0,
        );
        add_comb(
            &mut lib,
            "TIE1",
            vec![],
            "y",
            LibFunc::Const(LogicValue::One),
            1,
            0.0,
        );
        add_comb(
            &mut lib,
            "INV",
            vec!["a"],
            "y",
            LibFunc::Not(Box::new(LibFunc::Id(0))),
            1,
            0.4,
        );
        add_comb(&mut lib, "BUF", vec!["a"], "y", LibFunc::Id(0), 2, 0.8);
        add_comb(&mut lib, "DIODE", vec!["a"], "y", LibFunc::Id(0), 1, 0.05);
        add_comb(
            &mut lib,
            "AND2",
            vec!["a", "b"],
            "y",
            LibFunc::And(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            3,
            1.0,
        );
        add_comb(
            &mut lib,
            "OR2",
            vec!["a", "b"],
            "y",
            LibFunc::Or(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            3,
            1.1,
        );
        add_comb(
            &mut lib,
            "NAND2",
            vec!["a", "b"],
            "y",
            LibFunc::Nand(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            2,
            0.7,
        );
        add_comb(
            &mut lib,
            "NOR2",
            vec!["a", "b"],
            "y",
            LibFunc::Nor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            2,
            0.75,
        );
        add_comb(
            &mut lib,
            "XOR2",
            vec!["a", "b"],
            "y",
            LibFunc::Xor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            4,
            1.4,
        );
        add_comb(
            &mut lib,
            "XNOR2",
            vec!["a", "b"],
            "y",
            LibFunc::Xnor(vec![LibFunc::Id(0), LibFunc::Id(1)]),
            4,
            1.5,
        );
        add_comb(
            &mut lib,
            "MUX2",
            vec!["s", "a", "b"],
            "y",
            LibFunc::Mux {
                sel: Box::new(LibFunc::Id(0)),
                a: Box::new(LibFunc::Id(1)),
                b: Box::new(LibFunc::Id(2)),
            },
            4,
            1.3,
        );
        lib.add(LibCellDef {
            name: "DFF".into(),
            inputs: vec!["clk".into(), "d".into()],
            output: "q".into(),
            // combinational "function" of the flop is the identity on d
            func: LibFunc::Id(1),
            width_sites: 5,
            arcs: vec![(1, 1.2)], // clk->q handled via seq timing; d arc for STA convenience
            timing_tables: Vec::new(),
            seq: Some(SeqTiming {
                clk_pin: 0,
                clk_to_q: 1.2,
                setup: 0.6,
                hold: 0.2,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.25,
                min_pulse_low: 0.25,
                reset_pin: None,
            }),
        });
        lib.add(LibCellDef {
            name: "DFFR".into(),
            inputs: vec!["clk".into(), "d".into(), "rst".into()],
            output: "q".into(),
            func: LibFunc::Id(1),
            width_sites: 6,
            arcs: vec![(1, 1.2)],
            timing_tables: Vec::new(),
            seq: Some(SeqTiming {
                clk_pin: 0,
                clk_to_q: 1.2,
                setup: 0.6,
                hold: 0.2,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.3,
                removal: 0.15,
                min_pulse_high: 0.25,
                min_pulse_low: 0.25,
                reset_pin: Some(2),
            }),
        });
        lib
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_library_lookup_and_eval() {
        let lib = CellLibrary::std();
        let inv = lib.find("INV").unwrap();
        assert_eq!(lib.get(inv).width_sites, 1);
        assert_eq!(lib.eval(inv, &[LogicValue::One]), LogicValue::Zero);

        let nand = lib.find("NAND2").unwrap();
        assert_eq!(
            lib.eval(nand, &[LogicValue::One, LogicValue::One]),
            LogicValue::Zero
        );
        assert_eq!(
            lib.eval(nand, &[LogicValue::Zero, LogicValue::X]),
            LogicValue::One
        );

        let mux = lib.find("MUX2").unwrap();
        assert_eq!(
            lib.eval(mux, &[LogicValue::One, LogicValue::Zero, LogicValue::One]),
            LogicValue::One
        );

        let dff = lib.find("DFF").unwrap();
        assert!(lib.get(dff).seq.is_some());
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().setup, 0.6);
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().hold, 0.2);
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().min_pulse_high, 0.25);
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().min_pulse_low, 0.25);
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().recovery, 0.0);
        assert_eq!(lib.get(dff).seq.as_ref().unwrap().removal, 0.0);
        assert_eq!(
            lib.eval(dff, &[LogicValue::Zero, LogicValue::One]),
            LogicValue::One
        );
    }

    #[test]
    fn missing_cell_is_none() {
        let lib = CellLibrary::std();
        assert!(lib.find("NOT_A_CELL").is_none());
    }
}
