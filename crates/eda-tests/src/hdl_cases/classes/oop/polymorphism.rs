//! SystemVerilog OOP regression domain.

use super::shared::expect_net;
use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    class_polymorphism_cases(c);
}

fn class_polymorphism_cases(c: &mut CaseList) {
    // 1. 基类句柄 + 虚方法运行时分派（task 形式 + 基类字段动态绑定）。
    c.add("sv_class_virtual_dispatch_via_base_handle", || {
        let source = r#"
class Shape;
  int scale = 10;
  function new(int s);
    this.scale = s;
  endfunction
  virtual function void double_area();
    this.scale = this.scale * 2;
  endfunction
endclass
class Square extends Shape;
  int side;
  function new(int s, int sd);
    super.new(s);
    this.side = sd;
  endfunction
  virtual function void double_area();
    this.scale = this.scale + 1;
  endfunction
endclass
module top(output int y1, y2, y3);
  Shape b;
  Square q;
  initial begin
    b = new(5);
    b.double_area();      // 基类实现：5*2 = 10
    y1 = b.scale;
    q = new(7, 4);
    b = q;                // 引用别名：b 与 q 指向同一对象
    b.double_area();      // 派生 override：7+1 = 8
    y2 = b.scale;
    y3 = q.scale;         // 经原句柄观察同一存储：8
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "base dispatch")?;
        expect_net(&sim, "y2", 8, "derived dispatch via base handle")?;
        expect_net(
            &sim,
            "y3",
            8,
            "reference aliasing: write via b visible via q",
        )?;
        Ok(())
    });

    // 2. 表达式形式虚分派（virtual function int）。
    c.add("sv_class_virtual_dispatch_expression", || {
        let source = r#"
class Base;
  int v;
  function new(int x);
    this.v = x;
  endfunction
  virtual function int kind();
    kind = this.v;
  endfunction
endclass
class Derived extends Base;
  function new(int x);
    super.new(x);
  endfunction
  virtual function int kind();
    kind = this.v + 100;
  endfunction
endclass
module top(output int y1, y2);
  Base b;
  Derived d;
  initial begin
    b = new(3);
    y1 = b.kind();
    d = new(4);
    b = d;
    y2 = b.kind();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "base virtual expression")?;
        expect_net(&sim, "y2", 104, "derived virtual expression")?;
        Ok(())
    });

    // 3. 非虚方法保持静态绑定（对照）。
    c.add("sv_class_nonvirtual_stays_static", || {
        let source = r#"
class Base;
  virtual function int tag();
    tag = 1;
  endfunction
  function int plain();
    plain = 10;
  endfunction
endclass
class Derived extends Base;
  function new();
  endfunction
  function int plain();
    plain = 20;
  endfunction
endclass
module top(output int y);
  Base b;
  Derived d;
  initial begin
    d = new();
    b = d;
    y = b.plain(); // 非虚：按句柄静态类型解析 Base::plain = 10
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y", 10, "non-virtual static binding")?;
        Ok(())
    });
}
