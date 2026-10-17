use super::*;

fn parse_ok(src: &str) -> SourceFile {
    parse(src).unwrap_or_else(|e| panic!("parse failed: {}", e))
}

#[test]
fn simple_module() {
    let f = parse_ok("module m(input a, output y); assign y = ~a; endmodule");
    assert_eq!(f.modules.len(), 1);
    assert_eq!(f.modules[0].name, "m");
    assert_eq!(f.modules[0].items.len(), 3); // 2 ports + 1 assign
}

#[test]
fn systemverilog_keywords_cannot_be_used_as_identifiers() {
    assert!(parse("module repeat(output y); assign y = 1'b0; endmodule").is_err());
    assert!(parse("module m(output program); assign program = 1'b0; endmodule").is_err());
    assert!(parse("module m(output table); assign table = 1'b0; endmodule").is_err());
    assert!(parse("module m(output priority); assign priority = 1'b0; endmodule").is_err());
}

#[test]
fn non_ansi_ports() {
    let f = parse_ok("module m(a, b, y); input a, b; output y; assign y = a & b; endmodule");
    assert_eq!(f.modules[0].items.len(), 4);
}

#[test]
fn unpacked_array_ports() {
    let f = parse_ok("module m(input logic [7:0] a [0:1][2:3], output logic b [3:2]); endmodule");
    let ports: Vec<_> = f
        .modules
        .iter()
        .flat_map(|module| module.items.iter())
        .filter_map(|item| match item {
            Item::Port {
                names,
                array_ranges,
                ..
            } => Some((names, array_ranges)),
            _ => None,
        })
        .collect();
    assert_eq!(ports.len(), 2);
    assert_eq!(ports[0].0, &vec!["a".to_string()]);
    assert_eq!(ports[0].1.len(), 2);
    assert_eq!(ports[1].0, &vec!["b".to_string()]);
    assert_eq!(ports[1].1.len(), 1);
}

#[test]
fn always_and_initial() {
    let f = parse_ok(
        "module tb; reg clk; initial begin clk = 0; #5 clk = 1; #5 $display(\"clk=%b\", clk); $finish; end endmodule",
    );
    match &f.modules[0].items[1] {
        Item::Initial { body } => {
            assert!(matches!(body, Stmt::Block(_)));
        }
        other => panic!("expected initial, got {:?}", other),
    }
}

#[test]
fn precedence() {
    let f = parse_ok("module m(a,b,c,y); assign y = a | b & ~c; endmodule");
    if let Item::Assign { e, .. } = &f.modules[0].items[4] {
        // must be a | (b & (~c)) : top is Or
        assert!(matches!(e, Expr::Bin(BinOpSrc::Or, _, _)));
        let rhs = match e {
            Expr::Bin(_, _, r) => r,
            _ => unreachable!(),
        };
        assert!(matches!(**rhs, Expr::Bin(BinOpSrc::And, _, _)));
    } else {
        panic!("no assign");
    }
}

#[test]
fn indexed_part_selects() {
    let f = parse_ok(
        "module m(input [15:0] a, output [15:0] y); \
         assign y[6 +: 4] = a[11 -: 4]; endmodule",
    );
    match &f.modules[0].items[2] {
        Item::Assign { lv, .. } => {
            assert!(matches!(
                lv,
                Expr::IndexedPart {
                    ascending: true,
                    ..
                }
            ));
        }
        other => panic!("expected assignment, got {:?}", other),
    }
}

#[test]
fn unpacked_array_element_simple_range_selects() {
    let source = r#"
module m(input [1:0] i, output [3:0] y);
  logic [7:0] mem [0:3];
  assign y = mem[i][3:0];
endmodule
"#;
    let file = parse_ok(source);
    let assignment = file.modules[0]
        .items
        .iter()
        .find_map(|item| match item {
            Item::Assign { e, .. } => Some(e),
            _ => None,
        })
        .expect("range assignment");
    let Expr::ArrayPart {
        name,
        indexes,
        msb,
        lsb,
    } = assignment
    else {
        panic!("expected array-element range select, got {assignment:?}");
    };
    assert_eq!(name, "mem");
    assert_eq!(indexes.len(), 1);
    assert!(matches!(&**msb, Expr::Num { word, .. } if word.to_u64() == Some(3)));
    assert!(matches!(&**lsb, Expr::Num { word, .. } if word.to_u64() == Some(0)));
}

#[test]
fn net_type_keyword_can_combine_with_data_type() {
    let file = parse_ok(
        "module m(inout wire logic [7:0] p); wire logic [3:0] n; assign n = 4'b0; endmodule",
    );
    let port = file.modules[0]
        .items
        .iter()
        .find(|item| {
            matches!(item, Item::Port { names, .. }
                if names.iter().any(|name| name == "p"))
        })
        .expect("combined net-type port");
    assert!(matches!(port, Item::Port { is_reg: false, .. }));
    let net = file.modules[0]
        .items
        .iter()
        .find(|item| {
            matches!(item, Item::Net { names, .. }
                if names.iter().any(|(name, _)| name == "n"))
        })
        .expect("combined net-type declaration");
    assert!(matches!(net, Item::Net { is_reg: false, .. }));
}

#[test]
fn conditional_and_concat() {
    let f = parse_ok("module m(s,a,b,y); assign y = s ? {a, b[3:0]} : 4'b0; endmodule");
    if let Item::Assign { e, .. } = &f.modules[0].items[4] {
        assert!(matches!(e, Expr::Cond(..)));
    } else {
        panic!("no assign");
    }
}

#[test]
fn case_statement() {
    let f = parse_ok(
        "module m(sel, y); always @(*) case (sel) 2'b00, 2'b01: y = 1; default: y = 0; endcase endmodule",
    );
    match &f.modules[0].items[2] {
        Item::Always {
            body: Stmt::Case { arms, dflt, .. },
            ..
        } => {
            assert_eq!(arms.len(), 1);
            assert_eq!(arms[0].0.len(), 2); // two labels
            assert!(dflt.is_some());
        }
        other => panic!("expected case, got {:?}", other),
    }
}

#[test]
fn instance_with_params() {
    let f = parse_ok("module top; wire [3:0] s; adder #(4) u1(.a(s), .b(4'd1), .y()); endmodule");
    match &f.modules[0].items[1] {
        Item::Inst {
            module,
            name,
            array_ranges: _,
            params,
            conns,
        } => {
            assert_eq!(module, "adder");
            assert_eq!(name, "u1");
            assert_eq!(params.len(), 1);
            assert_eq!(conns.len(), 3);
            assert!(matches!(conns[2], Conn::Named(_, None)));
        }
        other => panic!("expected inst, got {:?}", other),
    }
}

#[test]
fn module_header_allows_multiple_parameter_groups() {
    let file = parse_ok(
        "module m #(parameter WIDTH = 4, parameter SIGNED = 0) (input [WIDTH-1:0] a, output y); assign y = a[0]; endmodule",
    );
    match &file.modules[0].items[0] {
        Item::Params(parameters) => assert_eq!(
            parameters
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["WIDTH", "SIGNED"]
        ),
        other => panic!("expected parameters, got {:?}", other),
    }
}

#[test]
fn modern_function_argument_lists_are_parsed() {
    let file = parse_ok(
        "module m; function automatic int add(int left, int right); add = left + right; endfunction endmodule",
    );
    match &file.modules[0].items[0] {
        Item::Function(function) => assert_eq!(
            function
                .args
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["left", "right"]
        ),
        other => panic!("expected function, got {:?}", other),
    }
}

#[test]
fn gate_primitives() {
    let f = parse_ok("module m(a,b,y); and g1(y, a, b); not (n1, a); endmodule");
    assert!(
        matches!(&f.modules[0].items[3], Item::Gate { kind: GateKind::And, name, .. } if name == "g1")
    );
    assert!(
        matches!(&f.modules[0].items[4], Item::Gate { kind: GateKind::Not, name, .. } if name.is_empty())
    );
}

#[test]
fn syntax_errors_reported() {
    assert!(parse("module m(").is_err());
    assert!(parse("module m begin endmodule").is_err());
    assert!(parse("module m; wire [3:0] w = ; endmodule").is_err());
    assert!(parse("module m; always @(*) y = ; endmodule").is_err());
    assert!(parse("module m; $display(\"%b %b\", a); endmodule").is_err()); // spec/arg count
}

#[test]
fn based_literals_ast() {
    let f = parse_ok("module m(y); assign y = 4'b10xz; endmodule");
    if let Item::Assign { e, .. } = &f.modules[0].items[1] {
        if let Expr::Num { width, word, .. } = e {
            assert_eq!(*width, Some(4));
            assert_eq!(word.to_bin_str(), "10xz");
        } else {
            panic!("not a number");
        }
    }
}

#[test]
fn question_based_digit_is_high_impedance() {
    let word = build_word(4, 2, "10??").unwrap();
    assert_eq!(word.to_bin_str(), "10zz");
}

#[test]
fn replicate() {
    let f = parse_ok("module m(y); assign y = {2{1'b1}}; endmodule");
    if let Item::Assign { e, .. } = &f.modules[0].items[1] {
        assert!(matches!(e, Expr::Replicate(..)));
    }
}
