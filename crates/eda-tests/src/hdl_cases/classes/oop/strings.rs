//! SystemVerilog OOP regression domain.

use super::shared::expect_net;
use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    string_cases(c);
}

fn string_cases(c: &mut CaseList) {
    // 1. string 变量赋值 + 与字面量比较（NUL 填充布局下相等语义）。
    c.add("sv_string_literal_compare", || {
        let source = r#"
module top(output ok1, ok2);
  string s;
  initial begin
    s = "hello";
    ok1 = (s == "hello");
    ok2 = (s != "world");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let ok1 = sim.read("ok1").and_then(|w| w.to_u64());
        let ok2 = sim.read("ok2").and_then(|w| w.to_u64());
        if ok1 != Some(1) || ok2 != Some(1) {
            return Err(format!("string compare ok1={ok1:?} ok2={ok2:?}"));
        }
        Ok(())
    });

    // 1b. 64 字节固定宽度字符串：满容量比较、格式化与 %s 打印。
    c.add("sv_string_64_byte_compare_and_format", || {
        let exact = "A".repeat(64);
        let source = format!(
            r#"
module top(output reg ok1, ok2);
  string s;
  initial begin
    s = "{exact}";
    ok1 = (s == "{exact}");
    s = $sformatf("uvm-frame-%s-%0d", "sequence", 42);
    ok2 = (s == "uvm-frame-sequence-42");
    $display("%s", s);
  end
endmodule
"#
        );
        let design = compile(&source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "64-byte string equality")?;
        expect_net(&sim, "ok2", 1, "formatted string equality")?;
        let text = sim.take_displays().join("\n");
        if !text.contains("uvm-frame-sequence-42") {
            return Err(format!("wide %s display missing: {text:?}"));
        }
        Ok(())
    });

    // 1c. string 内建 len/getc：支持变量索引、越界返回 0 与类属性路径。
    c.add("sv_string_len_getc_methods", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  string s;
  Named h;
  int index;
  initial begin
    s = "uvm_sequence_item";
    index = 4;
    y1 = s.len();          // 17
    y2 = s.getc(0);        // 'u' = 117
    y3 = s.getc(index);    // 's' = 115
    y4 = s.getc(99);       // 越界：0
    s = "";
    y5 = s.len();          // 0
    h = new();
    h.name = "driver0";
    y6 = h.name.len() + h.name.getc(1); // 7 + 'r' = 121
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 17, "string length")?;
        expect_net(&sim, "y2", 117, "first character")?;
        expect_net(&sim, "y3", 115, "runtime-indexed character")?;
        expect_net(&sim, "y4", 0, "out-of-range character")?;
        expect_net(&sim, "y5", 0, "empty string length")?;
        expect_net(&sim, "y6", 121, "class string property methods")?;
        Ok(())
    });

    // 1d. string 内建 putc/toupper/tolower：运行时写byte与整串大小写转换。
    c.add("sv_string_putc_and_case_methods", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output int y1, y2, y3, y4, output reg ok1, ok2, ok3, ok4, ok5);
  string s;
  string upper;
  string lower;
  Named h;
  int index;
  initial begin
    s = "Uvm-xeQ";
    index = 0;
    s.putc(index, 117);   // 'u'
    s.putc(5, 69);        // 'E'
    s.putc(99, 65);       // 越界：不改写
    s.putc(1, 0);         // NUL byte：按标准不改写
    y1 = s.getc(0);
    y2 = s.getc(5);
    y3 = s.len();
    upper = s.toupper();
    lower = s.tolower();
    ok1 = (upper == "UVM-XEQ");
    ok2 = (lower == "uvm-xeq");

    h = new();
    h.name = "Driver-0";
    h.name.putc(0, 100);  // 'd'
    y4 = h.name.getc(0);
    upper = h.name.toupper();
    ok3 = (upper == "DRIVER-0");
    ok4 = (h.name.len() == 8);
    ok5 = (s.len() == 7) && (s.getc(1) == 118) && (s.getc(6) == 81);
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 117, "putc runtime index")?;
        expect_net(&sim, "y2", 69, "putc constant index")?;
        expect_net(&sim, "y3", 7, "out-of-range putc preserves length")?;
        expect_net(&sim, "y4", 100, "class property putc")?;
        expect_net(&sim, "ok1", 1, "toupper")?;
        expect_net(&sim, "ok2", 1, "tolower")?;
        expect_net(&sim, "ok3", 1, "class property toupper")?;
        expect_net(&sim, "ok4", 1, "class property length after putc")?;
        expect_net(&sim, "ok5", 1, "NUL putc is a no-op")?;
        Ok(())
    });

    // 1e. string 内建 compare/icompare：按首差异 byte 给出 -1/0/1。
    c.add("sv_string_compare_icompare_methods", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  string a;
  string b;
  Named h;
  initial begin
    a = "ABC";
    b = "ABC";
    y1 = a.compare(b);       // 0
    b = "ABD";
    y2 = a.compare(b);       // -1
    y3 = b.compare(a);       // 1
    y4 = a.compare("ABC");   // 0
    a = "abc";
    b = "abd";
    y5 = a.compare(b);       // -1
    y6 = a.icompare("ABC");  // 0
    h = new("driver-1");
    y7 = h.name.compare("driver-2"); // -1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "equal strings")?;
        expect_net(&sim, "y2", u32::MAX as u64, "lexicographically less")?;
        expect_net(&sim, "y3", 1, "lexicographically greater")?;
        expect_net(&sim, "y4", 0, "literal compare")?;
        expect_net(&sim, "y5", u32::MAX as u64, "case-sensitive compare")?;
        expect_net(&sim, "y6", 0, "case-insensitive compare")?;
        expect_net(&sim, "y7", u32::MAX as u64, "class property compare")?;
        Ok(())
    });

    // 1f. string.substr：运行时左右边界、闭区间拷贝与非法区间清空。
    c.add("sv_string_substr_runtime_bounds", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5, ok6, ok7);
  string s;
  string sub;
  Named h;
  int left;
  int right;
  initial begin
    s = "uvm_driver_0";
    left = 4;
    right = 9;
    sub = s.substr(left, right);       // "driver"
    ok1 = (sub == "driver");
    sub = s.substr(0, 2);              // "uvm"
    ok2 = (sub == "uvm");
    sub = s.substr(10, 11);            // "_0"
    ok3 = (sub == "_0");
    sub = s.substr(right, left);       // left > right：空
    ok4 = (sub.len() == 0);
    sub = s.substr(-1, 3);             // 负左边界：空
    ok5 = (sub.len() == 0);
    sub = s.substr(0, 99);             // 右边界越过长度：空
    ok7 = (sub.len() == 0);

    h = new("scoreboard");
    sub = h.name.substr(5, 9);         // "board"
    ok6 = (sub == "board");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "runtime substring")?;
        expect_net(&sim, "ok2", 1, "prefix substring")?;
        expect_net(&sim, "ok3", 1, "suffix substring")?;
        expect_net(&sim, "ok4", 1, "reversed bounds empty")?;
        expect_net(&sim, "ok5", 1, "negative left bound empty")?;
        expect_net(&sim, "ok7", 1, "out-of-range right bound empty")?;
        expect_net(&sim, "ok6", 1, "class property substring")?;
        Ok(())
    });

    // 1g. string.atoi：十进制前缀、前导空白、正负号与非法输入。
    c.add("sv_string_atoi_decimal_prefix", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  string s;
  Named h;
  initial begin
    s = "42";
    y1 = s.atoi();             // 42
    s = "  -123uvm";
    y2 = s.atoi();             // -123
    s = "+7_items";
    y3 = s.atoi();             // 7
    s = "0043";
    y4 = s.atoi();             // 43
    s = "uvm42";
    y5 = s.atoi();             // 0
    s = "   ";
    y6 = s.atoi();             // 0
    h = new("123");
    y7 = h.name.atoi();        // 123
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 42, "decimal conversion")?;
        expect_net(
            &sim,
            "y2",
            (-123_i32) as u32 as u64,
            "signed negative conversion",
        )?;
        expect_net(&sim, "y3", 7, "explicit positive sign")?;
        expect_net(&sim, "y4", 43, "leading zeros")?;
        expect_net(&sim, "y5", 0, "invalid prefix")?;
        expect_net(&sim, "y6", 0, "whitespace-only input")?;
        expect_net(&sim, "y7", 123, "class property conversion")?;
        Ok(())
    });

    // 1h. string.atohex/atobin：十六进制与二进制前缀、符号和类属性。
    c.add("sv_string_atohex_atobin_radix", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  string s;
  Named h;
  initial begin
    s = "1A";
    y1 = s.atohex();          // 26
    s = " -2fUVM";
    y2 = s.atohex();          // -47
    s = "+7q";
    y3 = s.atohex();          // 7
    s = "uvm";
    y4 = s.atohex();          // 0
    h = new("DEAD");
    y5 = h.name.atohex();     // 57005

    s = "1011";
    y6 = s.atobin();          // 11
    s = " -1010uvm";
    y7 = s.atobin();          // -10
    s = "+011";
    y8 = s.atobin();          // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 26, "hex conversion")?;
        expect_net(
            &sim,
            "y2",
            (-47_i32) as u32 as u64,
            "signed negative hex conversion",
        )?;
        expect_net(&sim, "y3", 7, "positive hex sign")?;
        expect_net(&sim, "y4", 0, "invalid hex prefix")?;
        expect_net(&sim, "y5", 57_005, "class property hex")?;
        expect_net(&sim, "y6", 11, "binary conversion")?;
        expect_net(
            &sim,
            "y7",
            (-10_i32) as u32 as u64,
            "signed negative binary conversion",
        )?;
        expect_net(&sim, "y8", 3, "positive binary leading zero")?;
        Ok(())
    });

    // 1i. string.itoa：运行时整数到十进制 ASCII 字符串转换。
    c.add("sv_string_itoa_runtime_decimal", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5);
  string s;
  Named h;
  int value;
  initial begin
    value = 0;
    s.itoa(value);
    ok1 = (s == "0");

    value = 12345;
    s.itoa(value);
    ok2 = (s == "12345");

    value = -9876;
    s.itoa(value);
    ok3 = (s == "-9876");

    value = -2147483647;
    s.itoa(value);
    ok4 = (s == "-2147483647");

    h = new();
    h.name.itoa(7);
    ok5 = (h.name == "7");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "itoa zero")?;
        expect_net(&sim, "ok2", 1, "itoa positive")?;
        expect_net(&sim, "ok3", 1, "itoa negative")?;
        expect_net(&sim, "ok4", 1, "itoa large negative")?;
        expect_net(&sim, "ok5", 1, "class property itoa")?;
        Ok(())
    });

    // 1j. string.hextoa/bintoa：运行时整数到十六进制/二进制 ASCII。
    c.add("sv_string_hextoa_bintoa_runtime", || {
        let source = r#"
class Named;
  string name;
  function new();
  endfunction
endclass
module top(output reg ok1, ok2, ok3, ok4, ok5, ok6, ok7, ok8);
  string s;
  Named h;
  int value;
  initial begin
    value = 0;
    s.hextoa(value);
    ok1 = (s == "0");

    value = 26;
    s.hextoa(value);
    ok2 = (s == "1a");

    value = -1;
    s.hextoa(value);
    ok3 = (s == "ffffffff");

    h = new();
    h.name.hextoa(48879);
    ok4 = (h.name == "beef");

    value = 0;
    s.bintoa(value);
    ok5 = (s == "0");

    value = 5;
    s.bintoa(value);
    ok6 = (s == "101");

    value = -1;
    s.bintoa(value);
    ok7 = (s.len() == 32) && (s == "11111111111111111111111111111111");

    h.name.bintoa(6);
    ok8 = (h.name == "110");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "hextoa zero")?;
        expect_net(&sim, "ok2", 1, "hextoa positive")?;
        expect_net(&sim, "ok3", 1, "hextoa negative as 32-bit value")?;
        expect_net(&sim, "ok4", 1, "class property hextoa")?;
        expect_net(&sim, "ok5", 1, "bintoa zero")?;
        expect_net(&sim, "ok6", 1, "bintoa positive")?;
        expect_net(&sim, "ok7", 1, "bintoa negative as 32-bit value")?;
        expect_net(&sim, "ok8", 1, "class property bintoa")?;
        Ok(())
    });

    // 2. class string 属性 + 方法内比较（组合/方法路径）。
    c.add("sv_class_string_property", || {
        let source = r#"
class Named;
  string name;
  function new(string n);
    this.name = n;
  endfunction
  function int is_tb();
    is_tb = (this.name == "TB");
  endfunction
endclass
module top(output reg y);
  Named h;
  initial begin
    h = new("TB");
    y = h.is_tb();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let y = sim.read("y").and_then(|w| w.to_u64());
        if y != Some(1) {
            return Err(format!("class string compare y={y:?}"));
        }
        Ok(())
    });

    // 3. $uvm_info 打印带 tag/verbosity/格式化参数；%0d 修饰。
    c.add("sv_uvm_info_tagged_format", || {
        let source = r#"
module top;
  int count;
  initial begin
    count = 42;
    $uvm_info("TB", "count=%0d items, flag=%b", count, 1'b1, UVM_MEDIUM);
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let displays = sim.take_displays();
        let text = displays.join("\n");
        if !text.contains("[UVM_INFO][TB][UVM_MEDIUM] count=42 items, flag=1") {
            return Err(format!("uvm_info output missing: {text:?}"));
        }
        Ok(())
    });

    // 5. uvm_warning/error/fatal 家族 + %s 字符串参数打印。
    c.add("sv_uvm_severity_family_and_percent_s", || {
        let source = r#"
module top(output reg done);
  string tag;
  initial begin
    tag = "CHK";
    $uvm_warning("TB", "warn count=%0d", 5);
    $uvm_error("TB", "err tag=%s", tag);
    $uvm_fatal("TB", "fatal stop now");
    done = 1'b1; // fatal 后不应执行
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        let displays = sim.take_displays();
        let text = displays.join("\n");
        if !text.contains("[UVM_WARNING][TB] warn count=5") {
            return Err(format!("warning line missing: {text:?}"));
        }
        if !text.contains("[UVM_ERROR][TB] err tag=CHK") {
            return Err(format!("error line with %s missing: {text:?}"));
        }
        if !text.contains("[UVM_FATAL][TB] fatal stop now") {
            return Err(format!("fatal line missing: {text:?}"));
        }
        if !sim.finished() {
            return Err("$uvm_fatal must finish the simulation".to_string());
        }
        // fatal 后续语句不执行：done 保持 X。
        let done = sim.read("done").and_then(|w| w.to_u64());
        if done.is_some() {
            return Err(format!(
                "statement after $uvm_fatal executed: done={done:?}"
            ));
        }
        Ok(())
    });

    // 7. 表达式形式类方法的比较/算术体（回归：赋值按左值宽度补齐修复）。
    c.add("sv_class_expression_method_comparisons", || {
        let source = r#"
class box2 extends uvm_object;
  int v = 5;
  function new(int id);
    super.new(id);
  endfunction
  function int get_v();
    get_v = this.v;
  endfunction
  function int is_zero();
    is_zero = (this.v == 0);
  endfunction
  function int plus_two();
    plus_two = this.v + 2;
  endfunction
endclass
class holder extends uvm_component;
  box2 b;
  function new(int id);
    super.new(id);
    this.b = new(id * 10 + 1);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  box2 b;
  holder h;
  initial begin
    b = new(1);
    h = new(2);
    y1 = b.get_v();
    y2 = b.is_zero();
    y3 = h.b.get_v();
    y4 = h.b.is_zero();
    y5 = h.b.plus_two();
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 5, "top-level expression method")?;
        expect_net(
            &sim,
            "y2",
            0,
            "comparison expression method (v=5, not zero)",
        )?;
        expect_net(&sim, "y3", 5, "nested expression method")?;
        expect_net(&sim, "y4", 0, "nested comparison method")?;
        expect_net(&sim, "y5", 7, "nested arithmetic method")?;
        Ok(())
    });

    // 6. $sformatf 常量参数编译期格式化（%0d/%h/%b/%s/%%）。
    c.add("sv_sformatf_constant_formatting", || {
        let source = r#"
module top(output ok1, ok2, ok3);
  string a;
  string b;
  string c;
  initial begin
    a = $sformatf("n=%0d", 42);
    b = $sformatf("tag_%s", "TB");
    c = $sformatf("%b%%%0d", 1'b1, 7);
    ok1 = (a == "n=42");
    ok2 = (b == "tag_TB");
    ok3 = (c == "1%7");
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "ok1", 1, "sformatf numeric")?;
        expect_net(&sim, "ok2", 1, "sformatf string")?;
        expect_net(&sim, "ok3", 1, "sformatf binary and percent")?;
        Ok(())
    });

    // 4. 非法 verbosity / 超长字符串拒绝。
    c.add("sv_uvm_info_rejects_bad_verbosity", || {
        let result = compile(
            "module top; initial $uvm_info(\"TB\", \"x\", UVM_LOUD); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("bad verbosity accepted")?;
        if !error.contains("unknown $uvm_info verbosity") {
            return Err(format!("unexpected error: {error}"));
        }
        let oversized_literal = "x".repeat(65);
        let literal_source = format!(
            "module top; string s; initial s = \"{oversized_literal}\"; endmodule"
        );
        let result = compile(&literal_source, Some("top"));
        let error = result.err().ok_or("oversized string accepted")?;
        if !error.contains("limited to 64 bytes") {
            return Err(format!("unexpected error: {error}"));
        }
        let result = compile(
            "module top(input clk, output string s); always @(posedge clk) s = $sformatf(\"%0d\", clk); endmodule",
            Some("top"),
        );
        let error = result.err().ok_or("runtime sformatf accepted")?;
        if !error.contains("constant arguments") {
            return Err(format!("unexpected sformatf error: {error}"));
        }
        let oversized_result = "x".repeat(65);
        let sformat_source = format!(
            "module top; string s; initial s = $sformatf(\"{oversized_result}\"); endmodule"
        );
        let result = compile(&sformat_source, Some("top"));
        let error = result.err().ok_or("oversized sformatf accepted")?;
        if !error.contains("exceeds the 64-byte") {
            return Err(format!("unexpected sformatf error: {error}"));
        }
        Ok(())
    });
}
