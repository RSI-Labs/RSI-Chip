use crate::CaseList;
use eda_hdl::compile;
use eda_sim::Simulator;

use super::oop::expect_net;
pub(super) fn register(c: &mut CaseList) {
    uvm_fifo_cases(c);
    uvm_queue_cases(c);
    uvm_pool_cases(c);
    uvm_config_db_cases(c);
    uvm_factory_cases(c);
    uvm_objection_cases(c);
}
fn uvm_fifo_cases(c: &mut CaseList) {
    // TLM 深度 4 FIFO：满拒、FIFO 顺序取数与空读返回零。
    c.add("uvm_tlm_fifo_put_get_handshake", || {
        let source = r#"
class sequencer extends uvm_component;
  uvm_tlm_fifo fifo;
  int produced = 0;
  function new(int id, uvm_tlm_fifo f);
    super.new(id);
    this.fifo = f;
  endfunction
  function void send(int v);
    this.fifo.try_put(v);      // 语句调用：副作用写入 fifo 属性
    this.produced = this.fifo.puts;
  endfunction
endclass
class driver extends uvm_component;
  uvm_tlm_fifo fifo;
  int driven = 0;
  int total = 0;
  int last = 0;
  function new(int id, uvm_tlm_fifo f);
    super.new(id);
    this.fifo = f;
  endfunction
  function void step();
    this.last = this.fifo.try_get(); // 副作用方法可直接用于赋值表达式
    if (this.fifo.gets > this.driven) begin
      this.total = this.total + this.last;
      this.driven = this.driven + 1;
    end
  endfunction
endclass
module top(output int y1, y2, y3, y4, y5, y6, y7, y8);
  uvm_tlm_fifo fifo;
  sequencer sqr;
  driver drv;
  initial begin
    fifo = new(1);
    sqr = new(2, fifo);
    drv = new(3, fifo);
    sqr.send(10);           // q[0]
    sqr.send(20);           // q[1]
    sqr.send(30);           // q[2]
    sqr.send(40);           // q[3]
    sqr.send(50);           // 满拒
    y5 = fifo.count;        // 4
    y6 = fifo.puts;         // 4
    drv.step();             // 取 10
    y1 = drv.last;          // FIFO 首元素优先出队
    drv.step();             // 取 20
    y2 = drv.last;
    drv.step();             // 取 30
    y3 = drv.last;
    drv.step();             // 取 40
    y4 = drv.last;
    drv.step();             // 空读：peek 为 0，gets 不变
    y7 = drv.last;          // 0
    y8 = fifo.gets;         // 4
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "first FIFO element")?;
        expect_net(&sim, "y2", 20, "second FIFO element")?;
        expect_net(&sim, "y3", 30, "third FIFO element")?;
        expect_net(&sim, "y4", 40, "fourth FIFO element")?;
        expect_net(&sim, "y5", 4, "capacity after rejected put")?;
        expect_net(&sim, "y6", 4, "accepted puts")?;
        expect_net(&sim, "y7", 0, "empty peek")?;
        expect_net(&sim, "y8", 4, "successful fifo gets")?;
        Ok(())
    });
}

fn uvm_queue_cases(c: &mut CaseList) {
    // UVM-lite 深度 4 int 队列：两端 push/pop、满拒、空读与计数。
    c.add("uvm_queue_deque_operations", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_queue q;
  initial begin
    q = new(1);
    q.push_back(10);
    q.push_back(20);
    q.push_back(30);
    y1 = q.pop_front();   // 10
    q.push_front(5);      // 5,20,30
    y2 = q.pop_front();   // 5
    y3 = q.pop_back();    // 30
    y4 = q.pop_back();    // 20
    y5 = q.pop_back();    // 空读 0

    q.push_front(4);
    q.push_front(5);
    q.push_front(6);
    q.push_front(7);
    q.push_front(8);      // 满：不改变队列
    y6 = q.size();        // 4
    y7 = q.pop_front();   // 7
    y8 = q.pop_back();    // 4
    y9 = q.size();        // 2
    y10 = q.pushes;       // 8
    y11 = q.pops;         // 6
    q.delete();
    y12 = q.size();       // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 10, "initial FIFO head")?;
        expect_net(&sim, "y2", 5, "push-front becomes head")?;
        expect_net(&sim, "y3", 30, "pop-back tail")?;
        expect_net(&sim, "y4", 20, "remaining pop-back")?;
        expect_net(&sim, "y5", 0, "empty pop-back")?;
        expect_net(&sim, "y6", 4, "full after rejected push-front")?;
        expect_net(&sim, "y7", 7, "front push order newest")?;
        expect_net(&sim, "y8", 4, "full queue oldest tail")?;
        expect_net(&sim, "y9", 2, "size after two pops")?;
        expect_net(&sim, "y10", 8, "accepted pushes")?;
        expect_net(&sim, "y11", 6, "successful pops")?;
        expect_net(&sim, "y12", 0, "delete clears queue")?;
        Ok(())
    });
}

fn uvm_pool_cases(c: &mut CaseList) {
    // UVM-lite 深度 4 int->int pool：唯一键更新、满拒、删除与查找。
    c.add("uvm_pool_unique_key_add_find_remove", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  uvm_pool p;
  initial begin
    p = new(1);
    p.add(10, 100);
    p.add(20, 200);
    p.add(30, 300);
    p.add(40, 400);
    p.add(50, 500);       // 满拒
    p.add(20, 222);       // 已有键：更新，不增加 count
    y12 = p.num();         // 4
    y2 = p.find(20);       // 222
    y3 = p.exists(30);     // 1
    y4 = p.exists(50);     // 0
    y5 = p.find(40);       // 400

    p.remove(20);
    y6 = p.num();          // 3
    y7 = p.exists(20);     // 0
    y8 = p.find(40);       // 400
    y9 = p.adds;           // 4
    y10 = p.updates;       // 1
    y11 = p.removes;       // 1
    p.delete();
    y1 = p.num();          // 0
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "pool cleared")?;
        expect_net(&sim, "y2", 222, "existing key update")?;
        expect_net(&sim, "y3", 1, "existing key membership")?;
        expect_net(&sim, "y4", 0, "full-rejected key membership")?;
        expect_net(&sim, "y5", 400, "tail lookup before remove")?;
        expect_net(&sim, "y6", 3, "count after remove")?;
        expect_net(&sim, "y7", 0, "removed key membership")?;
        expect_net(&sim, "y8", 400, "tail preserved after shift")?;
        expect_net(&sim, "y9", 4, "accepted inserts")?;
        expect_net(&sim, "y10", 1, "key updates")?;
        expect_net(&sim, "y11", 1, "removals")?;
        expect_net(&sim, "y12", 4, "full pool count")?;
        Ok(())
    });
}

fn uvm_config_db_cases(c: &mut CaseList) {
    // UVM-lite 静态配置数据库：类名直接 set/get/exists/remove。
    c.add("uvm_config_db_static_string_key_entries", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11, y12);
  initial begin
    y1 = uvm_config_db.set("ADDR", 32);    // 1
    y2 = uvm_config_db.set("MODE", 1);     // 1
    y3 = uvm_config_db.set("DEPTH", 4);    // 1
    y4 = uvm_config_db.set("TIMEOUT", 100); // 1
    y5 = uvm_config_db.set("EXTRA", 9);    // 满拒：0
    y6 = uvm_config_db.set("MODE", 2);     // 已有键更新：1
    y7 = uvm_config_db.get("MODE");        // 2
    y8 = uvm_config_db.exists("DEPTH");    // 1
    y9 = uvm_config_db.remove("ADDR");     // 1
    y10 = uvm_config_db.exists("ADDR");    // 0
    y11 = uvm_config_db.get("DEPTH");      // 4
    y12 = uvm_config_db.count;             // 3
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "first config insert")?;
        expect_net(&sim, "y2", 1, "second config insert")?;
        expect_net(&sim, "y3", 1, "third config insert")?;
        expect_net(&sim, "y4", 1, "fourth config insert")?;
        expect_net(&sim, "y5", 0, "full config insert rejected")?;
        expect_net(&sim, "y6", 1, "existing key updated")?;
        expect_net(&sim, "y7", 2, "updated config value")?;
        expect_net(&sim, "y8", 1, "config key exists")?;
        expect_net(&sim, "y9", 1, "config key removed")?;
        expect_net(&sim, "y10", 0, "removed key absent")?;
        expect_net(&sim, "y11", 4, "later entry preserved after shift")?;
        expect_net(&sim, "y12", 3, "static config count")?;
        Ok(())
    });
}

fn uvm_factory_cases(c: &mut CaseList) {
    // UVM-lite factory：静态类型注册、type override 与解析策略。
    c.add("uvm_factory_type_override_resolution", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11);
  string resolved;
  initial begin
    y1 = uvm_factory.register("BASE");       // 1
    y2 = uvm_factory.register("DERIVED");    // 1
    y3 = uvm_factory.register("ITEM");       // 1
    y4 = uvm_factory.register("EXTRA");      // 1
    y5 = uvm_factory.register("TOOMANY");    // 满拒：0
    y6 = uvm_factory.register("BASE");       // 重复：0
    y7 = uvm_factory.set_type_override("BASE", "DERIVED"); // 1

    resolved = uvm_factory.get_override("BASE");
    y8 = (resolved == "DERIVED");             // 1
    y9 = uvm_factory.has_override("BASE");    // 1
    y10 = uvm_factory.is_registered("MISSING"); // 0
    y11 = uvm_factory.count;                   // 4
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "first factory registration")?;
        expect_net(&sim, "y2", 1, "second factory registration")?;
        expect_net(&sim, "y3", 1, "third factory registration")?;
        expect_net(&sim, "y4", 1, "fourth factory registration")?;
        expect_net(&sim, "y5", 0, "full factory registration rejected")?;
        expect_net(&sim, "y6", 0, "duplicate factory registration rejected")?;
        expect_net(&sim, "y7", 1, "type override installed")?;
        expect_net(&sim, "y8", 1, "override resolution")?;
        expect_net(&sim, "y9", 1, "override present")?;
        expect_net(&sim, "y10", 0, "unknown type absent")?;
        expect_net(&sim, "y11", 4, "factory type count")?;
        Ok(())
    });

    // instance override 优先于 type override；删除后回退全局策略。
    c.add("uvm_factory_instance_override_priority", || {
        let source = r#"
module top(output int y1, y2, y3, y4, y5, y6, y7, y8, y9, y10, y11);
  string resolved;
  initial begin
    y1 = uvm_factory.register("BASE");       // 1
    y2 = uvm_factory.register("DERIVED");    // 1
    y3 = uvm_factory.register("ITEM");       // 1
    y4 = uvm_factory.register("EXTRA");      // 1
    y5 = uvm_factory.set_type_override("BASE", "DERIVED"); // 1
    y6 = uvm_factory.set_inst_override(
           "env.agent.a", "BASE", "ITEM");                   // 1

    resolved = uvm_factory.get_inst_override("env.agent.a", "BASE");
    y7 = (resolved == "ITEM");                // instance override wins
    resolved = uvm_factory.get_inst_override("env.agent.b", "BASE");
    y8 = (resolved == "DERIVED");             // other instance uses type override
    y9 = uvm_factory.has_inst_override("env.agent.a", "BASE"); // 1

    y10 = uvm_factory.remove_inst_override("env.agent.a", "BASE"); // 1
    resolved = uvm_factory.get_inst_override("env.agent.a", "BASE");
    y11 = (resolved == "DERIVED");             // fallback after removal
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 1, "base registration")?;
        expect_net(&sim, "y2", 1, "derived registration")?;
        expect_net(&sim, "y3", 1, "item registration")?;
        expect_net(&sim, "y4", 1, "extra registration")?;
        expect_net(&sim, "y5", 1, "type override")?;
        expect_net(&sim, "y6", 1, "instance override")?;
        expect_net(&sim, "y7", 1, "instance override priority")?;
        expect_net(&sim, "y8", 1, "type override fallback")?;
        expect_net(&sim, "y9", 1, "instance override present")?;
        expect_net(&sim, "y10", 1, "instance override removed")?;
        expect_net(&sim, "y11", 1, "fallback after instance removal")?;
        Ok(())
    });
}

fn uvm_objection_cases(c: &mut CaseList) {
    // objection 计数：raise/drop 配对 + drained 终止条件。
    c.add("uvm_objection_raise_drop_drain", || {
        let source = r#"
class worker extends uvm_component;
  uvm_objection obj;
  int done = 0;
  function new(int id, uvm_objection o);
    super.new(id);
    this.obj = o;
  endfunction
  virtual function void run_phase();
    this.obj.raise(1);
    this.done = this.done + 1;
    this.obj.drop(1);
  endfunction
endclass
class env extends uvm_component;
  uvm_objection obj;
  worker w1;
  worker w2;
  function new(int id);
    super.new(id);
    this.obj = new(id * 10 + 1);
    this.w1 = new(id * 10 + 2, this.obj);
    this.w2 = new(id * 10 + 3, this.obj);
  endfunction
endclass
module top(output int y1, y2, y3, y4);
  env e;
  initial begin
    e = new(1);
    y1 = e.obj.raised;       // 0
    e.w1.run_phase();        // raise+drop
    e.w2.run_phase();
    e.obj.check_drained();
    y2 = e.obj.was_drained;  // 1
    y3 = e.obj.raise_calls;  // 2
    y4 = e.obj.drop_calls;   // 2
  end
endmodule
"#;
        let design = compile(source, Some("top")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design)?;
        sim.run_until(0)?;
        expect_net(&sim, "y1", 0, "initial raised zero")?;
        expect_net(&sim, "y2", 1, "drained after balanced raise/drop")?;
        expect_net(&sim, "y3", 2, "raise calls")?;
        expect_net(&sim, "y4", 2, "drop calls")?;
        Ok(())
    });
}
