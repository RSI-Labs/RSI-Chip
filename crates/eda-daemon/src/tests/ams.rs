use super::*;

#[test]
fn rpc_spice_dc_returns_typed_operating_point() {
    let source = "rpc divider\nVin in 0 DC 1.5\nR1 in out 1k\nR2 out 0 2k\n.end";
    let request =
        RpcRequest::new(3, "spiceDc").with_params(Json::object([("source", Json::string(source))]));
    let response = dispatch(&request);
    let wire = response.wire();
    assert!(wire.contains("\"converged\":true"), "{wire}");
    assert!(wire.contains("\"out\":1"), "{wire}");

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        spice_dc(&request.params, &cancellation).unwrap_err(),
        "DC operating point RPC cancelled"
    );
}

#[test]
fn rpc_spice_run_dispatches_any_directive_result() {
    let source = "rpc analyses\nVin in 0 DC 1 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 1 159.155 159.155\n.tran 1u 1m\n.end";
    let request = RpcRequest::new(31, "spiceRun").with_params(Json::object([
        ("source", Json::string(source)),
        ("analysisIndex", Json::Number(0.0)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["kind"], Json::string("acSweep"));
    assert!(fields["report"].as_str().unwrap().contains("159.155"));

    let request = RpcRequest::new(32, "spiceRun").with_params(Json::object([
        ("source", Json::string(source)),
        ("all", Json::Bool(true)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(
        fields["kinds"],
        Json::Array(vec![Json::string("acSweep"), Json::string("transient")])
    );
    assert_eq!(fields["count"], Json::Number(2.0));
}

#[test]
fn rpc_spice_transient_returns_structured_thermal_state() {
    let source = "rpc thermal transient\nV1 d 0 1.8\nV2 g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 MEXP=1.5 RTH=100k CTH=1u)\n.tran 10m 30m method=trapezoidal\n.end";
    let request = RpcRequest::new(1, "spiceRun").with_params(Json::object([
        ("source", Json::string(source)),
        ("analysisIndex", Json::Number(0.0)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["kind"], Json::string("transient"));
    let result = fields["result"].as_object().unwrap();
    assert_eq!(result["method"], Json::string("trapezoidal"));
    assert_eq!(result["sampleCount"], Json::from(4_u64));
    let Json::Array(samples) = &result["samples"] else {
        panic!("transient RPC result should contain samples");
    };
    let first = samples[0].as_object().unwrap();
    let final_sample = samples
        .last()
        .and_then(|sample| sample.as_object())
        .unwrap();
    let first_temperatures = first["deviceTemperatures"].as_object().unwrap();
    let final_temperatures = final_sample["deviceTemperatures"].as_object().unwrap();
    assert!(
        first_temperatures["M1"].as_number().unwrap()
            < final_temperatures["M1"].as_number().unwrap(),
        "{first_temperatures:?} {final_temperatures:?}"
    );
    assert!(fields["report"].as_str().unwrap().contains("T(M1)="));
}

#[test]
fn rpc_spice_run_honors_cooperative_cancellation() {
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let source = "cancelled rpc transient\nVin in 0 PULSE(0 1 0 1p 1p 1n 2n)\nR1 in out 1k\nC1 out 0 1n\n.tran 1p 2n\n.end";
    let error = spice_run(
        &Json::object([("source", Json::string(source))]),
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(error, "transient analysis cancelled");
}
