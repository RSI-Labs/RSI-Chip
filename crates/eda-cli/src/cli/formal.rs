//! Formal expression parsing and bounded-proof reporting helpers.

pub(crate) fn parse_invariants(value: &str) -> Result<Vec<(String, bool)>, String> {
    let mut out = Vec::new();
    for item in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let invariant = match item.split_once('=') {
            Some((net, "0")) => (net.trim().to_string(), false),
            Some((net, "1")) => (net.trim().to_string(), true),
            Some(_) => {
                return Err(format!(
                    "invalid invariant '{}'; expected net=0 or net=1",
                    item
                ))
            }
            None => (item.to_string(), false),
        };
        if invariant.0.is_empty() {
            return Err(format!("invalid invariant '{}'", item));
        }
        out.push(invariant);
    }
    if out.is_empty() {
        Err("--assume requires at least one net or net=value".into())
    } else {
        Ok(out)
    }
}

pub(crate) fn parse_proof_expression(value: &str) -> Result<eda_api::BoolExpr, String> {
    let source = format!("assert property ({});", value);
    let assertions = eda_api::parse_sva(&source)?;
    let assertion = assertions
        .first()
        .ok_or_else(|| "no Boolean proof expression found".to_string())?;
    match &assertion.property {
        eda_api::Property::Immediate(expr) | eda_api::Property::Always(expr) => Ok(expr.clone()),
        eda_api::Property::Implication { .. } => {
            Err("temporal implications are not yet supported by expression proving".into())
        }
        eda_api::Property::RepeatedImplication { .. } => {
            Err("repeated temporal implications are not yet supported by expression proving".into())
        }
        eda_api::Property::NonconsecutiveImplication { .. } => Err(
            "nonconsecutive temporal implications are not supported by expression proving".into(),
        ),
        eda_api::Property::SequenceImplication { .. } => {
            Err("sequence antecedents are not supported by expression proving".into())
        }
    }
}

pub(crate) fn parse_named_proof_expression(value: &str) -> Result<eda_api::ProofInvariant, String> {
    let (name, expression) = value
        .split_once(':')
        .ok_or_else(|| "expected <name>:<boolean-expression>".to_string())?;
    let name = name.trim();
    if name.is_empty() {
        return Err("expression invariant requires a name".into());
    }
    Ok(eda_api::ProofInvariant::expression(
        name,
        parse_proof_expression(expression)?,
    ))
}

pub(crate) fn temporal_bmc_report(
    netlist: &eda_core::netlist::Netlist,
    source: &str,
    depth: usize,
    cex_path: Option<&str>,
) -> Result<String, String> {
    let properties = eda_api::parse_sva(source)?;
    let (assertions, assumptions): (Vec<_>, Vec<_>) = properties
        .iter()
        .cloned()
        .partition(|property| property.kind != eda_api::AssertionKind::Assume);
    if assertions.is_empty() {
        return Err("SVA input contains assumptions but no assert or cover targets".into());
    }
    let results = eda_api::check_assertions_bounded_with_assumptions(
        netlist,
        &assertions,
        &assumptions,
        depth,
    )?;
    if let Some(path) = cex_path {
        let Some(violation) = results.iter().find(|result| result.violated()) else {
            return Err("--cex requires a violating property".into());
        };
        eda_api::verify_property_counterexample(netlist, violation)?;
        let text = eda_api::export_property_counterexample_vcd(netlist, violation)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    let mut report = assumptions
        .iter()
        .map(|assumption| {
            format!(
                "Temporal BMC report '{}': ASSUMPTION CONSTRAINT applied\n",
                assumption.name
            )
        })
        .collect::<Vec<_>>()
        .join("");
    report.push_str(
        &results
            .iter()
            .map(eda_api::property_bmc_report)
            .collect::<Vec<_>>()
            .join(""),
    );
    Ok(report)
}

pub(crate) fn write_proof_cex(
    path: &str,
    netlist: &eda_core::netlist::Netlist,
    proof: &eda_api::ProofResult,
) -> Result<(), String> {
    let text = eda_api::export_proof_counterexample_vcd(netlist, proof)?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}
