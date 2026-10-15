use super::oop::expect_net;
use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    uvm_sequence_cases(c);
    uvm_port_cases(c);
    uvm_nested_port_cases(c);
}
fn uvm_sequence_cases(c: &mut CaseList) {
    // UVM-lite sequence item 流：virtual body → sequencer queue → driver pop。
    c.add("uvm_sequence_body_to_sequencer_items", || {
        let source = r#"
class int_sequence extends uvm_sequence;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void body();
    this.start_item(10);
    this.finish_item();
    this.start_item(20);
    this.finish_item();
  endfunction
endclass
class driver extends uvm_component;
  uvm_sequencer sqr;
  int last = 0;
  function new(int id, uvm_sequencer s);
    super.new(id);
    this.sqr = s;
  endfunction
  function void step();
    this.last = this.sqr.get_next_item();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  uvm_sequencer sqr;
  int_sequence seq;
  driver drv;
  uvm_sequence_item item;
  initial begin
    sqr = new(1);
    seq = new(2);
    drv = new(3, sqr);
    item = new(37, 9);

    seq.start(sqr);
    y1 = seq.items_started;    // 2
    y2 = seq.items_finished;   // 2
    y3 = sqr.generated;        // 2
    y4 = sqr.peek_next_item(); // 10
    drv.step();
    y5 = drv.last;             // 10
    drv.step();
    y6 = drv.last;             // 20
    y7 = sqr.delivered;        // 2
    y8 = sqr.items.count;      // 0
    y9 = item.get_data();      // 37
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 2, "sequence items started")?;
        expect_net(&sim, "y2", 2, "sequence items finished")?;
        expect_net(&sim, "y3", 2, "sequencer generated items")?;
        expect_net(&sim, "y4", 10, "first queued item")?;
        expect_net(&sim, "y5", 10, "driver first item")?;
        expect_net(&sim, "y6", 20, "driver second item")?;
        expect_net(&sim, "y7", 2, "sequencer delivered count")?;
        expect_net(&sim, "y8", 0, "sequencer queue drained")?;
        expect_net(&sim, "y9", 37, "sequence item payload")?;
        Ok(())
    });

    // UVM-lite seq_item_port：sequence→sequencer→driver handshake。
    c.add("uvm_driver_seq_item_port_handshake", || {
        let source = r#"
class burst_sequence extends uvm_sequence;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void body();
    this.start_item(11);
    this.finish_item();
    this.start_item(22);
    this.finish_item();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  uvm_sequencer sqr;
  burst_sequence seq;
  uvm_driver drv;
  initial begin
    sqr = new(1);
    seq = new(2);
    drv = new(3);

    drv.connect(sqr);
    seq.start(sqr);
    y1 = drv.seq_item_port.is_connected(); // 1
    y2 = drv.seq_item_port.peek_next_item(); // 11

    drv.step();
    y3 = drv.last_item;       // 11
    drv.step();
    y4 = drv.last_item;       // 22
    y5 = drv.seq_item_port.gets; // 2
    y6 = drv.seq_item_port.dones; // 2
    y7 = drv.transfers;       // 2
    y8 = sqr.items.count;     // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "seq_item_port connected")?;
        expect_net(&sim, "y2", 11, "port peek first item")?;
        expect_net(&sim, "y3", 11, "driver first transfer")?;
        expect_net(&sim, "y4", 22, "driver second transfer")?;
        expect_net(&sim, "y5", 2, "port gets")?;
        expect_net(&sim, "y6", 2, "port item_done calls")?;
        expect_net(&sim, "y7", 2, "driver transfers")?;
        expect_net(&sim, "y8", 0, "sequencer queue empty")?;
        Ok(())
    });
}

fn uvm_port_cases(c: &mut CaseList) {
    // uvm_analysis_port：connect(s) 句柄实参别名 + write 虚扇出。
    c.add("uvm_analysis_port_connect_and_write", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
module top(output int y1, y2, y3);
  uvm_analysis_port ap;
  sink s;
  initial begin
    s = new(2);
    ap = new(1);
    ap.connect(s);
    ap.write(4);
    ap.write(6);
    y1 = s.sum;
    y2 = s.hits;
    y3 = ap.writes;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "port subscriber sum")?;
        expect_net(&sim, "y2", 2, "port subscriber hits")?;
        expect_net(&sim, "y3", 2, "port write count")?;
        Ok(())
    });

    // uvm_analysis_port 多订阅者扇出：四个槽位 + 第五个连接满拒。
    c.add("uvm_analysis_port_multi_subscriber_fanout", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
    this.hits = this.hits + 1;
    this.observed = this.observed + 1;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7);
  uvm_analysis_port ap;
  sink a;
  sink b;
  sink c;
  sink d;
  sink extra;
  initial begin
    a = new(1);
    b = new(2);
    c = new(3);
    d = new(4);
    extra = new(5);
    ap = new(0);

    ap.connect(a);
    ap.connect1(b);
    ap.connect2(c);
    ap.connect3(d);
    ap.reject();        // 显式记录额外连接被拒绝
    ap.write4(3);
    ap.write4(7);
    ap.write4(5);

    y1 = a.sum;               // 15
    y2 = b.hits + c.hits + d.hits; // 9
    y3 = a.observed + b.observed + c.observed + d.observed; // 12
    y4 = extra.hits;          // 0
    y5 = ap.writes;           // 3
    y6 = ap.connects;         // 4
    y7 = ap.rejected_connects; // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 15, "first subscriber receives all writes")?;
        expect_net(&sim, "y2", 9, "other subscriber hit total")?;
        expect_net(&sim, "y3", 12, "all subscriber observations")?;
        expect_net(&sim, "y4", 0, "rejected subscriber not connected")?;
        expect_net(&sim, "y5", 3, "analysis port writes")?;
        expect_net(&sim, "y6", 4, "accepted analysis connections")?;
        expect_net(&sim, "y7", 1, "rejected analysis connection")?;
        Ok(())
    });
}

fn uvm_nested_port_cases(c: &mut CaseList) {
    // 嵌套路径实参：env 组合 port+sink，connect(e.s) 经路径解析绑定。
    c.add("uvm_analysis_port_nested_connect_actual", || {
        let source = r#"
class sink extends uvm_subscriber;
  int sum = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.sum = this.sum + data;
  endfunction
endclass
class env extends uvm_component;
  uvm_analysis_port ap;
  sink s;
  function new(int id);
    super.new(id);
    this.ap = new(id * 10 + 1);
    this.s = new(id * 10 + 2);
  endfunction
endclass
module top(output int y1, y2);
  env e;
  initial begin
    e = new(1);
    e.ap.connect(e.s);   // 嵌套路径实参
    e.ap.write(5);
    e.ap.write(9);
    y1 = e.s.sum;        // 14
    y2 = e.ap.writes;    // 2
  end
endmodule
"#;
        match compile(source, Some("top")) {
            Ok(design) => {
                let mut sim = Simulator::new(&design)?;
                sim.run_until(0)?;
                expect_net(&sim, "y1", 14, "nested connect sum")?;
                expect_net(&sim, "y2", 2, "nested port writes")?;
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    });
}
