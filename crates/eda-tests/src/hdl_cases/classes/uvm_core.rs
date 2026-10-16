use super::oop::expect_net;
use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

pub(super) fn register(c: &mut CaseList) {
    uvm_component_cases(c);
    uvm_channel_cases(c);
    uvm_structure_cases(c);
}
fn uvm_component_cases(c: &mut CaseList) {
    // 1. phase 钩子按 build→connect→run 顺序经虚分派执行。
    c.add("uvm_component_phase_order_virtual", || {
        let source = r#"
class tb_comp extends uvm_component;
  int trace = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void build_phase();
    this.trace = this.trace * 10 + 1;
  endfunction
  virtual function void connect_phase();
    this.trace = this.trace * 10 + 2;
  endfunction
  virtual function void run_phase();
    this.trace = this.trace * 10 + 3;
  endfunction
endclass
module top(output int y1, y2);
  uvm_component base;
  tb_comp t;
  initial begin
    t = new(1);
    base = t;                 // 基类句柄观察派生 override
    base.run_phases();
    y1 = t.trace;             // 123
    y2 = base.phase_state;    // 9（all common phases complete）
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 123, "phase order 1-2-3")?;
        expect_net(&sim, "y2", 9, "phase_state completion")?;
        Ok(())
    });

    // 1a. IEEE UVM common component phases execute in their canonical order.
    c.add("uvm_component_common_phase_order", || {
        let source = r#"
class phased_component extends uvm_component;
  int trace = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void build_phase();
    this.trace = this.trace * 10 + 1;
  endfunction
  virtual function void connect_phase();
    this.trace = this.trace * 10 + 2;
  endfunction
  virtual function void end_of_elaboration_phase();
    this.trace = this.trace * 10 + 3;
  endfunction
  virtual function void start_of_simulation_phase();
    this.trace = this.trace * 10 + 4;
  endfunction
  virtual function void run_phase();
    this.trace = this.trace * 10 + 5;
  endfunction
  virtual function void extract_phase();
    this.trace = this.trace * 10 + 6;
  endfunction
  virtual function void check_phase();
    this.trace = this.trace * 10 + 7;
  endfunction
  virtual function void report_phase();
    this.trace = this.trace * 10 + 8;
  endfunction
  virtual function void final_phase();
    this.trace = this.trace * 10 + 9;
  endfunction
endclass
module top(output int y1, y2);
  phased_component component;
  initial begin
    component = new(9);
    component.run_phases();
    y1 = component.trace;
    y2 = component.phase_state;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 123_456_789, "common phase order")?;
        expect_net(&sim, "y2", 9, "all common phases complete")?;
        Ok(())
    });

    // 1b. run_phase 自带 framework objection：raise→virtual body→drop→drain。
    c.add("uvm_component_run_phase_objection_lifecycle", || {
        let source = r#"
class worker extends uvm_component;
  int work = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void run_phase();
    this.objection.raise(2);
    this.work = this.work + 1;
    this.objection.drop(2);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5);
  worker w;
  initial begin
    w = new(7);
    w.run_phases();
    y1 = w.work;                    // 1
    y2 = w.objection.raised;        // 0：framework + user 均已平衡
    y3 = w.objection.raise_calls;   // 2
    y4 = w.objection.drop_calls;    // 2
    y5 = w.objection.was_drained;   // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "virtual run body executed")?;
        expect_net(&sim, "y2", 0, "objection count drained")?;
        expect_net(&sim, "y3", 2, "framework and user raises")?;
        expect_net(&sim, "y4", 2, "framework and user drops")?;
        expect_net(&sim, "y5", 1, "objection drain recorded")?;
        Ok(())
    });

    // 1c. 组件命名 / parent-child 层次 / 一层子组件 phase 驱动。
    c.add("uvm_component_named_child_hierarchy", || {
        let source = r#"
class leaf extends uvm_component;
  int hits = 0;
  function new(int id, string n = "");
    super.new(id, n);
  endfunction
  virtual function void build_phase();
    this.hits = this.hits + 1;
  endfunction
  virtual function void run_phase();
    this.hits = this.hits + 10;
  endfunction
endclass
class env extends uvm_component;
  leaf a;
  leaf b;
  function new(int id, string n);
    super.new(id, n);
    this.a = new(id * 10 + 1);
    this.b = new(id * 10 + 2);
    this.add_child0(this.a);
    this.add_child1(this.b);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  env e;
  initial begin
    e = new(1, "env");
    e.a.name = "agent_a";
    e.b.name = "agent_b";
    e.child0.run_phases();
    e.child1.run_phases();

    y1 = (e.get_name() == "env");
    y2 = e.a.hits;                  // 11
    y3 = e.child_count;            // 2
    y4 = (e.a.parent == e);        // parent 引用
    y5 = (e.child1 == e.b);        // child slot 引用
    y6 = e.b.hits;                  // 11
    y7 = e.a.phase_state;           // 9
    y8 = (e.a.parent == e);
    y9 = e.b.phase_state;           // 9
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "component name")?;
        expect_net(&sim, "y2", 11, "first child phases")?;
        expect_net(&sim, "y3", 2, "child count")?;
        expect_net(&sim, "y4", 1, "parent identity")?;
        expect_net(&sim, "y5", 1, "child slot identity")?;
        expect_net(&sim, "y6", 11, "second child phases")?;
        expect_net(&sim, "y7", 9, "first child phase completion")?;
        expect_net(&sim, "y8", 1, "parent identity after phases")?;
        expect_net(&sim, "y9", 9, "second child phase completion")?;
        Ok(())
    });

    // 2. 组合层次：env 持有子组件，嵌套路径调用 run_phases 派生钩子生效。
    c.add("uvm_component_hierarchy_nested_phases", || {
        let source = r#"
class agent extends uvm_component;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void build_phase();
    this.hits = this.hits + 1;
  endfunction
  virtual function void run_phase();
    this.hits = this.hits + 10;
  endfunction
endclass
class env extends uvm_component;
  agent a1;
  agent a2;
  function new(int id);
    super.new(id);
    this.a1 = new(id * 10 + 1);
    this.a2 = new(id * 10 + 2);
  endfunction
endclass
module top(output int y1, y2, y3);
  env e;
  initial begin
    e = new(1);
    e.a1.run_phases();        // 嵌套组件：build + run
    e.a2.run_phases();
    y1 = e.a1.hits;            // 11
    y2 = e.a2.hits;            // 11
    e.run_phases();           // env 自身（无 override → 空 + state=3）
    y3 = e.phase_state;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 11, "nested agent a1 build+run")?;
        expect_net(&sim, "y2", 11, "nested agent a2 build+run")?;
        expect_net(&sim, "y3", 9, "env phases complete")?;
        Ok(())
    });
}

fn uvm_structure_cases(c: &mut CaseList) {
    c.add("uvm_core_structural_class_surface", || {
        let source = r#"
class smoke_test extends uvm_test;
  int ran = 0;
  function new(int id, string name);
    super.new(id, name);
  endfunction
  virtual function void run_phase();
    this.report_info("smoke", "run phase", 0);
    this.ran = 1;
  endfunction
endclass
class value_subscriber extends uvm_subscriber;
  int observed_value = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.observed_value = data;
    this.observed = this.observed + 1;
  endfunction
endclass
class smoke_env extends uvm_env;
  uvm_analysis_export analysis_export;
  value_subscriber subscriber;
  function new(int id, string name);
    super.new(id, name);
    this.analysis_export = new(id + 1, name);
    this.subscriber = new(id + 2);
    this.analysis_export.connect(this.subscriber);
  endfunction
endclass
class smoke_transaction extends uvm_transaction;
  function new(int id, int transaction_id);
    super.new(id, transaction_id);
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  smoke_test test;
  smoke_env environment;
  smoke_transaction transaction;
  uvm_agent agent;
  uvm_monitor monitor;
  uvm_scoreboard scoreboard;
  initial begin
    test = new(1, "smoke_test");
    test.run_phases();
    environment = new(2, "smoke_env");
    environment.analysis_export.write(7);
    transaction = new(3, 41);
    agent = new(4, "agent");
    monitor = new(5, "monitor");
    scoreboard = new(6, "scoreboard");
    agent.run_phases();

    y1 = test.ran;
    y2 = test.report_info_count;
    y3 = test.phase_state;
    y4 = transaction.get_transaction_id();
    y5 = environment.subscriber.observed_value;
    y6 = environment.analysis_export.writes;
    y7 = environment.subscriber.observed;
    y8 = agent.phase_state;
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "uvm_test virtual run phase")?;
        expect_net(&sim, "y2", 1, "uvm_report_object info counter")?;
        expect_net(&sim, "y3", 9, "uvm_test phase completion")?;
        expect_net(&sim, "y4", 41, "uvm_transaction id")?;
        expect_net(&sim, "y5", 7, "analysis export forwards write")?;
        expect_net(&sim, "y6", 1, "analysis export write count")?;
        expect_net(&sim, "y7", 1, "analysis subscriber observation count")?;
        expect_net(&sim, "y8", 9, "structural component phase completion")?;
        Ok(())
    });
}

fn uvm_channel_cases(c: &mut CaseList) {
    // 分析风格通道：monitor→subscriber 虚 write 扇出（事务计数与载荷）。
    c.add("uvm_analysis_subscriber_fanout", || {
        let source = r#"
class monitor extends uvm_component;
  int value = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void observe(int v);
    this.value = v;
  endfunction
endclass
class collector extends uvm_subscriber;
  int total = 0;
  int hits = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    this.total = this.total + data;
    this.hits = this.hits + 1;
    this.observed = this.observed + 1;
  endfunction
endclass
class checker_component extends uvm_subscriber;
  int max = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
    if (data > this.max) this.max = data;
    this.observed = this.observed + 1;
  endfunction
endclass
class env extends uvm_component;
  monitor mon;
  collector col;
  checker_component chk;
  function new(int id);
    super.new(id);
    this.mon = new(id * 100 + 1);
    this.col = new(id * 100 + 2);
    this.chk = new(id * 100 + 3);
  endfunction
  function void burst(int a, int b, int c);
    this.col.write(a);
    this.chk.write(a);
    this.col.write(b);
    this.chk.write(b);
    this.col.write(c);
    this.chk.write(c);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  env e;
  initial begin
    e = new(1);
    e.burst(3, 7, 5);
    y1 = e.col.total;      // 15
    y2 = e.col.hits;       // 3
    y3 = e.chk.max;        // 7
    y4 = e.col.observed + e.chk.observed; // 6
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 15, "collector total")?;
        expect_net(&sim, "y2", 3, "collector hits")?;
        expect_net(&sim, "y3", 7, "checker max")?;
        expect_net(&sim, "y4", 6, "subscriber fanout observations")?;
        Ok(())
    });

    // UVM-lite callback pool：四个显式槽、virtual callback 扇出与计数。
    c.add("uvm_callback_pool_virtual_fanout", || {
        let source = r#"
class tracer extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event_code, int data);
    this.event_sum = this.event_sum + event_code;
    this.data_sum = this.data_sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
class scale extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event_code, int data);
    this.scaled = this.scaled + event_code * data;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6);
  uvm_callback_pool pool;
  tracer t0;
  tracer t1;
  scale s0;
  scale s1;
  initial begin
    pool = new(1);
    t0 = new(2);
    t1 = new(3);
    s0 = new(4);
    s1 = new(5);
    pool.add0(t0);
    pool.add1(t1);
    pool.add2(s0);
    pool.add3(s1);
    pool.call(7, 9);
    pool.call(2, 5);

    y1 = t0.event_sum + t1.event_sum; // 18
    y2 = t0.data_sum + t1.data_sum;   // 28
    y3 = t0.hits + t1.hits;           // 4
    y4 = s0.scaled + s1.scaled;      // 146
    y5 = pool.connects;               // 4
    y6 = pool.calls;                  // 2
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 18, "callback event sum")?;
        expect_net(&sim, "y2", 28, "callback data sum")?;
        expect_net(&sim, "y3", 4, "tracer callback hits")?;
        expect_net(&sim, "y4", 146, "scaled callback result")?;
        expect_net(&sim, "y5", 4, "callback connections")?;
        expect_net(&sim, "y6", 2, "callback pool calls")?;
        Ok(())
    });

    // callback enable/disable 过滤：禁用槽不执行但保留绑定。
    c.add("uvm_callback_pool_enable_filter", || {
        let source = r#"
class tracer extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event_code, int data);
    this.event_sum = this.event_sum + event_code;
    this.data_sum = this.data_sum + data;
    this.hits = this.hits + 1;
  endfunction
endclass
class scale extends uvm_callback;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void callback(int event_code, int data);
    this.scaled = this.scaled + event_code * data;
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9);
  uvm_callback_pool pool;
  tracer t0;
  tracer t1;
  scale s0;
  scale s1;
  initial begin
    pool = new(1);
    t0 = new(2);
    t1 = new(3);
    s0 = new(4);
    s1 = new(5);
    pool.add0(t0);
    pool.add1(t1);
    pool.add2(s0);
    pool.add3(s1);
    pool.call(7, 9);
    pool.disable1();
    pool.disable3();
    pool.call_filtered(2, 5);

    y1 = t0.event_sum;       // 9
    y2 = t1.event_sum;       // 7：禁用后不再执行
    y3 = t0.data_sum;        // 14
    y4 = t1.data_sum;        // 9
    y5 = s0.scaled;          // 73
    y6 = s1.scaled;          // 63
    y7 = pool.filtered_calls; // 1
    y8 = pool.skipped_callbacks; // 2
    y9 = pool.connects;      // 4：绑定仍保留
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 9, "enabled tracer event")?;
        expect_net(&sim, "y2", 7, "disabled tracer event frozen")?;
        expect_net(&sim, "y3", 14, "enabled tracer data")?;
        expect_net(&sim, "y4", 9, "disabled tracer data frozen")?;
        expect_net(&sim, "y5", 73, "enabled scale callback")?;
        expect_net(&sim, "y6", 63, "disabled scale callback frozen")?;
        expect_net(&sim, "y7", 1, "filtered call count")?;
        expect_net(&sim, "y8", 2, "skipped disabled slots")?;
        expect_net(&sim, "y9", 4, "connections retained")?;
        Ok(())
    });

    // UVM-lite event/barrier：共享句柄上的 pending trigger 消费与阈值到达。
    c.add("uvm_event_barrier_synchronization", || {
        let source = r#"
class producer extends uvm_component;
  uvm_event ev;
  function new(int id, uvm_event e);
    super.new(id);
    this.ev = e;
  endfunction
  function void fire();
    this.ev.trigger();
  endfunction
endclass
class consumer extends uvm_component;
  uvm_event ev;
  int got = 0;
  function new(int id, uvm_event e);
    super.new(id);
    this.ev = e;
  endfunction
  function void poll_step();
    this.got = this.ev.wait_trigger();
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_event ev;
  producer p;
  consumer c;
  uvm_barrier barrier;
  initial begin
    ev = new(1);
    p = new(2, ev);
    c = new(3, ev);

    p.fire();
    y1 = ev.is_on();       // 1：trigger pending
    c.poll_step();
    y2 = c.got;            // 1：consumer 消费 trigger
    c.poll_step();
    y3 = c.got;            // 0：pending 已被消费
    p.fire();
    y4 = ev.get_trigger_count(); // 2
    y5 = ev.wait_trigger();      // 1：顶层句柄消费第二次 trigger
    y6 = ev.wait_count;          // 2
    ev.reset();

    barrier = new(10, 3);
    y7 = barrier.wait_for(1);  // 0
    y8 = barrier.wait_for(1);  // 0
    y9 = barrier.wait_for(1);  // 1：到达阈值
    y10 = barrier.count;   // 3
    y11 = barrier.reached(); // 1
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "event pending after trigger")?;
        expect_net(&sim, "y2", 1, "consumer consumes event")?;
        expect_net(&sim, "y3", 0, "event not pending after consume")?;
        expect_net(&sim, "y4", 2, "event trigger count")?;
        expect_net(&sim, "y5", 1, "top handle consumes event")?;
        expect_net(&sim, "y6", 2, "event wait count")?;
        expect_net(&sim, "y7", 0, "barrier below threshold one")?;
        expect_net(&sim, "y8", 0, "barrier below threshold two")?;
        expect_net(&sim, "y9", 1, "barrier reaches threshold")?;
        expect_net(&sim, "y10", 3, "barrier count saturates")?;
        expect_net(&sim, "y11", 1, "barrier reached state")?;
        Ok(())
    });
}
