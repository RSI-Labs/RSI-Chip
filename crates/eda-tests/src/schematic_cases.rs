//! Interactive schematic session event regressions.

use crate::CaseList;
use eda_api::{
    InstanceTransform, Point, SchematicBrowserService, SchematicDocument, SchematicEvent,
    SchematicSession, SchematicTool, Symbol, SymbolPin,
};

fn resistor_document() -> SchematicDocument {
    let mut document = SchematicDocument::new("interactive");
    document
        .add_symbol(Symbol {
            name: "res".into(),
            width: 20.0,
            height: 8.0,
            pins: vec![
                SymbolPin {
                    name: "A".into(),
                    position: Point::new(0.0, 4.0),
                },
                SymbolPin {
                    name: "B".into(),
                    position: Point::new(20.0, 4.0),
                },
            ],
        })
        .unwrap();
    document
        .add_instance("R1", "res", InstanceTransform::default())
        .unwrap();
    document
}

pub fn register(c: &mut CaseList) {
    for index in 0_i64..100 {
        c.add(
            format!("schematic_interactive_drag_{index:03}"),
            move || {
                let mut session = SchematicSession::new(resistor_document())?;
                session.process(SchematicEvent::PointerDown {
                    point: Point::new(10.0, 4.0),
                })?;
                let pointer_target = Point::new(index as f64, index as f64 + 1.0);
                let target = Point::new(pointer_target.x - 10.0, pointer_target.y - 4.0);
                session.process(SchematicEvent::PointerMove { point: target })?;
                session.process(SchematicEvent::PointerUp {
                    point: pointer_target,
                })?;
                let origin = session.document().instances["R1"].transform.origin;
                if origin != target {
                    Err(format!("drag {} produced {:?}", index, origin))
                } else {
                    Ok(())
                }
            },
        );
    }

    for index in 0_i64..100 {
        c.add(
            format!("schematic_interactive_wire_{index:03}"),
            move || {
                let mut session = SchematicSession::new(resistor_document())?;
                let offset = (index % 5) as f64 * 0.1;
                session.process(SchematicEvent::SetTool {
                    tool: SchematicTool::Wire,
                })?;
                session.process(SchematicEvent::PointerDown {
                    point: Point::new(offset, 4.0 + offset),
                })?;
                session.process(SchematicEvent::PointerUp {
                    point: Point::new(20.0 - offset, 4.0 - offset),
                })?;
                let wires: Vec<_> = session.document().wires.values().collect();
                if wires.len() != 1
                    || wires[0].start != Point::new(0.0, 4.0)
                    || wires[0].end != Point::new(20.0, 4.0)
                    || session.document().extract_nets()?.len() != 1
                {
                    Err(format!("wire snap {} failed: {:?}", index, wires))
                } else {
                    Ok(())
                }
            },
        );
    }

    for index in 0_i64..100 {
        c.add(format!("schematic_browser_wire_{index:03}"), move || {
            let service = SchematicBrowserService::new(resistor_document())?;
            let offset = (index % 4) as f64 * 0.1;
            let bodies = [
                "{\"type\":\"setTool\",\"tool\":\"wire\"}".to_string(),
                format!(
                    "{{\"type\":\"pointerDown\",\"point\":{{\"x\":{offset},\"y\":{}}}}}",
                    4.0 + offset
                ),
                format!(
                    "{{\"type\":\"pointerUp\",\"point\":{{\"x\":{},\"y\":{}}}}}",
                    20.0 - offset,
                    4.0 - offset
                ),
            ];
            for body in bodies {
                let request = format!(
                    "POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let response = service.handle_request(&request);
                if response.status != "200 OK" {
                    return Err(format!(
                        "browser event failed with {}: {}",
                        response.status,
                        String::from_utf8_lossy(&response.body)
                    ));
                }
            }
            Ok(())
        });
    }

    c.add("schematic_browser_property_grid", || {
        let service = SchematicBrowserService::new(resistor_document())?;
        let initial = service.handle_request("GET /properties HTTP/1.1\r\nHost: x\r\n\r\n");
        if !String::from_utf8_lossy(&initial.body).contains("\"selection\":\"none\"") {
            return Err(format!("{}", String::from_utf8_lossy(&initial.body)));
        }
        let body = r#"{"type":"pointerDown","point":{"x":10,"y":4}}"#;
        let request = format!(
            "POST /event HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let selected = service.handle_request(&request);
        let text = String::from_utf8_lossy(&selected.body).to_string();
        if selected.status != "200 OK"
            || !text.contains("\"selection\":\"instance\"")
            || !text.contains("\"name\":\"R1\"")
            || !text.contains("\"symbol\":\"res\"")
        {
            return Err(text);
        }
        Ok(())
    });
}
