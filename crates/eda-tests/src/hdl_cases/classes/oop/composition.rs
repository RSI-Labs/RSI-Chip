//! SystemVerilog OOP regression domain.

use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    class_composition_cases(c);
}

fn class_composition_cases(c: &mut CaseList) {
    // 1. Nested class-typed properties: construct, call through paths.
    c.add("sv_class_nested_env_driver_composition", || {
        let source = r#"
class Driver;
  int addr = 0;
  function new(int a);
    this.addr = a;
  endfunction
  function void drive(int v);
    this.addr = this.addr + v;
  endfunction
  function int get();
    get = this.addr;
  endfunction
endclass
class Env;
  Driver drv;
  int bias = 100;
  function new(int a);
    this.drv = new(a);
  endfunction
  function void run(int v);
    this.drv.drive(v);
  endfunction
endclass
module top(output int y);
  Env e;
  initial begin
    e = new(7);
    e.run(5);
    e.drv.drive(10);
    y = e.drv.get() + e.bias;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("composition output unknown")?;
        if actual != 122 {
            return Err(format!("composition result {actual}, expected 122"));
        }
        Ok(())
    });

    // 2. Two-level nesting: top.mid.leaf field and method access.
    c.add("sv_class_two_level_nested_access", || {
        let source = r#"
class Leaf;
  int v = 3;
  function void add(int x);
    this.v = this.v + x;
  endfunction
endclass
class Mid;
  Leaf leaf;
  int scale = 10;
  function new();
    this.leaf = new();
  endfunction
endclass
class Top;
  Mid mid;
  function new();
    this.mid = new();
  endfunction
endclass
module top(output int y);
  Top t;
  initial begin
    t = new();
    t.mid.leaf.add(4);
    y = t.mid.leaf.v * t.mid.scale;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("two-level output unknown")?;
        if actual != 70 {
            return Err(format!("two-level result {actual}, expected 70"));
        }
        Ok(())
    });

    // 3. UVM component base class with overridden run_phase.
    c.add("sv_class_uvm_component_run_phase", || {
        let source = r#"
class my_comp extends uvm_component;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void run_phase();
    this.hits = this.hits + 1;
  endfunction
endclass
module top(output int y);
  my_comp c;
  initial begin
    c = new(3);
    c.run_phase();
    c.run_phase();
    y = c.get_id() * 100 + c.hits;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("uvm component output unknown")?;
        if actual != 302 {
            return Err(format!("uvm component result {actual}, expected 302"));
        }
        Ok(())
    });

    // 4. Inherited nested property access through a derived component.
    c.add("sv_class_nested_field_via_inheritance", || {
        let source = r#"
class Node;
  int base = 1;
endclass
class Big extends Node;
  int extra = 2;
endclass
class Holder;
  Big item;
  function new();
    this.item = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.item.base + h.item.extra;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("nested inheritance output unknown")?;
        if actual != 3 {
            return Err(format!("nested inheritance result {actual}, expected 3"));
        }
        Ok(())
    });

    // 5. Method through nested path returns value in expression.
    c.add("sv_class_nested_method_expression", || {
        let source = r#"
class Cell;
  int data;
  function new(int d);
    this.data = d;
  endfunction
  function int read();
    read = this.data;
  endfunction
endclass
class Bank;
  Cell cells0;
  function new(int d);
    this.cells0 = new(d);
  endfunction
endclass
module top(output int y);
  Bank b;
  initial begin
    b = new(44);
    y = b.cells0.read() + 1;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("nested method expression unknown")?;
        if actual != 45 {
            return Err(format!("nested method result {actual}, expected 45"));
        }
        Ok(())
    });

    // Negative: unknown method through nested path.
    c.add("sv_class_nested_unknown_method_rejected", || {
        let source = r#"
class Leaf;
  int v;
endclass
class Holder;
  Leaf leaf;
  function new();
    this.leaf = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.leaf.missing();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("nested unknown method accepted")?;
        if !error.contains("no method 'missing'") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    // Negative: selecting through a non-class property.
    c.add("sv_class_select_through_value_field_rejected", || {
        let source = r#"
class Leaf;
  int v;
endclass
class Holder;
  Leaf leaf;
  int plain;
  function new();
    this.leaf = new();
  endfunction
endclass
module top(output int y);
  Holder h;
  initial begin
    h = new();
    y = h.plain.deeper;
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("value-field chain accepted")?;
        if !error.contains("cannot select further") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    // Negative: new on a value field.
    c.add("sv_class_new_on_value_field_rejected", || {
        let source = r#"
class Holder;
  int plain;
endclass
module top;
  Holder h;
  initial begin
    h = new();
    h.plain = new();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("new on value field accepted")?;
        if !error.contains("new can only construct a class-typed property") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });
}
