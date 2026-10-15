//! Built-in UVM-lite classes and generic class specialization collection.

use super::const_eval;
use super::substitution::{subst_expr, subst_stmt};
use crate::ast;
use crate::parser;
use std::collections::HashMap;

/// Built-in UVM-lite base classes available without an explicit package
/// import. These provide the minimal object identity surface that derived
/// verification classes extend.
pub(super) const BUILTIN_UVM_CLASSES: &str = r#"
package __uvm_builtins;
virtual class uvm_object;
  protected int m_id = 0;
  function new(int id);
    this.m_id = id;
  endfunction
  function int get_id();
    get_id = this.m_id;
endfunction
endclass
virtual class uvm_component extends uvm_object(0);
  string name = "";
  int phase_state = 0;
  uvm_component parent;
  uvm_component child0;
  uvm_component child1;
  uvm_component child2;
  uvm_component child3;
  int child_count = 0;
  uvm_objection objection;
  function new(int id = 0, string name = "");
    super.new(id);
    this.name = name;
    this.objection = new(id + 1);
  endfunction
  virtual function void build_phase();
  endfunction
  virtual function void connect_phase();
  endfunction
  virtual function void run_phase();
  endfunction
  function void run_phases();
    this.build_phase();
    this.connect_phase();
    this.objection.raise(1);
    this.run_phase();
    this.objection.drop(1);
    this.objection.check_drained();
    this.phase_state = 3;
  endfunction
  function void set_name(string n);
    this.name = n;
  endfunction
  function string get_name();
    get_name = this.name;
  endfunction
  function void add_child0(uvm_component c);
    this.child0 = c;
    this.child0.parent = this;
    this.child_count = 1;
  endfunction
  function void add_child1(uvm_component c);
    this.child1 = c;
    this.child1.parent = this;
    this.child_count = 2;
  endfunction
  function void add_child2(uvm_component c);
    this.child2 = c;
    this.child2.parent = this;
    this.child_count = 3;
  endfunction
  function void add_child3(uvm_component c);
    this.child3 = c;
    this.child3.parent = this;
    this.child_count = 4;
  endfunction
endclass
virtual class uvm_subscriber extends uvm_component;
  int observed = 0;
  function new(int id);
    super.new(id);
  endfunction
  virtual function void write(int data);
  endfunction
endclass
virtual class uvm_callback extends uvm_object;
  int event_sum = 0;
  int data_sum = 0;
  int scaled = 0;
  int hits = 0;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void callback(int event, int data);
  endfunction
endclass
class uvm_callback_pool extends uvm_object;
  // 深度 4 的显式 callback 槽；call/call_filtered 要求四个槽都已绑定。
  uvm_callback cb0;
  uvm_callback cb1;
  uvm_callback cb2;
  uvm_callback cb3;
  int connects = 0;
  int calls = 0;
  int filtered_calls = 0;
  int skipped_callbacks = 0;
  int enabled0 = 1;
  int enabled1 = 1;
  int enabled2 = 1;
  int enabled3 = 1;
  function new(int id = 0);
    super.new(id);
  endfunction
  function void add0(uvm_callback cb);
    this.cb0 = cb;
    this.connects = 1;
  endfunction
  function void add1(uvm_callback cb);
    this.cb1 = cb;
    this.connects = 2;
  endfunction
  function void add2(uvm_callback cb);
    this.cb2 = cb;
    this.connects = 3;
  endfunction
  function void add3(uvm_callback cb);
    this.cb3 = cb;
    this.connects = 4;
  endfunction
  function void call(int event, int data);
    this.cb0.callback(event, data);
    this.cb1.callback(event, data);
    this.cb2.callback(event, data);
    this.cb3.callback(event, data);
    this.calls = this.calls + 1;
  endfunction
  function void enable0();
    this.enabled0 = 1;
  endfunction
  function void disable0();
    this.enabled0 = 0;
  endfunction
  function void enable1();
    this.enabled1 = 1;
  endfunction
  function void disable1();
    this.enabled1 = 0;
  endfunction
  function void enable2();
    this.enabled2 = 1;
  endfunction
  function void disable2();
    this.enabled2 = 0;
  endfunction
  function void enable3();
    this.enabled3 = 1;
  endfunction
  function void disable3();
    this.enabled3 = 0;
  endfunction
  function void call_filtered(int event, int data);
    if (this.enabled0)
      this.cb0.callback(event, data);
    else
      this.skipped_callbacks = this.skipped_callbacks + 1;
    if (this.enabled1)
      this.cb1.callback(event, data);
    else
      this.skipped_callbacks = this.skipped_callbacks + 1;
    if (this.enabled2)
      this.cb2.callback(event, data);
    else
      this.skipped_callbacks = this.skipped_callbacks + 1;
    if (this.enabled3)
      this.cb3.callback(event, data);
    else
      this.skipped_callbacks = this.skipped_callbacks + 1;
    this.filtered_calls = this.filtered_calls + 1;
  endfunction
endclass
class uvm_sequence_item extends uvm_object;
  int data = 0;
  function new(int data = 0, int id = 0);
    super.new(id);
    this.data = data;
  endfunction
  function int get_data();
    get_data = this.data;
  endfunction
  function void set_data(int value);
    this.data = value;
  endfunction
endclass
class uvm_sequencer extends uvm_component;
  uvm_queue items;
  int generated = 0;
  int delivered = 0;
  function new(int id = 0);
    super.new(id);
    this.items = new(id + 1);
  endfunction
  function void send_item(int value);
    this.items.push_back(value);
    this.generated = this.generated + 1;
  endfunction
  function int get_next_item();
    get_next_item = this.items.peek_front();
    this.items.shift_front();
    if (this.items.count == 0)
      this.delivered = this.generated;
    else
      this.delivered = this.delivered + 1;
  endfunction
  function int peek_next_item();
    peek_next_item = this.items.q0;
  endfunction
endclass
class uvm_sequence extends uvm_object;
  uvm_sequencer sequencer;
  int current_item = 0;
  int items_started = 0;
  int items_finished = 0;
  int is_started = 0;
  int is_done = 0;
  function new(int id = 0);
    super.new(id);
  endfunction
  virtual function void body();
  endfunction
  function void start(uvm_sequencer sqr);
    this.sequencer = sqr;
    this.is_started = 1;
    this.is_done = 0;
    this.body();
    this.is_started = 0;
    this.is_done = 1;
  endfunction
  function void start_item(int value);
    this.current_item = value;
    this.items_started = this.items_started + 1;
  endfunction
  function void finish_item();
    this.sequencer.send_item(this.current_item);
    this.items_finished = this.items_finished + 1;
  endfunction
endclass
class uvm_seq_item_port extends uvm_object;
  uvm_sequencer sqr;
  int connects = 0;
  int gets = 0;
  int dones = 0;
  function new(int id = 0);
    super.new(id);
  endfunction
  function void connect(uvm_sequencer s);
    this.sqr = s;
    this.connects = this.connects + 1;
  endfunction
  function int is_connected();
    is_connected = (this.connects != 0);
  endfunction
  function int get_next_item();
    get_next_item = this.sqr.peek_next_item();
    this.gets = this.gets + 1;
  endfunction
  function int peek_next_item();
    peek_next_item = this.sqr.peek_next_item();
  endfunction
  function void item_done();
    this.sqr.get_next_item();
    this.dones = this.dones + 1;
  endfunction
endclass
class uvm_driver extends uvm_component;
  uvm_seq_item_port seq_item_port;
  uvm_sequencer sequencer;
  int connect_calls = 0;
  int last_item = 0;
  int transfers = 0;
  function new(int id = 0);
    super.new(id);
    this.seq_item_port = new(id + 1);
  endfunction
  function void connect(uvm_sequencer s);
    this.sequencer = s;
    this.seq_item_port.connect(this.sequencer);
    this.connect_calls = this.connect_calls + 1;
  endfunction
  function void step();
    this.last_item = this.seq_item_port.get_next_item();
    this.seq_item_port.item_done();
    this.transfers = this.transfers + 1;
  endfunction
endclass
class uvm_objection extends uvm_object;
  int raised = 0;
  int raise_calls = 0;
  int drop_calls = 0;
  int was_drained = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void raise(int n);
    this.raised = this.raised + n;
    this.raise_calls = this.raise_calls + 1;
  endfunction
  function void drop(int n);
    this.raised = this.raised - n;
    this.drop_calls = this.drop_calls + 1;
  endfunction
  function int drained();
    drained = (this.raised == 0);
  endfunction
  function void check_drained();
    this.was_drained = 1;
    if (this.raised != 0)
      this.was_drained = 0;
  endfunction
endclass
class uvm_event extends uvm_object;
  // UVM-lite sequential event: trigger makes it pending and wait_trigger
  // consumes one pending observation. It does not suspend a process.
  int state = 0;
  int trigger_count = 0;
  int wait_count = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void reset();
    this.state = 0;
  endfunction
  function void trigger();
    this.state = 1;
    this.trigger_count = this.trigger_count + 1;
  endfunction
  function int wait_trigger();
    if (this.state != 0) begin
      this.state = 0;
      this.wait_count = this.wait_count + 1;
      wait_trigger = 1;
    end else begin
      wait_trigger = 0;
    end
  endfunction
  function int is_on();
    is_on = (this.state != 0);
  endfunction
  function int get_trigger_count();
    get_trigger_count = this.trigger_count;
  endfunction
endclass
class uvm_barrier extends uvm_object;
  int threshold = 0;
  int count = 0;
  int waits = 0;
  int was_reached = 0;
  function new(int id, int threshold);
    super.new(id);
    this.threshold = threshold;
  endfunction
  function void set_threshold(int threshold);
    this.threshold = threshold;
    this.count = 0;
    this.was_reached = 0;
  endfunction
  function void reset();
    this.count = 0;
    this.was_reached = 0;
  endfunction
  function int wait(int n);
    if (this.threshold > 0) begin
      this.count = this.count + n;
      this.waits = this.waits + 1;
      if (this.count >= this.threshold) begin
        this.count = this.threshold;
        this.was_reached = 1;
        wait = 1;
      end else begin
        wait = 0;
      end
    end else begin
      wait = 0;
    end
  endfunction
  function int reached();
    reached = this.was_reached;
  endfunction
endclass
class uvm_tlm_fifo extends uvm_object;
  // 深度 4 的移位式 FIFO：q0 为队首，put 追加到 q[count]。
  int q0 = 0;
  int q1 = 0;
  int q2 = 0;
  int q3 = 0;
  int count = 0;
  int puts = 0;
  int gets = 0;
  function new(int id);
    super.new(id);
  endfunction
  function int try_put(int v);
    if (this.count == 0) begin
      this.q0 = v;
      this.count = 1;
      this.puts = this.puts + 1;
      try_put = 1;
    end else if (this.count == 1) begin
      this.q1 = v;
      this.count = 2;
      this.puts = this.puts + 1;
      try_put = 1;
    end else if (this.count == 2) begin
      this.q2 = v;
      this.count = 3;
      this.puts = this.puts + 1;
      try_put = 1;
    end else if (this.count == 3) begin
      this.q3 = v;
      this.count = 4;
      this.puts = this.puts + 1;
      try_put = 1;
    end else begin
      try_put = 0;
    end
  endfunction
  function int try_get();
    if (this.count != 0) begin
      try_get = this.q0;
      this.q0 = this.q1;
      this.q1 = this.q2;
      this.q2 = this.q3;
      this.count = this.count - 1;
      this.gets = this.gets + 1;
    end else begin
      try_get = 0;
    end
  endfunction
  function int peek();
    peek = (this.count != 0) ? this.q0 : 0;
  endfunction
endclass
class uvm_queue extends uvm_object;
  // 深度 4 的 int 队列子集：q0 为队首，支持两端进出。
  int q0 = 0;
  int q1 = 0;
  int q2 = 0;
  int q3 = 0;
  int count = 0;
  int pushes = 0;
  int pops = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void push_back(int v);
    if (this.count == 0) begin
      this.q0 = v;
      this.count = 1;
      this.pushes = this.pushes + 1;
    end else if (this.count == 1) begin
      this.q1 = v;
      this.count = 2;
      this.pushes = this.pushes + 1;
    end else if (this.count == 2) begin
      this.q2 = v;
      this.count = 3;
      this.pushes = this.pushes + 1;
    end else if (this.count == 3) begin
      this.q3 = v;
      this.count = 4;
      this.pushes = this.pushes + 1;
    end
  endfunction
  function void push_front(int v);
    if (this.count == 0) begin
      this.q0 = v;
      this.count = 1;
      this.pushes = this.pushes + 1;
    end else if (this.count == 1) begin
      this.q1 = this.q0;
      this.q0 = v;
      this.count = 2;
      this.pushes = this.pushes + 1;
    end else if (this.count == 2) begin
      this.q3 = this.q2;
      this.q2 = this.q1;
      this.q1 = this.q0;
      this.q0 = v;
      this.count = 3;
      this.pushes = this.pushes + 1;
    end else if (this.count == 3) begin
      this.q3 = this.q2;
      this.q2 = this.q1;
      this.q1 = this.q0;
      this.q0 = v;
      this.count = 4;
      this.pushes = this.pushes + 1;
    end
  endfunction
  function int pop_front();
    if (this.count != 0) begin
      pop_front = this.q0;
      this.q0 = this.q1;
      this.q1 = this.q2;
      this.q2 = this.q3;
      this.count = this.count - 1;
      this.pops = this.pops + 1;
    end else begin
      pop_front = 0;
    end
  endfunction
  function int peek_front();
    peek_front = this.q0;
  endfunction
  function void shift_front();
    if (this.count != 0) begin
      this.q0 = this.q1;
      this.q1 = this.q2;
      this.q2 = this.q3;
      this.count = this.count - 1;
      this.pops = this.pops + 1;
    end
  endfunction
  function int pop_back();
    if (this.count == 4) begin
      pop_back = this.q3;
      this.q3 = 0;
      this.count = 3;
      this.pops = this.pops + 1;
    end else if (this.count == 3) begin
      pop_back = this.q2;
      this.q2 = 0;
      this.count = 2;
      this.pops = this.pops + 1;
    end else if (this.count == 2) begin
      pop_back = this.q1;
      this.q1 = 0;
      this.count = 1;
      this.pops = this.pops + 1;
    end else if (this.count == 1) begin
      pop_back = this.q0;
      this.q0 = 0;
      this.count = 0;
      this.pops = this.pops + 1;
    end else begin
      pop_back = 0;
    end
  endfunction
  function int size();
    size = this.count;
  endfunction
  function void delete();
    this.q0 = 0;
    this.q1 = 0;
    this.q2 = 0;
    this.q3 = 0;
    this.count = 0;
  endfunction
endclass
class uvm_pool extends uvm_object;
  // 深度 4 的 int->int pool 子集：键唯一，按插入顺序保存。
  int k0 = 0;
  int k1 = 0;
  int k2 = 0;
  int k3 = 0;
  int v0 = 0;
  int v1 = 0;
  int v2 = 0;
  int v3 = 0;
  int count = 0;
  int adds = 0;
  int updates = 0;
  int removes = 0;
  function new(int id);
    super.new(id);
  endfunction
  function int find(int key);
    if (this.k0 == key) find = this.v0;
    else if (this.k1 == key) find = this.v1;
    else if (this.k2 == key) find = this.v2;
    else if (this.k3 == key) find = this.v3;
    else find = 0;
  endfunction
  function int exists(int key);
    exists = (this.k0 == key) || (this.k1 == key) ||
             (this.k2 == key) || (this.k3 == key);
  endfunction
  function void add(int key, int value);
    if (this.k0 == key) begin
      this.v0 = value;
      this.updates = this.updates + 1;
    end else if (this.k1 == key) begin
      this.v1 = value;
      this.updates = this.updates + 1;
    end else if (this.k2 == key) begin
      this.v2 = value;
      this.updates = this.updates + 1;
    end else if (this.k3 == key) begin
      this.v3 = value;
      this.updates = this.updates + 1;
    end else if (this.count == 0) begin
      this.k0 = key;
      this.v0 = value;
      this.count = 1;
      this.adds = this.adds + 1;
    end else if (this.count == 1) begin
      this.k1 = key;
      this.v1 = value;
      this.count = 2;
      this.adds = this.adds + 1;
    end else if (this.count == 2) begin
      this.k2 = key;
      this.v2 = value;
      this.count = 3;
      this.adds = this.adds + 1;
    end else if (this.count == 3) begin
      this.k3 = key;
      this.v3 = value;
      this.count = 4;
      this.adds = this.adds + 1;
    end
  endfunction
  function void remove(int key);
    if (this.k0 == key) begin
      this.k0 = this.k1;
      this.v0 = this.v1;
      this.k1 = this.k2;
      this.v1 = this.v2;
      this.k2 = this.k3;
      this.v2 = this.v3;
      this.count = this.count - 1;
      this.removes = this.removes + 1;
    end else if (this.k1 == key) begin
      this.k1 = this.k2;
      this.v1 = this.v2;
      this.k2 = this.k3;
      this.v2 = this.v3;
      this.count = this.count - 1;
      this.removes = this.removes + 1;
    end else if (this.k2 == key) begin
      this.k2 = this.k3;
      this.v2 = this.v3;
      this.count = this.count - 1;
      this.removes = this.removes + 1;
    end else if (this.k3 == key) begin
      this.count = this.count - 1;
      this.removes = this.removes + 1;
    end
  endfunction
  function int num();
    num = this.count;
  endfunction
  function void delete();
    this.k0 = 0;
    this.k1 = 0;
    this.k2 = 0;
    this.k3 = 0;
    this.v0 = 0;
    this.v1 = 0;
    this.v2 = 0;
    this.v3 = 0;
    this.count = 0;
  endfunction
endclass
virtual class uvm_config_db;
  // 深度 4 的静态 string->int 配置数据库子集。
  static string k0 = "";
  static string k1 = "";
  static string k2 = "";
  static string k3 = "";
  static int v0 = 0;
  static int v1 = 0;
  static int v2 = 0;
  static int v3 = 0;
  static int count = 0;
  static int sets = 0;
  static int updates = 0;
  static int removals = 0;
  static function int set(string key, int value);
    if (uvm_config_db.k0 == key) begin
      uvm_config_db.v0 = value;
      uvm_config_db.updates = uvm_config_db.updates + 1;
      set = 1;
    end else if (uvm_config_db.k1 == key) begin
      uvm_config_db.v1 = value;
      uvm_config_db.updates = uvm_config_db.updates + 1;
      set = 1;
    end else if (uvm_config_db.k2 == key) begin
      uvm_config_db.v2 = value;
      uvm_config_db.updates = uvm_config_db.updates + 1;
      set = 1;
    end else if (uvm_config_db.k3 == key) begin
      uvm_config_db.v3 = value;
      uvm_config_db.updates = uvm_config_db.updates + 1;
      set = 1;
    end else if (uvm_config_db.count == 0) begin
      uvm_config_db.k0 = key;
      uvm_config_db.v0 = value;
      uvm_config_db.count = 1;
      uvm_config_db.sets = uvm_config_db.sets + 1;
      set = 1;
    end else if (uvm_config_db.count == 1) begin
      uvm_config_db.k1 = key;
      uvm_config_db.v1 = value;
      uvm_config_db.count = 2;
      uvm_config_db.sets = uvm_config_db.sets + 1;
      set = 1;
    end else if (uvm_config_db.count == 2) begin
      uvm_config_db.k2 = key;
      uvm_config_db.v2 = value;
      uvm_config_db.count = 3;
      uvm_config_db.sets = uvm_config_db.sets + 1;
      set = 1;
    end else if (uvm_config_db.count == 3) begin
      uvm_config_db.k3 = key;
      uvm_config_db.v3 = value;
      uvm_config_db.count = 4;
      uvm_config_db.sets = uvm_config_db.sets + 1;
      set = 1;
    end else begin
      set = 0;
    end
  endfunction
  static function int get(string key);
    if (uvm_config_db.k0 == key) get = uvm_config_db.v0;
    else if (uvm_config_db.k1 == key) get = uvm_config_db.v1;
    else if (uvm_config_db.k2 == key) get = uvm_config_db.v2;
    else if (uvm_config_db.k3 == key) get = uvm_config_db.v3;
    else get = 0;
  endfunction
  static function int exists(string key);
    exists = (uvm_config_db.k0 == key) ||
             (uvm_config_db.k1 == key) ||
             (uvm_config_db.k2 == key) ||
             (uvm_config_db.k3 == key);
  endfunction
  static function int remove(string key);
    if (uvm_config_db.k0 == key) begin
      uvm_config_db.k0 = uvm_config_db.k1;
      uvm_config_db.v0 = uvm_config_db.v1;
      uvm_config_db.k1 = uvm_config_db.k2;
      uvm_config_db.v1 = uvm_config_db.v2;
      uvm_config_db.k2 = uvm_config_db.k3;
      uvm_config_db.v2 = uvm_config_db.v3;
      uvm_config_db.count = uvm_config_db.count - 1;
      uvm_config_db.removals = uvm_config_db.removals + 1;
      remove = 1;
    end else if (uvm_config_db.k1 == key) begin
      uvm_config_db.k1 = uvm_config_db.k2;
      uvm_config_db.v1 = uvm_config_db.v2;
      uvm_config_db.k2 = uvm_config_db.k3;
      uvm_config_db.v2 = uvm_config_db.v3;
      uvm_config_db.count = uvm_config_db.count - 1;
      uvm_config_db.removals = uvm_config_db.removals + 1;
      remove = 1;
    end else if (uvm_config_db.k2 == key) begin
      uvm_config_db.k2 = uvm_config_db.k3;
      uvm_config_db.v2 = uvm_config_db.v3;
      uvm_config_db.count = uvm_config_db.count - 1;
      uvm_config_db.removals = uvm_config_db.removals + 1;
      remove = 1;
    end else if (uvm_config_db.k3 == key) begin
      uvm_config_db.count = uvm_config_db.count - 1;
      uvm_config_db.removals = uvm_config_db.removals + 1;
      remove = 1;
    end else begin
      remove = 0;
    end
  endfunction
endclass
virtual class uvm_factory;
  // 深度 4 的静态类型注册 / override 表子集。
  static string n0 = "";
  static string n1 = "";
  static string n2 = "";
  static string n3 = "";
  static string o0 = "";
  static string o1 = "";
  static string o2 = "";
  static string o3 = "";
  static int count = 0;
  static int registrations = 0;
  static int rejected = 0;
  static int overrides = 0;
  static int removals = 0;
  static string in0 = "";
  static string in1 = "";
  static string in2 = "";
  static string in3 = "";
  static string it0 = "";
  static string it1 = "";
  static string it2 = "";
  static string it3 = "";
  static string ir0 = "";
  static string ir1 = "";
  static string ir2 = "";
  static string ir3 = "";
  static int icount = 0;
  static int inst_overrides = 0;
  static int inst_updates = 0;
  static int rejected_inst_overrides = 0;
  static function int register(string type_name);
    if (uvm_factory.n0 == type_name) begin
      uvm_factory.rejected = uvm_factory.rejected + 1;
      register = 0;
    end else if (uvm_factory.n1 == type_name) begin
      uvm_factory.rejected = uvm_factory.rejected + 1;
      register = 0;
    end else if (uvm_factory.n2 == type_name) begin
      uvm_factory.rejected = uvm_factory.rejected + 1;
      register = 0;
    end else if (uvm_factory.n3 == type_name) begin
      uvm_factory.rejected = uvm_factory.rejected + 1;
      register = 0;
    end else if (uvm_factory.count == 0) begin
      uvm_factory.n0 = type_name;
      uvm_factory.o0 = "";
      uvm_factory.count = 1;
      uvm_factory.registrations = uvm_factory.registrations + 1;
      register = 1;
    end else if (uvm_factory.count == 1) begin
      uvm_factory.n1 = type_name;
      uvm_factory.o1 = "";
      uvm_factory.count = 2;
      uvm_factory.registrations = uvm_factory.registrations + 1;
      register = 1;
    end else if (uvm_factory.count == 2) begin
      uvm_factory.n2 = type_name;
      uvm_factory.o2 = "";
      uvm_factory.count = 3;
      uvm_factory.registrations = uvm_factory.registrations + 1;
      register = 1;
    end else if (uvm_factory.count == 3) begin
      uvm_factory.n3 = type_name;
      uvm_factory.o3 = "";
      uvm_factory.count = 4;
      uvm_factory.registrations = uvm_factory.registrations + 1;
      register = 1;
    end else begin
      uvm_factory.rejected = uvm_factory.rejected + 1;
      register = 0;
    end
  endfunction
  static function int set_type_override(string original, string replacement);
    if (uvm_factory.n0 == original) begin
      uvm_factory.o0 = replacement;
      uvm_factory.overrides = uvm_factory.overrides + 1;
      set_type_override = 1;
    end else if (uvm_factory.n1 == original) begin
      uvm_factory.o1 = replacement;
      uvm_factory.overrides = uvm_factory.overrides + 1;
      set_type_override = 1;
    end else if (uvm_factory.n2 == original) begin
      uvm_factory.o2 = replacement;
      uvm_factory.overrides = uvm_factory.overrides + 1;
      set_type_override = 1;
    end else if (uvm_factory.n3 == original) begin
      uvm_factory.o3 = replacement;
      uvm_factory.overrides = uvm_factory.overrides + 1;
      set_type_override = 1;
    end else begin
      set_type_override = 0;
    end
  endfunction
  static function string get_override(string type_name);
    if (uvm_factory.n0 == type_name)
      get_override = (uvm_factory.o0 == "") ? type_name : uvm_factory.o0;
    else if (uvm_factory.n1 == type_name)
      get_override = (uvm_factory.o1 == "") ? type_name : uvm_factory.o1;
    else if (uvm_factory.n2 == type_name)
      get_override = (uvm_factory.o2 == "") ? type_name : uvm_factory.o2;
    else if (uvm_factory.n3 == type_name)
      get_override = (uvm_factory.o3 == "") ? type_name : uvm_factory.o3;
    else
      get_override = "";
  endfunction
  static function int has_override(string type_name);
    has_override = ((uvm_factory.n0 == type_name) && (uvm_factory.o0 != "")) ||
                   ((uvm_factory.n1 == type_name) && (uvm_factory.o1 != "")) ||
                   ((uvm_factory.n2 == type_name) && (uvm_factory.o2 != "")) ||
                   ((uvm_factory.n3 == type_name) && (uvm_factory.o3 != ""));
  endfunction
  static function int is_registered(string type_name);
    is_registered = (uvm_factory.n0 == type_name) ||
                    (uvm_factory.n1 == type_name) ||
                    (uvm_factory.n2 == type_name) ||
                    (uvm_factory.n3 == type_name);
  endfunction
  static function int remove(string type_name);
    if (uvm_factory.n0 == type_name) begin
      uvm_factory.n0 = uvm_factory.n1;
      uvm_factory.o0 = uvm_factory.o1;
      uvm_factory.n1 = uvm_factory.n2;
      uvm_factory.o1 = uvm_factory.o2;
      uvm_factory.n2 = uvm_factory.n3;
      uvm_factory.o2 = uvm_factory.o3;
      uvm_factory.count = uvm_factory.count - 1;
      uvm_factory.removals = uvm_factory.removals + 1;
      remove = 1;
    end else if (uvm_factory.n1 == type_name) begin
      uvm_factory.n1 = uvm_factory.n2;
      uvm_factory.o1 = uvm_factory.o2;
      uvm_factory.n2 = uvm_factory.n3;
      uvm_factory.o2 = uvm_factory.o3;
      uvm_factory.count = uvm_factory.count - 1;
      uvm_factory.removals = uvm_factory.removals + 1;
      remove = 1;
    end else if (uvm_factory.n2 == type_name) begin
      uvm_factory.n2 = uvm_factory.n3;
      uvm_factory.o2 = uvm_factory.o3;
      uvm_factory.count = uvm_factory.count - 1;
      uvm_factory.removals = uvm_factory.removals + 1;
      remove = 1;
    end else if (uvm_factory.n3 == type_name) begin
      uvm_factory.count = uvm_factory.count - 1;
      uvm_factory.removals = uvm_factory.removals + 1;
      remove = 1;
    end else begin
      remove = 0;
    end
  endfunction
  static function int set_inst_override(
    string instance_name,
    string original,
    string replacement
  );
    if (!((uvm_factory.n0 == original) ||
          (uvm_factory.n1 == original) ||
          (uvm_factory.n2 == original) ||
          (uvm_factory.n3 == original))) begin
      uvm_factory.rejected_inst_overrides =
          uvm_factory.rejected_inst_overrides + 1;
      set_inst_override = 0;
    end else if ((uvm_factory.in0 == instance_name) &&
                 (uvm_factory.it0 == original)) begin
      uvm_factory.ir0 = replacement;
      uvm_factory.inst_updates = uvm_factory.inst_updates + 1;
      set_inst_override = 1;
    end else if ((uvm_factory.in1 == instance_name) &&
                 (uvm_factory.it1 == original)) begin
      uvm_factory.ir1 = replacement;
      uvm_factory.inst_updates = uvm_factory.inst_updates + 1;
      set_inst_override = 1;
    end else if ((uvm_factory.in2 == instance_name) &&
                 (uvm_factory.it2 == original)) begin
      uvm_factory.ir2 = replacement;
      uvm_factory.inst_updates = uvm_factory.inst_updates + 1;
      set_inst_override = 1;
    end else if ((uvm_factory.in3 == instance_name) &&
                 (uvm_factory.it3 == original)) begin
      uvm_factory.ir3 = replacement;
      uvm_factory.inst_updates = uvm_factory.inst_updates + 1;
      set_inst_override = 1;
    end else if (uvm_factory.icount == 0) begin
      uvm_factory.in0 = instance_name;
      uvm_factory.it0 = original;
      uvm_factory.ir0 = replacement;
      uvm_factory.icount = 1;
      uvm_factory.inst_overrides = uvm_factory.inst_overrides + 1;
      set_inst_override = 1;
    end else if (uvm_factory.icount == 1) begin
      uvm_factory.in1 = instance_name;
      uvm_factory.it1 = original;
      uvm_factory.ir1 = replacement;
      uvm_factory.icount = 2;
      uvm_factory.inst_overrides = uvm_factory.inst_overrides + 1;
      set_inst_override = 1;
    end else if (uvm_factory.icount == 2) begin
      uvm_factory.in2 = instance_name;
      uvm_factory.it2 = original;
      uvm_factory.ir2 = replacement;
      uvm_factory.icount = 3;
      uvm_factory.inst_overrides = uvm_factory.inst_overrides + 1;
      set_inst_override = 1;
    end else if (uvm_factory.icount == 3) begin
      uvm_factory.in3 = instance_name;
      uvm_factory.it3 = original;
      uvm_factory.ir3 = replacement;
      uvm_factory.icount = 4;
      uvm_factory.inst_overrides = uvm_factory.inst_overrides + 1;
      set_inst_override = 1;
    end else begin
      uvm_factory.rejected_inst_overrides =
          uvm_factory.rejected_inst_overrides + 1;
      set_inst_override = 0;
    end
  endfunction
  static function string get_inst_override(
    string instance_name,
    string type_name
  );
    if ((uvm_factory.in0 == instance_name) &&
        (uvm_factory.it0 == type_name))
      get_inst_override = uvm_factory.ir0;
    else if ((uvm_factory.in1 == instance_name) &&
             (uvm_factory.it1 == type_name))
      get_inst_override = uvm_factory.ir1;
    else if ((uvm_factory.in2 == instance_name) &&
             (uvm_factory.it2 == type_name))
      get_inst_override = uvm_factory.ir2;
    else if ((uvm_factory.in3 == instance_name) &&
             (uvm_factory.it3 == type_name))
      get_inst_override = uvm_factory.ir3;
    else if (uvm_factory.n0 == type_name)
      get_inst_override = (uvm_factory.o0 == "") ? type_name : uvm_factory.o0;
    else if (uvm_factory.n1 == type_name)
      get_inst_override = (uvm_factory.o1 == "") ? type_name : uvm_factory.o1;
    else if (uvm_factory.n2 == type_name)
      get_inst_override = (uvm_factory.o2 == "") ? type_name : uvm_factory.o2;
    else if (uvm_factory.n3 == type_name)
      get_inst_override = (uvm_factory.o3 == "") ? type_name : uvm_factory.o3;
    else
      get_inst_override = "";
  endfunction
  static function int has_inst_override(
    string instance_name,
    string type_name
  );
    has_inst_override = ((uvm_factory.in0 == instance_name) &&
                         (uvm_factory.it0 == type_name)) ||
                        ((uvm_factory.in1 == instance_name) &&
                         (uvm_factory.it1 == type_name)) ||
                        ((uvm_factory.in2 == instance_name) &&
                         (uvm_factory.it2 == type_name)) ||
                        ((uvm_factory.in3 == instance_name) &&
                         (uvm_factory.it3 == type_name));
  endfunction
  static function int remove_inst_override(
    string instance_name,
    string type_name
  );
    if ((uvm_factory.in0 == instance_name) &&
        (uvm_factory.it0 == type_name)) begin
      uvm_factory.in0 = uvm_factory.in1;
      uvm_factory.it0 = uvm_factory.it1;
      uvm_factory.ir0 = uvm_factory.ir1;
      uvm_factory.in1 = uvm_factory.in2;
      uvm_factory.it1 = uvm_factory.it2;
      uvm_factory.ir1 = uvm_factory.ir2;
      uvm_factory.in2 = uvm_factory.in3;
      uvm_factory.it2 = uvm_factory.it3;
      uvm_factory.ir2 = uvm_factory.ir3;
      uvm_factory.icount = uvm_factory.icount - 1;
      remove_inst_override = 1;
    end else if ((uvm_factory.in1 == instance_name) &&
                 (uvm_factory.it1 == type_name)) begin
      uvm_factory.in1 = uvm_factory.in2;
      uvm_factory.it1 = uvm_factory.it2;
      uvm_factory.ir1 = uvm_factory.ir2;
      uvm_factory.in2 = uvm_factory.in3;
      uvm_factory.it2 = uvm_factory.it3;
      uvm_factory.ir2 = uvm_factory.ir3;
      uvm_factory.icount = uvm_factory.icount - 1;
      remove_inst_override = 1;
    end else if ((uvm_factory.in2 == instance_name) &&
                 (uvm_factory.it2 == type_name)) begin
      uvm_factory.in2 = uvm_factory.in3;
      uvm_factory.it2 = uvm_factory.it3;
      uvm_factory.ir2 = uvm_factory.ir3;
      uvm_factory.icount = uvm_factory.icount - 1;
      remove_inst_override = 1;
    end else if ((uvm_factory.in3 == instance_name) &&
                 (uvm_factory.it3 == type_name)) begin
      uvm_factory.icount = uvm_factory.icount - 1;
      remove_inst_override = 1;
    end else begin
      remove_inst_override = 0;
    end
  endfunction
endclass
class uvm_analysis_port extends uvm_object;
  uvm_subscriber s0;
  uvm_subscriber s1;
  uvm_subscriber s2;
  uvm_subscriber s3;
  int writes = 0;
  int connects = 0;
  int rejected_connects = 0;
  function new(int id);
    super.new(id);
  endfunction
  function void connect(uvm_subscriber s);
    this.s0 = s;
    this.connects = 1;
  endfunction
  function void connect1(uvm_subscriber s);
    this.s1 = s;
    this.connects = 2;
  endfunction
  function void connect2(uvm_subscriber s);
    this.s2 = s;
    this.connects = 3;
  endfunction
  function void connect3(uvm_subscriber s);
    this.s3 = s;
    this.connects = 4;
  endfunction
  function void reject();
    this.rejected_connects = this.rejected_connects + 1;
  endfunction
  function void write(int data);
    this.s0.write(data);
    this.writes = this.writes + 1;
  endfunction
  function void write4(int data);
    this.s0.write(data);
    this.s1.write(data);
    this.s2.write(data);
    this.s3.write(data);
    this.writes = this.writes + 1;
  endfunction
endclass
endpackage
"#;

pub(super) const BUILTIN_UVM_CLASS_NAMES: &[&str] = &[
    "uvm_object",
    "uvm_component",
    "uvm_subscriber",
    "uvm_callback",
    "uvm_callback_pool",
    "uvm_sequence_item",
    "uvm_sequencer",
    "uvm_sequence",
    "uvm_seq_item_port",
    "uvm_driver",
    "uvm_objection",
    "uvm_event",
    "uvm_barrier",
    "uvm_tlm_fifo",
    "uvm_queue",
    "uvm_pool",
    "uvm_config_db",
    "uvm_factory",
    "uvm_analysis_port",
];

pub(super) fn collect_classes(
    file: &ast::SourceFile,
) -> Result<HashMap<String, ast::ClassDef>, String> {
    let mut classes = HashMap::new();
    let mut generic_classes = HashMap::new();
    let symbolic_specializations: HashMap<&str, &ast::ClassSpecialization> = file
        .class_specializations
        .iter()
        .filter(|specialization| {
            specialization
                .values
                .iter()
                .any(|value| matches!(value, ast::ClassParameterValue::Parameter(_)))
        })
        .map(|specialization| (specialization.canonical_name.as_str(), specialization))
        .collect();
    let mut generated_specializations = Vec::new();
    let builtin = parser::parse(BUILTIN_UVM_CLASSES)?;
    let mut builtin_classes: Vec<&ast::ClassDef> = builtin
        .modules
        .iter()
        .flat_map(|module| module.items.iter())
        .filter_map(|item| match item {
            ast::Item::ClassDef(definition) => Some(definition),
            _ => None,
        })
        .chain(
            builtin
                .packages
                .iter()
                .flat_map(|package| package.classes.iter()),
        )
        .collect();
    for definition in builtin_classes.drain(..) {
        classes.insert(definition.name.clone(), definition.clone());
    }
    let mut user_classes: Vec<&ast::ClassDef> = file
        .modules
        .iter()
        .flat_map(|module| module.items.iter())
        .filter_map(|item| match item {
            ast::Item::ClassDef(definition) => Some(definition),
            _ => None,
        })
        .chain(file.classes.iter())
        .chain(
            file.packages
                .iter()
                .flat_map(|package| package.classes.iter()),
        )
        .collect();
    for definition in user_classes.drain(..) {
        generic_classes.insert(definition.name.clone(), definition.clone());
        let definition = specialize_class_defaults(
            definition,
            &symbolic_specializations,
            &mut generated_specializations,
        )?;
        if classes
            .insert(definition.name.clone(), definition.clone())
            .is_some()
        {
            return Err(format!("duplicate class '{}'", definition.name));
        }
    }
    // Package classes are additionally addressable by qualified name and,
    // when unambiguous, by their simple name after `import pkg::*;`.
    for package in &file.packages {
        for definition in &package.classes {
            let definition = specialize_class_defaults(
                definition,
                &symbolic_specializations,
                &mut generated_specializations,
            )?;
            classes.insert(format!("{}::{}", package.name, definition.name), definition);
        }
    }
    for import in &file.imports {
        if import.names.is_empty() {
            if let Some(package) = file.packages.iter().find(|p| p.name == import.package) {
                for definition in &package.classes {
                    let definition = specialize_class_defaults(
                        definition,
                        &symbolic_specializations,
                        &mut generated_specializations,
                    )?;
                    classes.entry(definition.name.clone()).or_insert(definition);
                }
            }
        } else {
            for name in &import.names {
                if let Some(definition) = file
                    .packages
                    .iter()
                    .find(|p| p.name == import.package)
                    .and_then(|p| p.classes.iter().find(|c| &c.name == name))
                {
                    let definition = specialize_class_defaults(
                        definition,
                        &symbolic_specializations,
                        &mut generated_specializations,
                    )?;
                    classes.insert(name.clone(), definition);
                }
            }
        }
    }
    let mut specialization_queue: Vec<_> = file
        .class_specializations
        .iter()
        .filter(|specialization| {
            specialization
                .values
                .iter()
                .all(|value| matches!(value, ast::ClassParameterValue::Constant(_)))
        })
        .cloned()
        .collect();
    specialization_queue.extend(generated_specializations);
    while let Some(specialization) = specialization_queue.pop() {
        let Some(generic) = generic_classes.get(&specialization.base_name) else {
            return Err(format!(
                "unknown parameterized class '{}'",
                specialization.base_name
            ));
        };
        let specialized = specialize_class_values(
            generic,
            &specialization.parameter_names,
            &specialization.values,
            &symbolic_specializations,
            &mut specialization_queue,
        )?;
        let mut specialized = specialized;
        specialized.name = specialization.canonical_name.clone();
        classes.insert(specialization.canonical_name.clone(), specialized);
    }
    Ok(classes)
}

pub(super) fn specialize_class_defaults(
    definition: &ast::ClassDef,
    symbolic_specializations: &HashMap<&str, &ast::ClassSpecialization>,
    generated_specializations: &mut Vec<ast::ClassSpecialization>,
) -> Result<ast::ClassDef, String> {
    let mut values = HashMap::new();
    for (parameter, default) in &definition.params {
        let Some(default) = default else {
            return Err(format!(
                "class '{}' parameter '{}' requires a default in this subset",
                definition.name, parameter
            ));
        };
        let value = const_eval(default, &values, "class parameter")?;
        values.insert(parameter.clone(), value);
    }
    let mut specialized = definition.clone();
    for (parameter, value) in &values {
        specialized = substitute_class_definition(&specialized, parameter, *value);
    }
    specialized.extends = resolve_symbolic_extends(
        specialized.extends,
        &values,
        symbolic_specializations,
        generated_specializations,
    )?;
    specialized.params.clear();
    Ok(specialized)
}

pub(super) fn specialize_class_values(
    definition: &ast::ClassDef,
    parameter_names: &[Option<String>],
    actual_values: &[ast::ClassParameterValue],
    symbolic_specializations: &HashMap<&str, &ast::ClassSpecialization>,
    generated_specializations: &mut Vec<ast::ClassSpecialization>,
) -> Result<ast::ClassDef, String> {
    let mut values = HashMap::new();
    for (index, (parameter, default)) in definition.params.iter().enumerate() {
        let mut value = None;
        if index < actual_values.len() && parameter_names[index].is_none() {
            value = Some(constant_class_actual(&actual_values[index])?);
        }
        for (name, actual) in parameter_names.iter().zip(actual_values) {
            if name.as_deref() == Some(parameter.as_str()) {
                if value.is_some() {
                    return Err(format!(
                        "class '{}' parameter '{}' has multiple actuals",
                        definition.name, parameter
                    ));
                }
                value = Some(constant_class_actual(actual)?);
            }
        }
        let value = match value {
            Some(value) => value,
            None => {
                let Some(default) = default else {
                    return Err(format!(
                        "class '{}' parameter '{}' requires an explicit or default value",
                        definition.name, parameter
                    ));
                };
                const_eval(default, &values, "class parameter")?
            }
        };
        values.insert(parameter.clone(), value);
    }
    if actual_values.len() > definition.params.len() {
        return Err(format!(
            "class '{}' expects at most {} parameter(s), received {}",
            definition.name,
            definition.params.len(),
            actual_values.len()
        ));
    }
    let mut specialized = definition.clone();
    for (parameter, value) in &values {
        specialized = substitute_class_definition(&specialized, parameter, *value);
    }
    specialized.extends = resolve_symbolic_extends(
        specialized.extends,
        &values,
        symbolic_specializations,
        generated_specializations,
    )?;
    specialized.params.clear();
    Ok(specialized)
}

pub(super) fn constant_class_actual(value: &ast::ClassParameterValue) -> Result<i64, String> {
    match value {
        ast::ClassParameterValue::Constant(value) => Ok(*value),
        ast::ClassParameterValue::Parameter(parameter) => Err(format!(
            "class specialization parameter '{parameter}' is not bound to a constant"
        )),
    }
}

pub(super) fn resolve_symbolic_extends(
    extends: Option<String>,
    values: &HashMap<String, i64>,
    symbolic_specializations: &HashMap<&str, &ast::ClassSpecialization>,
    generated_specializations: &mut Vec<ast::ClassSpecialization>,
) -> Result<Option<String>, String> {
    let Some(canonical_name) = extends else {
        return Ok(None);
    };
    let Some(symbolic) = symbolic_specializations.get(canonical_name.as_str()) else {
        return Ok(Some(canonical_name));
    };
    let mut concrete_values = Vec::with_capacity(symbolic.values.len());
    for value in &symbolic.values {
        let value = match value {
            ast::ClassParameterValue::Constant(value) => *value,
            ast::ClassParameterValue::Parameter(parameter) => {
                *values.get(parameter).ok_or_else(|| {
                    format!(
                        "class parameter '{parameter}' is not bound in symbolic base specialization"
                    )
                })?
            }
        };
        concrete_values.push(ast::ClassParameterValue::Constant(value));
    }
    let concrete_name = elaborated_class_specialization_name(&symbolic.base_name, &concrete_values);
    generated_specializations.push(ast::ClassSpecialization {
        base_name: symbolic.base_name.clone(),
        canonical_name: concrete_name.clone(),
        parameter_names: symbolic.parameter_names.clone(),
        values: concrete_values,
    });
    Ok(Some(concrete_name))
}

pub(super) fn elaborated_class_specialization_name(
    base_name: &str,
    values: &[ast::ClassParameterValue],
) -> String {
    let mut name = format!("__eda_class_{base_name}");
    for value in values {
        name.push('_');
        if let ast::ClassParameterValue::Constant(value) = value {
            if *value < 0 {
                name.push('n');
            }
            name.push_str(&value.unsigned_abs().to_string());
        }
    }
    name
}

pub(super) fn substitute_class_definition(
    definition: &ast::ClassDef,
    parameter: &str,
    value: i64,
) -> ast::ClassDef {
    ast::ClassDef {
        name: definition.name.clone(),
        params: definition.params.clone(),
        extends: definition.extends.clone(),
        extends_args: definition
            .extends_args
            .iter()
            .map(|expression| subst_expr(expression, parameter, value))
            .collect(),
        is_virtual: definition.is_virtual,
        properties: definition
            .properties
            .iter()
            .map(|property| substitute_class_property(property, parameter, value))
            .collect(),
        methods: definition
            .methods
            .iter()
            .map(|method| substitute_function_definition(method, parameter, value))
            .collect(),
    }
}

pub(super) fn substitute_class_property(
    property: &ast::ClassProperty,
    parameter: &str,
    value: i64,
) -> ast::ClassProperty {
    ast::ClassProperty {
        quals: property.quals.clone(),
        is_reg: property.is_reg,
        is_signed: property.is_signed,
        type_name: property.type_name.clone(),
        range: substitute_range(&property.range, parameter, value),
        names: property
            .names
            .iter()
            .map(|(name, initializer)| {
                (
                    name.clone(),
                    initializer
                        .as_ref()
                        .map(|initializer| subst_expr(initializer, parameter, value)),
                )
            })
            .collect(),
    }
}

pub(super) fn substitute_function_definition(
    definition: &ast::FunctionDef,
    parameter: &str,
    value: i64,
) -> ast::FunctionDef {
    ast::FunctionDef {
        name: definition.name.clone(),
        return_range: substitute_range(&definition.return_range, parameter, value),
        args: definition
            .args
            .iter()
            .map(|(name, range)| (name.clone(), substitute_range(range, parameter, value)))
            .collect(),
        arg_defaults: definition
            .arg_defaults
            .iter()
            .map(|default| {
                default
                    .as_ref()
                    .map(|default| subst_expr(default, parameter, value))
            })
            .collect(),
        arg_types: definition.arg_types.clone(),
        body: definition
            .body
            .iter()
            .map(|statement| subst_stmt(statement, parameter, value))
            .collect(),
        is_virtual: definition.is_virtual,
        is_static: definition.is_static,
    }
}

pub(super) fn substitute_range(
    range: &ast::RangeAst,
    parameter: &str,
    value: i64,
) -> ast::RangeAst {
    range.as_ref().map(|(msb, lsb)| {
        (
            Box::new(subst_expr(msb, parameter, value)),
            Box::new(subst_expr(lsb, parameter, value)),
        )
    })
}

/// Whether a constructor body explicitly invokes `super.new(...)`.
pub(super) fn ctor_body_calls_super(body: &[ast::Stmt]) -> bool {
    body.iter().any(|statement| {
        matches!(
            statement,
            ast::Stmt::MethodCall {
                object,
                method,
                ..
            } if object == "super" && method == "new"
        ) || match statement {
            ast::Stmt::Block(nested) => ctor_body_calls_super(nested),
            ast::Stmt::If { t, f, .. } => {
                ctor_body_calls_super(std::slice::from_ref(t))
                    || f.as_ref()
                        .is_some_and(|branch| ctor_body_calls_super(std::slice::from_ref(branch)))
            }
            _ => false,
        }
    })
}

pub(super) fn is_cast_assignment(statement: &ast::Stmt) -> bool {
    match statement {
        ast::Stmt::BL {
            e: ast::Expr::Call(name, _),
            ..
        }
        | ast::Stmt::NBL {
            e: ast::Expr::Call(name, _),
            ..
        } => name == "$cast",
        _ => false,
    }
}
