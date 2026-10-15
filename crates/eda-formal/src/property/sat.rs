//! Formal property implementation module.

use crate::bmc::{encode_frame, flop_pins, shift_lit, BmcOptions};
use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::{Dir, NetId, Netlist};
use std::collections::HashMap;

pub(super) fn unroll(
    nl: &Netlist,
    opts: &BmcOptions,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<(crate::sat::Cnf, Vec<HashMap<NetId, i32>>), String> {
    let mut cnf = crate::sat::Cnf::new();
    let mut frames: Vec<HashMap<NetId, i32>> = Vec::new();
    let flops = flop_pins(nl)?;
    for t in 0..=depth {
        if cancellation.is_cancelled() {
            return Err("temporal property BMC cancelled".into());
        }
        let (frame_cnf, frame_lit) = encode_frame(nl)?;
        let off = cnf.n_vars as i32;
        while cnf.n_vars < off as usize + frame_cnf.n_vars {
            cnf.add_var();
        }
        for clause in &frame_cnf.clauses {
            cnf.add_clause(clause.iter().map(|lit| shift_lit(*lit, off)).collect());
        }
        let lit = frame_lit
            .iter()
            .map(|(net, value)| (*net, shift_lit(*value, off)))
            .collect::<HashMap<_, _>>();
        if t > 0 {
            let previous = &frames[t - 1];
            for flop in &flops {
                if cancellation.is_cancelled() {
                    return Err("temporal property BMC cancelled".into());
                }
                let q_now = lit[&flop.q];
                let d_previous = previous[&flop.d];
                match flop.reset {
                    Some((reset_previous, reset_high)) => {
                        let reset_active = if reset_high {
                            previous[&reset_previous]
                        } else {
                            -previous[&reset_previous]
                        };
                        cnf.add_clause(vec![-q_now, -reset_active]);
                        cnf.add_clause(vec![-q_now, reset_active, d_previous]);
                        cnf.add_clause(vec![q_now, reset_active, -d_previous]);
                    }
                    None => {
                        cnf.add_clause(vec![-q_now, d_previous]);
                        cnf.add_clause(vec![q_now, -d_previous]);
                    }
                }
                if let Some((reset_now, reset_high)) = flop.reset {
                    let active_now = if reset_high {
                        lit[&reset_now]
                    } else {
                        -lit[&reset_now]
                    };
                    cnf.add_clause(vec![-q_now, -active_now]);
                }
            }
        }
        if t == 0 && opts.init_state_zero {
            for flop in &flops {
                cnf.add_clause(vec![-lit[&flop.q]]);
            }
        }
        frames.push(lit);
    }
    Ok((cnf, frames))
}

pub(super) fn and_lits(cnf: &mut crate::sat::Cnf, lits: &[i32]) -> i32 {
    let output = cnf.add_var() as i32;
    for lit in lits {
        cnf.add_clause(vec![-output, *lit]);
    }
    cnf.add_clause({
        let mut clause = Vec::with_capacity(lits.len() + 1);
        clause.push(output);
        clause.extend(lits.iter().map(|lit| -lit));
        clause
    });
    output
}

pub(super) fn constant_lit(cnf: &mut crate::sat::Cnf, value: bool) -> i32 {
    let lit = cnf.add_var() as i32;
    cnf.add_clause(vec![if value { lit } else { -lit }]);
    lit
}

/// Encode `exactly k` of the input literals with a sequential unary counter.
#[allow(clippy::needless_range_loop)]
pub(super) fn exactly_true_lits(
    cnf: &mut crate::sat::Cnf,
    lits: &[i32],
    count: usize,
) -> Result<i32, String> {
    if count > lits.len() {
        return Ok(constant_lit(cnf, false));
    }
    let mut previous = vec![constant_lit(cnf, false); count + 1];
    previous[0] = constant_lit(cnf, true);
    for (index, input) in lits.iter().enumerate() {
        let processed = index + 1;
        let mut current = Vec::with_capacity(count + 1);
        for target in 0..=count.min(processed) {
            let mut sources = Vec::new();
            if target <= index {
                sources.push(and_lits(cnf, &[previous[target], -*input]));
            }
            if target > 0 && target - 1 <= index {
                sources.push(and_lits(cnf, &[previous[target - 1], *input]));
            }
            current.push(or_lits(cnf, &sources));
        }
        for target in count.min(processed) + 1..=count {
            current.push(previous[target]);
        }
        previous = current;
    }
    let result = previous[count];
    Ok(result)
}

pub(super) fn or_lits(cnf: &mut crate::sat::Cnf, lits: &[i32]) -> i32 {
    let output = cnf.add_var() as i32;
    let mut completion = Vec::with_capacity(lits.len() + 1);
    completion.push(-output);
    for lit in lits {
        cnf.add_clause(vec![output, -*lit]);
        completion.push(*lit);
    }
    cnf.add_clause(completion);
    output
}

pub(super) fn model_value(model: &[bool], lit: i32) -> bool {
    model.get(lit.unsigned_abs() as usize - 1).copied() == Some(lit > 0)
}

pub(super) fn input_trace(
    nl: &Netlist,
    frames: &[HashMap<NetId, i32>],
    model: &[bool],
) -> Vec<HashMap<String, u64>> {
    frames
        .iter()
        .map(|frame| {
            let mut values = HashMap::new();
            for port in &nl.ports {
                if port.dir != Dir::In {
                    continue;
                }
                let mut value = 0u64;
                for (bit, net) in port.bits.iter().enumerate() {
                    if model_value(model, frame[net]) {
                        value |= 1 << bit;
                    }
                }
                values.insert(port.name.clone(), value);
            }
            values
        })
        .collect()
}
