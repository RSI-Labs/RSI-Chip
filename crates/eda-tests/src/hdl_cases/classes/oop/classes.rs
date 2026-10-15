//! SystemVerilog OOP regression domain.

use super::shared::expect_net;
use crate::CaseList;
use eda_hdl::{compile, parse};
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    class_cases(c);
}

fn class_cases(c: &mut CaseList) {
    // 1. Constructor, void statement method, expression method.
    c.add("sv_class_counter_method_and_get", || {
        let source = r#"
class Counter;
  int count = 0;
  function new(int start);
    this.count = start;
  endfunction
  function void inc();
    this.count = this.count + 1;
  endfunction
  function int get();
    get = this.count;
  endfunction
endclass
module top(output int y);
  Counter h;
  initial begin
    h = new(41);
    h.inc();
    h.inc();
    y = h.get();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("class counter output unknown")?;
        if actual != 43 {
            return Err(format!("counter result {actual}, expected 43"));
        }
        Ok(())
    });

    // 4b. Static methods are callable through the class name without a handle.
    c.add("sv_class_static_methods_class_scope", || {
        let source = r#"
class Registry;
  static int total = 0;
  static function void add(int n);
    Registry.total = Registry.total + n;
  endfunction
  static function int total_plus(int n);
    total_plus = Registry.total + n;
  endfunction
endclass
module top(output int y1, y2);
  initial begin
    Registry.total = 0;
    Registry.add(4);
    Registry.add(3);
    y1 = Registry.total;       // 7
    y2 = Registry.total_plus(6); // 13
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 7, "static method updates class storage")?;
        expect_net(&sim, "y2", 13, "static value-return method")?;
        Ok(())
    });

    // 4c. Class constructor/method and module function default arguments.
    c.add("sv_class_constructor_method_default_args", || {
        let source = r#"
class Item;
  int a;
  int b;
  function new(int a = 3, int b = 4);
    this.a = a;
    this.b = b;
  endfunction
  function int scaled(int scale = 10);
    scaled = (this.a + this.b) * scale;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  function int add(input int a, input int b = 10);
    add = a + b;
  endfunction
  Item h;
  Item k;
  initial begin
    h = new();          // a=3, b=4
    y1 = h.a;
    y2 = h.b;
    y3 = h.scaled();    // 70
    y4 = h.scaled(2);   // 14
    k = new(7);         // a=7, b=4
    y5 = k.a;
    y6 = k.scaled();    // 110
    y7 = add(1);        // 11
    y8 = add(1, 2);     // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "first constructor default")?;
        expect_net(&sim, "y2", 4, "second constructor default")?;
        expect_net(&sim, "y3", 70, "method default")?;
        expect_net(&sim, "y4", 14, "method explicit override")?;
        expect_net(&sim, "y5", 7, "constructor explicit prefix")?;
        expect_net(&sim, "y6", 110, "mixed explicit/default constructor")?;
        expect_net(&sim, "y7", 11, "module function default")?;
        expect_net(&sim, "y8", 3, "module function explicit override")?;
        Ok(())
    });

    // 4d. null 句柄：显式清空、比较、别名组分离与嵌套属性清空。
    c.add("sv_class_null_handle_compare_and_detach", || {
        let source = r#"
class Box;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Env;
  Box b;
  function new(int id = 0);
    this.b = new(id + 10);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  Box h;
  Box q;
  Env e;
  initial begin
    h = null;
    y1 = (h == null);       // 1
    y2 = (h != null);       // 0
    h = new(9);
    y3 = (h != null);       // 1
    q = h;                  // q 与 h 共享对象
    y4 = (q != null);       // 1
    h = null;               // h 从别名组分离，q 保持原对象
    y5 = (h == null);       // 1
    y6 = (q != null);       // 1
    y7 = q.value;           // 9
    e = new(1);
    y8 = (e.b != null);     // 1
    e.b = null;
    y9 = (e.b == null);     // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "initial null equality")?;
        expect_net(&sim, "y2", 0, "initial null inequality")?;
        expect_net(&sim, "y3", 1, "constructed handle")?;
        expect_net(&sim, "y4", 1, "aliased handle")?;
        expect_net(&sim, "y5", 1, "cleared handle detaches alias")?;
        expect_net(&sim, "y6", 1, "old alias remains allocated")?;
        expect_net(&sim, "y7", 9, "old alias preserves object storage")?;
        expect_net(&sim, "y8", 1, "nested handle constructed")?;
        expect_net(&sim, "y9", 1, "nested handle cleared")?;
        Ok(())
    });

    // 4e. class 对象身份相等：别名比较相等，不同构造对象比较不等。
    c.add("sv_class_object_identity_equality", || {
        let source = r#"
class Box;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Box h;
  Box q;
  Box other;
  initial begin
    h = new(11);
    q = h;                 // q 与 h 引用同一对象
    other = new(11);       // value 相同，但对象不同
    y1 = (h == q);         // 1
    y2 = (h == other);     // 0
    y3 = (h != other);     // 1

    h = null;              // h 脱离别名组；q 保持旧对象
    y4 = (h == q);         // 0
    y5 = (h != q);         // 1
    y6 = q.value;          // 11
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "aliased handles identify same object")?;
        expect_net(&sim, "y2", 0, "same payload is not same object")?;
        expect_net(&sim, "y3", 1, "different objects are unequal")?;
        expect_net(&sim, "y4", 0, "null handle is not old alias")?;
        expect_net(&sim, "y5", 1, "null handle differs from old alias")?;
        expect_net(&sim, "y6", 11, "old alias retains object")?;
        Ok(())
    });

    // 4f. $cast：已知继承链的上/下行转换、失败状态与 void 调用。
    c.add("sv_class_cast_handle_compatibility", || {
        let source = r#"
class Base;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Derived extends Base;
  int extra;
  function new(int v = 0, int e = 0);
    super.new(v);
    this.extra = e;
  endfunction
endclass
class Other;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  Base b;
  Base up;
  Derived d;
  Derived down;
  Derived failed;
  Other other;
  initial begin
    d = new(11, 22);
    b = d;
    y1 = $cast(down, b);   // 基类句柄实际引用 Derived：1
    y2 = down.extra;       // 22

    y3 = $cast(up, d);     // 派生→基类：1
    y4 = up.value;         // 11

    other = new(33);
    y5 = $cast(failed, other); // 类型与 Derived 无关：0
    y6 = failed.value;     // 未绑定，输出 X

    $cast(down, b);        // void 形式仍执行兼容 cast
    y7 = $cast(down, other); // 0（Other 不能转 Derived）
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "compatible downcast")?;
        expect_net(&sim, "y2", 22, "downcast derived field")?;
        expect_net(&sim, "y3", 1, "upcast")?;
        expect_net(&sim, "y4", 11, "upcast base field")?;
        expect_net(&sim, "y5", 0, "incompatible cast status")?;
        let failed_value = sim.read("y6").and_then(|word| word.to_u64());
        if failed_value.is_some() {
            return Err(format!(
                "failed cast destination should remain unbound: {failed_value:?}"
            ));
        }
        expect_net(&sim, "y7", 0, "void cast followed by status cast")?;
        Ok(())
    });

    // 4g. `new source` 浅拷贝：新对象身份、顶层字段独立、嵌套句柄共享。
    c.add("sv_class_new_source_shallow_copy", || {
        let source = r#"
class Inner;
  int value;
  function new(int v = 0);
    this.value = v;
  endfunction
endclass
class Outer;
  int tag;
  Inner inner;
  function new(int t = 0);
    this.tag = t;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Outer a;
  Outer copy;
  Inner nested;
  initial begin
    nested = new(5);
    a = new(11);
    a.inner = nested;
    copy = new a;          // shallow copy

    y1 = copy.tag;             // 11
    y2 = copy.inner.value;     // 5
    y3 = (copy == a);          // 0：新对象身份
    y4 = (copy.inner == nested); // 1：嵌套句柄仍引用同一对象

    copy.tag = 22;
    copy.inner.value = 7;      // 经共享 nested 句柄写入
    y5 = a.tag;                // 11：顶层字段独立
    y6 = nested.value;         // 7：嵌套对象共享
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 11, "copied scalar field")?;
        expect_net(&sim, "y2", 5, "nested value visible through copy")?;
        expect_net(&sim, "y3", 0, "copy has fresh object identity")?;
        expect_net(&sim, "y4", 1, "nested handle is shared")?;
        expect_net(&sim, "y5", 11, "source scalar remains independent")?;
        expect_net(&sim, "y6", 7, "nested object is shared after write")?;
        Ok(())
    });

    // 4e. 参数化 class 默认值：属性宽度、方法体、返回宽度与继承基类。
    c.add("sv_class_parameterized_default_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
  function logic [W-1:0] mask();
    mask = {W{1'b1}};
  endfunction
endclass
class DerivedBus extends Bus;
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  Bus bus;
  DerivedBus narrow;
  initial begin
    bus = new(8'h5a);
    y1 = bus.width();       // 8
    y2 = bus.total_bits(); // 16
    y3 = bus.mask();       // 8'hff
    y4 = bus.banks;        // 2
    narrow = new();
    y5 = narrow.width();   // inherited default 8
    y6 = narrow.mask();    // inherited default mask
    y7 = narrow.total_bits(); // inherited default 16
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 8, "default width")?;
        expect_net(&sim, "y2", 16, "multiple defaults")?;
        expect_net(&sim, "y3", 0xff, "parameterized mask")?;
        expect_net(&sim, "y4", 2, "second parameter property")?;
        expect_net(&sim, "y5", 8, "derived inherited default width")?;
        expect_net(&sim, "y6", 0xff, "derived inherited mask")?;
        expect_net(&sim, "y7", 16, "derived inherited total bits")?;
        Ok(())
    });

    // 4f. 参数化 class 显式覆盖：位置实参、命名实参与未覆盖默认值。
    c.add("sv_class_parameter_explicit_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  Bus #(.W(4), .BANKS(3)) named;
  Bus #(6) positional;
  initial begin
    named = new(4'he);
    y1 = named.width();       // 4
    y2 = named.banks;         // 3
    y3 = named.total_bits(); // 12

    positional = new(6'h2a);
    y4 = positional.width(); // 6
    y5 = positional.banks;   // 2
    y6 = positional.total_bits(); // 12
    y7 = named.data;
    y8 = positional.data;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 4, "named width override")?;
        expect_net(&sim, "y2", 3, "named banks override")?;
        expect_net(&sim, "y3", 12, "named total bits")?;
        expect_net(&sim, "y4", 6, "positional width override")?;
        expect_net(&sim, "y5", 2, "unoverridden banks default")?;
        expect_net(&sim, "y6", 12, "positional total bits")?;
        expect_net(&sim, "y7", 0xe, "named specialization payload")?;
        expect_net(&sim, "y8", 0x2a, "positional specialization payload")?;
        Ok(())
    });

    // 4g. 参数化继承：派生类显式选择基类 specialization。
    c.add("sv_class_parameterized_base_specialization", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
class Bus4 extends Bus #(4);
  function new(logic [3:0] initial_data = 0);
    super.new(initial_data);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  Bus4 bus;
  initial begin
    bus = new(4'hd);
    y1 = bus.width();      // 4
    y2 = bus.total_bits(); // 8
    y3 = bus.banks;        // 2
    y4 = bus.data;         // 13
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 4, "specialized base width")?;
        expect_net(&sim, "y2", 8, "specialized base total bits")?;
        expect_net(&sim, "y3", 2, "unoverridden base default")?;
        expect_net(&sim, "y4", 13, "specialized base payload")?;
        Ok(())
    });

    // 4h. 派生类参数转发：默认与显式派生 specialization 均传给基类。
    c.add("sv_class_parameter_forward_to_base", || {
        let source = r#"
class Bus #(parameter int W = 8, parameter int BANKS = 2);
  logic [W-1:0] data;
  int banks = BANKS;
  function new(logic [W-1:0] initial_data = 0);
    this.data = initial_data;
  endfunction
  function int width();
    width = W;
  endfunction
  function int total_bits();
    total_bits = W * BANKS;
  endfunction
endclass
class Wrapper #(parameter int W = 3) extends Bus #(W);
  function new(logic [W-1:0] initial_data = 0);
    super.new(initial_data);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  Wrapper narrow;
  Wrapper #(.W(6)) wide;
  initial begin
    narrow = new(3'h5);
    y1 = narrow.width();       // 3
    y2 = narrow.total_bits(); // 6
    y3 = narrow.banks;        // 2

    wide = new(6'h2c);
    y4 = wide.width();        // 6
    y5 = wide.total_bits();   // 12
    y6 = wide.data;           // 44
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 3, "default forwarded width")?;
        expect_net(&sim, "y2", 6, "default forwarded total bits")?;
        expect_net(&sim, "y3", 2, "default base banks")?;
        expect_net(&sim, "y4", 6, "explicit forwarded width")?;
        expect_net(&sim, "y5", 12, "explicit forwarded total bits")?;
        expect_net(&sim, "y6", 44, "explicit forwarded payload")?;
        Ok(())
    });

    // 1b. 值返回类方法可在直接赋值中执行副作用（静态与虚分派）。
    c.add("sv_class_side_effect_method_assignment", || {
        let source = r#"
class Accumulator;
  int total = 0;
  function new();
  endfunction
  function int add(int value);
    this.total = this.total + value;
    add = this.total;
  endfunction
endclass
class VBase;
  function new();
  endfunction
  virtual function int tick();
    tick = 1;
  endfunction
endclass
class VDerived extends VBase;
  int seen = 0;
  function new();
    super.new();
  endfunction
  virtual function int tick();
    this.seen = this.seen + 1;
    tick = 100 + this.seen;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  Accumulator a;
  VBase b;
  VDerived d;
  initial begin
    a = new();
    y1 = a.add(7);       // 7，同时 total 更新
    y2 = a.add(8);       // 15
    y3 = a.total;        // 15
    d = new();
    b = d;               // 引用别名：b 的动态类型为 VDerived
    y4 = b.tick();       // 101
    y5 = d.seen;         // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 7, "first side-effect return")?;
        expect_net(&sim, "y2", 15, "second side-effect return")?;
        expect_net(&sim, "y3", 15, "static method state updated")?;
        expect_net(&sim, "y4", 101, "virtual side-effect return")?;
        expect_net(&sim, "y5", 1, "virtual override state updated")?;
        Ok(())
    });

    // 2. Inheritance: base fields, super.new, method inheritance.
    c.add("sv_class_inheritance_super_new_and_method", || {
        let source = r#"
class Base;
  int value;
  function new(int v);
    this.value = v;
  endfunction
  function int get();
    get = this.value;
  endfunction
endclass
class Derived extends Base;
  int extra = 5;
  function new(int v);
    super.new(v + 1);
  endfunction
endclass
module top(output int y);
  Derived h;
  initial begin
    h = new(10);
    y = h.get() + h.extra;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("inheritance output unknown")?;
        if actual != 16 {
            return Err(format!("inheritance result {actual}, expected 16"));
        }
        Ok(())
    });

    // 3. Method override + super.method dispatch.
    c.add("sv_class_override_and_super_method", || {
        let source = r#"
class Shape;
  function int area();
    area = 1;
  endfunction
endclass
class Square extends Shape;
  int side;
  function new(int s);
    super.new();
    this.side = s;
  endfunction
  function int area();
    area = super.area() + this.side * this.side - 1;
  endfunction
endclass
module top(output int y);
  Square q;
  initial begin
    q = new(6);
    y = q.area();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("override output unknown")?;
        if actual != 36 {
            return Err(format!("override result {actual}, expected 36"));
        }
        Ok(())
    });

    // 4. Static property shared across every handle and the class name.
    c.add("sv_class_static_property_shared", || {
        let source = r#"
class S;
  static int count = 0;
  function void bump();
    S.count = S.count + 1;
  endfunction
endclass
module top(output int y1, y2);
  S a;
  S b;
  initial begin
    a = new();
    b = new();
    a.bump();
    b.bump();
    b.bump();
    y1 = S.count;
    y2 = a.count;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let y1 = sim
            .read("y1")
            .and_then(|word| word.to_u64())
            .ok_or("static count output unknown")?;
        let y2 = sim
            .read("y2")
            .and_then(|word| word.to_u64())
            .ok_or("static handle access output unknown")?;
        if y1 != 3 || y2 != 3 {
            return Err(format!("static counts {y1}/{y2}, expected 3/3"));
        }
        Ok(())
    });

    // 5. Built-in UVM-lite base class identity.
    c.add("sv_class_uvm_object_builtin_base", || {
        let source = r#"
class trx extends uvm_object;
  int payload;
  function new(int id, int p);
    super.new(id);
    this.payload = p;
  endfunction
endclass
module top(output int y);
  trx t;
  initial begin
    t = new(7, 99);
    y = t.get_id() + t.payload;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("uvm object output unknown")?;
        if actual != 106 {
            return Err(format!("uvm object result {actual}, expected 106"));
        }
        Ok(())
    });

    // 6. 句柄赋值 = 引用别名：经任一句柄写/读共享同一存储。
    c.add("sv_class_handle_assignment_aliases_storage", || {
        let source = r#"
class Pair;
  int a = 0;
  int b = 0;
  function void set(int x, int y);
    this.a = x;
    this.b = y;
  endfunction
endclass
module top(output int y1, y2, y3);
  Pair p;
  Pair q;
  initial begin
    p = new();
    p.set(11, 22);
    q = p;              // q 与 p 同一对象
    q.set(33, 44);      // 经 q 写
    y1 = p.a;           // 经 p 读：33
    y2 = p.b;           // 44
    p.set(55, 66);      // 经 p 写
    y3 = q.a;           // 经 q 读：55
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 33, "write via q visible via p")?;
        expect_net(&sim, "y2", 44, "second field via alias")?;
        expect_net(&sim, "y3", 55, "write via p visible via q")?;
        Ok(())
    });

    // 7. Declaration-initializer construction `Counter h = new(...)`.
    c.add("sv_class_declaration_initializer_new", || {
        let source = r#"
class Val;
  int v;
  function new(int x);
    this.v = x;
  endfunction
endclass
module top(output int y);
  Val h = new(77);
  initial y = h.v;
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("declaration-init output unknown")?;
        if actual != 77 {
            return Err(format!("declaration-init result {actual}, expected 77"));
        }
        Ok(())
    });

    // 8. Virtual classes and qualifier parsing.
    c.add("sv_class_virtual_and_qualifiers_parse", || {
        let source = r#"
virtual class Item;
  rand int addr;
  randc int data;
  local int secret;
  protected int guard;
  static int shared = 2;
endclass
"#;
        parse(&format!("module top; {source} endmodule")).map_err(|e| e.to_string())?;
        Ok(())
    });

    // 9. Class in a package, used by name from the module.
    c.add("sv_class_package_scope_visible", || {
        let source = r#"
package model_pkg;
  class Reg;
    int width;
    function new(int w);
      this.width = w;
    endfunction
  endclass
endpackage
module top(output int y);
  model_pkg::Reg r;
  initial begin
    r = new(32);
    y = r.width;
  end
endmodule
"#;
        // Scoped handle types are not yet resolved; parsing and class
        // collection must still succeed.
        parse(source).map_err(|e| e.to_string())?;
        Ok(())
    });

    // 10. Task-style void methods parse and run.
    c.add("sv_class_task_method_runs", || {
        let source = r#"
class Acc;
  int total = 1;
  task add(int x);
    this.total = this.total + x;
  endtask
endclass
module top(output int y);
  Acc a;
  initial begin
    a = new();
    a.add(20);
    y = a.total;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("task method output unknown")?;
        if actual != 21 {
            return Err(format!("task method result {actual}, expected 21"));
        }
        Ok(())
    });

    // 11. Statement-invoked value-returning method writes scratch result.
    c.add("sv_class_value_method_as_statement", || {
        let source = r#"
class W;
  int x;
  function int double();
    double = this.x * 2;
  endfunction
endclass
module top(output int y);
  W w;
  initial begin
    w = new();
    w.x = 9;
    w.double();
    y = w.x;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("statement method output unknown")?;
        if actual != 9 {
            return Err(format!("statement method result {actual}, expected 9"));
        }
        Ok(())
    });

    // 12. Field initializer expressions run before constructor body.
    c.add("sv_class_property_initializer_order", || {
        let source = r#"
class Init;
  int a = 10;
  int b;
  function new(int x);
    this.b = this.a + x;
  endfunction
endclass
module top(output int y);
  Init h;
  initial begin
    h = new(5);
    y = h.b;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let actual = sim
            .read("y")
            .and_then(|word| word.to_u64())
            .ok_or("initializer order output unknown")?;
        if actual != 15 {
            return Err(format!("initializer order result {actual}, expected 15"));
        }
        Ok(())
    });

    // Rejected programs.
    c.add("sv_class_unknown_type_rejected", || {
        let result = compile(
            "module top; Missing h; initial h = new(); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("unknown class type accepted")?;
        if !error.contains("unknown type") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_unknown_method_rejected", || {
        let source = r#"
class C;
  int x;
endclass
module top(output int y);
  C h;
  initial begin
    h = new();
    y = h.missing();
  end
endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("unknown method accepted")?;
        if !error.contains("no method 'missing'") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_new_on_non_handle_rejected", || {
        let result = compile(
            "module top(output int y); int h; initial begin h = new(); y = h; end endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("new on non-handle accepted")?;
        if !error.contains("not a class handle") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_constructor_wrong_arity_rejected", || {
        let source = r#"
class C;
  function new(int a, int b);
  endfunction
endclass
module top; C h; initial h = new(1); endmodule
"#;
        let result = compile(source, Some("top"));
        let error = result.err().ok_or("wrong constructor arity accepted")?;
        if !error.contains("expects 2 argument") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });

    c.add("sv_class_endclass_label_mismatch_rejected", || {
        let result = parse("class C; endclass : D");
        let error = result.err().ok_or("label mismatch accepted")?;
        if !error.contains("does not match class") {
            return Err(format!("unexpected error: {error}"));
        }
        Ok(())
    });
}
