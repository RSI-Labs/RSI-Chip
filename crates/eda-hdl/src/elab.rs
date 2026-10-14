//! Elaborator: flattens the module hierarchy into a single [`Design`],
//! resolving parameters, ranges, port directions and hierarchical names.

use crate::ast;
use crate::parser;
use eda_core::ir::{
    Assign, BinOp, Design, Dir, DispPart as DispPartIR, Expr as ExprIR, HierarchicalInstance,
    HierarchicalPort, LValue, NetDecl, PortDecl, Process, Range, Sens as SensIR, Stmt as StmtIR,
    UnOp,
};
use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{HashMap, HashSet};

/// Parse + elaborate in one step.
pub fn compile(source: &str, top: Option<&str>) -> Result<Design, String> {
    let file = parser::parse(source)?;
    elaborate(&file, top)
}

pub fn elaborate(file: &ast::SourceFile, top: Option<&str>) -> Result<Design, String> {
    let mut mods: HashMap<String, &ast::Module> = HashMap::new();
    for m in &file.modules {
        if mods.insert(m.name.clone(), m).is_some() {
            return Err(format!("duplicate module '{}'", m.name));
        }
    }
    let top_name = match top {
        Some(t) => {
            if !mods.contains_key(t) {
                return Err(format!("top module '{}' not found", t));
            }
            t.to_string()
        }
        None => pick_top(file)?,
    };
    let mut el = Elaborator {
        mods: &mods,
        packages: &resolve_packages(&file.packages)?,
        file_imports: &file.imports,
        design: Design {
            name: top_name.clone(),
            ..Default::default()
        },
        depth: 0,
        classes: collect_classes(file)?,
        next_object_id: 1,
    };
    el.elab_module(&top_name, "", &HashMap::new(), &HashMap::new())?;
    el.design
        .hierarchy
        .sort_by_key(|instance| (instance.depth, instance.path.clone()));
    Ok(el.design)
}

/// Built-in UVM-lite base classes available without an explicit package
/// import. These provide the minimal object identity surface that derived
/// verification classes extend.
const BUILTIN_UVM_CLASSES: &str = r#"
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

const BUILTIN_UVM_CLASS_NAMES: &[&str] = &[
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

fn collect_classes(file: &ast::SourceFile) -> Result<HashMap<String, ast::ClassDef>, String> {
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

fn specialize_class_defaults(
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

fn specialize_class_values(
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

fn constant_class_actual(value: &ast::ClassParameterValue) -> Result<i64, String> {
    match value {
        ast::ClassParameterValue::Constant(value) => Ok(*value),
        ast::ClassParameterValue::Parameter(parameter) => Err(format!(
            "class specialization parameter '{parameter}' is not bound to a constant"
        )),
    }
}

fn resolve_symbolic_extends(
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

fn elaborated_class_specialization_name(
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

fn substitute_class_definition(
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

fn substitute_class_property(
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

fn substitute_function_definition(
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

fn substitute_range(range: &ast::RangeAst, parameter: &str, value: i64) -> ast::RangeAst {
    range.as_ref().map(|(msb, lsb)| {
        (
            Box::new(subst_expr(msb, parameter, value)),
            Box::new(subst_expr(lsb, parameter, value)),
        )
    })
}

/// Whether a constructor body explicitly invokes `super.new(...)`.
fn ctor_body_calls_super(body: &[ast::Stmt]) -> bool {
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

fn is_cast_assignment(statement: &ast::Stmt) -> bool {
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

/// Pick the module that no other module instantiates; prefer tb/top names
/// when several candidates exist.
fn pick_top(file: &ast::SourceFile) -> Result<String, String> {
    let mut instantiated = std::collections::HashSet::new();
    for m in &file.modules {
        for it in &m.items {
            if let ast::Item::Inst { module, .. } = it {
                instantiated.insert(module.clone());
            }
        }
    }
    let cands: Vec<&ast::Module> = file
        .modules
        .iter()
        .filter(|m| !m.is_interface && !instantiated.contains(&m.name))
        .collect();
    match cands.len() {
        0 => Err("no top module: every module is instantiated".into()),
        1 => Ok(cands[0].name.clone()),
        _ => cands
            .iter()
            .find(|m| m.name.contains("tb") || m.name.contains("top"))
            .map(|m| m.name.clone())
            .or_else(|| cands.last().map(|m| m.name.clone()))
            .ok_or_else(|| "no top module".into()),
    }
}

#[derive(Clone)]
struct NetEntry {
    base: usize,
    width: usize,
    msb: i64,
    lsb: i64,
    type_name: Option<String>,
    is_signed: bool,
    array: Option<NetArray>,
}

#[derive(Clone)]
struct NetArray {
    dimensions: Vec<NetArrayDimension>,
    length: usize,
    element_width: usize,
    element_msb: i64,
    element_lsb: i64,
    dynamic: bool,
    dynamic_size_base: usize,
}

#[derive(Clone, Copy)]
struct NetArrayDimension {
    base: i64,
    step: i64,
    length: usize,
}

impl NetEntry {
    /// Declaration-relative offset of a source index (handles reversed
    /// declarations like `[0:3]`).
    fn offset(&self, index: i64) -> Result<usize, String> {
        let off = if self.msb >= self.lsb {
            index - self.lsb
        } else {
            self.lsb - index
        };
        if off < 0 || off as usize >= self.width {
            return Err(format!(
                "index {} out of range [{}:{}]",
                index, self.msb, self.lsb
            ));
        }
        Ok(off as usize)
    }

    fn array_offset(&self, indexes: &[i64]) -> Result<usize, String> {
        let array = self
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let mut element = 0_usize;
        for (index, dimension) in indexes.iter().zip(&array.dimensions) {
            let delta = index - dimension.base;
            if delta % dimension.step != 0 {
                return Err(format!(
                    "array index {} is not on declaration stride {}",
                    index, dimension.step
                ));
            }
            let offset = (delta / dimension.step) as usize;
            if offset >= dimension.length {
                return Err(format!(
                    "array index {} is out of range; length is {}",
                    index, dimension.length
                ));
            }
            element = element * dimension.length + offset;
        }
        if element >= array.length {
            return Err(format!(
                "array element calculation reached {element}; length is {}",
                array.length
            ));
        }
        Ok(element * array.element_width)
    }

    fn array_slice(&self, left: i64, right: i64) -> Result<(usize, usize), String> {
        let array = self
            .array
            .as_ref()
            .ok_or_else(|| "identifier is not an unpacked array".to_string())?;
        if array.dimensions.len() != 1 {
            return Err("unpacked array slices support one-dimensional arrays only".to_string());
        }
        let dimension = &array.dimensions[0];
        let follows_direction = if dimension.step > 0 {
            left <= right
        } else {
            left >= right
        };
        if !follows_direction {
            return Err(format!(
                "array slice [{left}:{right}] does not follow declaration direction with stride {}",
                dimension.step
            ));
        }
        let first = (left - dimension.base) / dimension.step;
        let last = (right - dimension.base) / dimension.step;
        if first < 0 || last < 0 {
            return Err(format!("array slice [{left}:{right}] is out of range"));
        }
        let first = first as usize;
        let last = last as usize;
        if first >= dimension.length || last >= dimension.length {
            return Err(format!(
                "array slice [{left}:{right}] is out of range; length is {}",
                dimension.length
            ));
        }
        let low = first.min(last);
        let high = first.max(last);
        let element_count = (left - right).unsigned_abs() as usize + 1;
        if high - low + 1 != element_count {
            return Err("array slice is not contiguous".to_string());
        }
        Ok((
            low * array.element_width,
            element_count * array.element_width,
        ))
    }
}

/// Stable per-elaboration class ids used for runtime virtual dispatch.
fn class_type_ids(classes: &HashMap<String, ast::ClassDef>) -> HashMap<String, u32> {
    let mut names: Vec<&String> = classes.keys().collect();
    names.sort();
    names
        .into_iter()
        .enumerate()
        .map(|(id, name)| (name.clone(), id as u32))
        .collect()
}

fn type_id_width(class_count: usize) -> usize {
    (usize::BITS - (class_count.max(1) - 1).leading_zeros()) as usize
}

fn type_id_net(handle: &str) -> String {
    format!("__{handle}__tid")
}

fn object_id_net(handle: &str) -> String {
    format!("__{handle}__oid")
}

/// Classes whose inheritance chain contains `base` (including itself):
/// every runtime type a `base`-typed handle may hold.
fn derived_classes<'a>(classes: &'a HashMap<String, ast::ClassDef>, base: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    for name in classes.keys() {
        let mut cursor = Some(name.as_str());
        let mut depth_left = 32;
        while let Some(current) = cursor {
            if current == base {
                out.push(name.as_str());
                break;
            }
            if depth_left == 0 {
                break;
            }
            depth_left -= 1;
            cursor = classes.get(current).and_then(|d| d.extends.as_deref());
        }
    }
    out.sort_unstable();
    out
}

/// Resolution of a dotted object path against the class object model.
#[derive(Clone, Debug)]
enum ClassPathTarget {
    /// A value property; contains the flattened storage net name.
    Field(String),
    /// A class handle (top-level or nested); contains the allocation
    /// flag net, the storage prefix, and the static class type.
    Handle {
        flag_net: String,
        prefix: String,
        class: String,
    },
}

struct Elaborator<'a> {
    mods: &'a HashMap<String, &'a ast::Module>,
    packages: &'a PackageTables,
    file_imports: &'a [ast::Import],
    design: Design,
    depth: usize,
    /// SystemVerilog class definitions visible to the design (module scope,
    /// package scope, and the built-in UVM-lite base classes).
    classes: HashMap<String, ast::ClassDef>,
    /// Compile-time serial assigned to each elaborated class construction.
    next_object_id: u64,
}

#[derive(Default)]
struct PackageTables {
    constants: HashMap<String, HashMap<String, i64>>,
    types: HashMap<String, HashMap<String, ResolvedType>>,
    functions: HashMap<String, HashMap<String, ast::FunctionDef>>,
}

#[derive(Clone, Debug)]
struct ResolvedEnumType {
    width: usize,
    constants: HashMap<String, i64>,
}

#[derive(Clone, Debug)]
struct ResolvedStructType {
    width: usize,
    fields: HashMap<String, (usize, usize)>,
}

#[derive(Clone, Debug)]
enum ResolvedType {
    Enum(ResolvedEnumType),
    Struct(ResolvedStructType),
    Union(ResolvedStructType),
}

impl ResolvedType {
    fn width(&self) -> usize {
        match self {
            Self::Enum(definition) => definition.width,
            Self::Struct(definition) => definition.width,
            Self::Union(definition) => definition.width,
        }
    }
}

#[derive(Clone, Debug)]
struct FunctionArrayDimension {
    base: i64,
    step: i64,
    length: usize,
}

#[derive(Clone, Debug)]
struct FunctionArrayLocal {
    dimensions: Vec<FunctionArrayDimension>,
    element_width: usize,
    element_msb: i64,
    element_lsb: i64,
    length: usize,
}

impl FunctionArrayLocal {
    fn element_index(&self, indexes: &[i64]) -> Result<usize, String> {
        if indexes.len() != self.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                self.dimensions.len(),
                indexes.len()
            ));
        }
        let mut element = 0_usize;
        for (index, dimension) in indexes.iter().zip(&self.dimensions) {
            let delta = index - dimension.base;
            if delta % dimension.step != 0 {
                return Err(format!(
                    "array index {} is not on declaration stride {}",
                    index, dimension.step
                ));
            }
            let offset = (delta / dimension.step) as usize;
            if offset >= dimension.length {
                return Err(format!(
                    "array index {} is out of range; length is {}",
                    index, dimension.length
                ));
            }
            element = element * dimension.length + offset;
        }
        if element >= self.length {
            return Err(format!(
                "array element calculation reached {element}; length is {}",
                self.length
            ));
        }
        Ok(element)
    }
}

#[derive(Clone, Debug, Default)]
struct FunctionAggregateState {
    types: HashMap<String, String>,
    arrays: HashMap<String, FunctionArrayLocal>,
    array_values: HashMap<String, Vec<ast::Expr>>,
    scalar_widths: HashMap<String, usize>,
    scalar_ranges: HashMap<String, (i64, i64)>,
}

#[derive(Clone)]
enum InterfaceInstanceBinding {
    Single(HashMap<String, NetEntry>),
    Array {
        dimensions: Vec<InterfaceArrayDimension>,
        elements: Vec<HashMap<String, NetEntry>>,
    },
}

#[derive(Clone, Copy)]
struct InterfaceArrayDimension {
    base: i64,
    step: i64,
    length: usize,
}

impl InterfaceInstanceBinding {
    fn select(&self, indexes: &[i64]) -> Result<&HashMap<String, NetEntry>, String> {
        match self {
            Self::Single(fields) if indexes.is_empty() => Ok(fields),
            Self::Single(_) => Err("scalar interface object is not an array".to_string()),
            Self::Array {
                dimensions,
                elements,
            } => {
                if indexes.len() != dimensions.len() {
                    return Err(format!(
                        "interface array expects {} index(es), received {}",
                        dimensions.len(),
                        indexes.len()
                    ));
                }
                let mut element = 0_usize;
                for (index, dimension) in indexes.iter().zip(dimensions) {
                    let delta = index - dimension.base;
                    if delta % dimension.step != 0 {
                        return Err(format!(
                            "interface array index {} is not on declaration stride {}",
                            index, dimension.step
                        ));
                    }
                    let offset = (delta / dimension.step) as usize;
                    if offset >= dimension.length {
                        return Err(format!(
                            "interface array index {} is out of range; length is {}",
                            index, dimension.length
                        ));
                    }
                    element = element * dimension.length + offset;
                }
                elements.get(element).ok_or_else(|| {
                    format!(
                        "interface array element calculation reached {element}; length is {}",
                        elements.len()
                    )
                })
            }
        }
    }
}

fn resolve_packages(packages: &[ast::Package]) -> Result<PackageTables, String> {
    let mut out = PackageTables::default();
    for package in packages {
        if out.constants.contains_key(&package.name) {
            return Err(format!("duplicate package '{}'", package.name));
        }
        let mut values = HashMap::new();
        for (name, expression) in &package.params {
            let value = package_const_eval(expression, &out, &package.name, &values)?;
            values.insert(name.clone(), value);
        }
        let mut types = HashMap::new();
        let mut functions = HashMap::new();
        for definition in &package.types {
            let resolved = resolve_type(definition, &values, &package.name, &types)?;
            if let ResolvedType::Enum(enum_definition) = &resolved {
                for (constant, value) in &enum_definition.constants {
                    values.insert(constant.clone(), *value);
                }
            }
            let type_name = match definition {
                ast::TypeDef::Enum(definition) => &definition.name,
                ast::TypeDef::Struct(definition) => &definition.name,
                ast::TypeDef::Union(definition) => &definition.name,
            };
            types.insert(type_name.clone(), resolved);
        }
        for definition in &package.functions {
            if functions.contains_key(&definition.name) {
                return Err(format!(
                    "package '{}': duplicate function '{}'",
                    package.name, definition.name
                ));
            }
            validate_function_body(definition)?;
            functions.insert(definition.name.clone(), definition.clone());
        }
        out.constants.insert(package.name.clone(), values);
        out.types.insert(package.name.clone(), types);
        out.functions.insert(package.name.clone(), functions);
    }
    Ok(out)
}

fn resolve_enum_type(
    definition: &ast::EnumTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
) -> Result<ResolvedEnumType, String> {
    let width = definition
        .range
        .as_ref()
        .map(|(msb, lsb)| -> Result<usize, String> {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            Ok((hi - lo).unsigned_abs() as usize + 1)
        })
        .transpose()?
        .unwrap_or(32);
    if width == 0 || width > 64 {
        return Err(format!(
            "enum typedef '{}' width must be between 1 and 64",
            definition.name
        ));
    }
    let mut constants = HashMap::new();
    let mut next = 0_i64;
    for (name, value) in &definition.constants {
        let explicit = value.is_some();
        let value = match value {
            Some(expression) => const_eval(expression, params, module_name)?,
            None => next,
        };
        if explicit && (value < 0 || (value as u64) >= (1_u64 << width)) {
            return Err(format!(
                "enum constant '{}::{}' value {} does not fit width {}",
                definition.name, name, value, width
            ));
        }
        let mask: u64 = if width == 64 {
            u64::MAX
        } else {
            (1_u64 << width) - 1
        };
        let stored = value & mask as i64;
        constants.insert(name.clone(), stored);
        next = stored.wrapping_add(1) & mask as i64;
    }
    Ok(ResolvedEnumType { width, constants })
}

fn resolve_type(
    definition: &ast::TypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedType, String> {
    match definition {
        ast::TypeDef::Enum(definition) => Ok(ResolvedType::Enum(resolve_enum_type(
            definition,
            params,
            module_name,
        )?)),
        ast::TypeDef::Struct(definition) => Ok(ResolvedType::Struct(resolve_struct_type(
            definition,
            params,
            module_name,
            existing_types,
        )?)),
        ast::TypeDef::Union(definition) => Ok(ResolvedType::Union(resolve_union_type(
            definition,
            params,
            module_name,
            existing_types,
        )?)),
    }
}

fn resolve_struct_type(
    definition: &ast::StructTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedStructType, String> {
    let mut field_widths = Vec::new();
    let mut width = 0_usize;
    for field in &definition.fields {
        let field_width = if let Some((msb, lsb)) = &field.range {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            (hi - lo).unsigned_abs() as usize + 1
        } else {
            let type_name = field
                .type_name
                .as_deref()
                .ok_or_else(|| format!("struct field '{}' has no width", field.name))?;
            existing_types
                .get(type_name)
                .map(ResolvedType::width)
                .ok_or_else(|| {
                    format!(
                        "struct typedef '{}': field '{}' uses unknown type '{}'",
                        definition.name, field.name, type_name
                    )
                })?
        };
        if field_width == 0 || width + field_width > 4096 {
            return Err(format!(
                "struct typedef '{}' exceeds the packed-width limit",
                definition.name
            ));
        }
        width += field_width;
        field_widths.push(field_width);
    }
    let mut fields = HashMap::new();
    let mut consumed = 0_usize;
    for (field, field_width) in definition.fields.iter().zip(field_widths) {
        let offset = width - consumed - field_width;
        fields.insert(field.name.clone(), (offset, field_width));
        consumed += field_width;
    }
    Ok(ResolvedStructType { width, fields })
}

fn resolve_union_type(
    definition: &ast::StructTypeDef,
    params: &HashMap<String, i64>,
    module_name: &str,
    existing_types: &HashMap<String, ResolvedType>,
) -> Result<ResolvedStructType, String> {
    let mut fields = HashMap::new();
    let mut width = 0_usize;
    for field in &definition.fields {
        let field_width = if let Some((msb, lsb)) = &field.range {
            let hi = const_eval(msb, params, module_name)?;
            let lo = const_eval(lsb, params, module_name)?;
            (hi - lo).unsigned_abs() as usize + 1
        } else {
            let type_name = field
                .type_name
                .as_deref()
                .ok_or_else(|| format!("union field '{}' has no width", field.name))?;
            existing_types
                .get(type_name)
                .map(ResolvedType::width)
                .ok_or_else(|| {
                    format!(
                        "union typedef '{}': field '{}' uses unknown type '{}'",
                        definition.name, field.name, type_name
                    )
                })?
        };
        if field_width == 0 || field_width > 4096 {
            return Err(format!(
                "union typedef '{}' exceeds the packed-width limit",
                definition.name
            ));
        }
        width = width.max(field_width);
        fields.insert(field.name.clone(), (0, field_width));
    }
    Ok(ResolvedStructType { width, fields })
}

fn package_const_eval(
    expression: &ast::Expr,
    packages: &PackageTables,
    current_package: &str,
    current_values: &HashMap<String, i64>,
) -> Result<i64, String> {
    match expression {
        ast::Expr::ScopedIdent { package, name } => packages
            .constants
            .get(package)
            .and_then(|values| values.get(name))
            .copied()
            .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name)),
        ast::Expr::Ident(name) if current_values.contains_key(name) => Ok(current_values[name]),
        other => const_eval_packages(other, current_values, current_package, packages),
    }
}

fn substitute_generate_var(item: &ast::Item, var: &str, value: i64) -> ast::Item {
    if value < 0 {
        return item.clone();
    }
    match item {
        ast::Item::Params(list) => ast::Item::Params(
            list.iter()
                .map(|(name, expr)| (name.clone(), subst_expr(expr, var, value)))
                .collect(),
        ),
        ast::Item::Imports(_) => item.clone(),
        ast::Item::TypeDef(_) => item.clone(),
        ast::Item::ClassDef(_) => item.clone(),
        ast::Item::Function(_) => item.clone(),
        ast::Item::Modport { .. } => item.clone(),
        ast::Item::AssertProperty {
            kind,
            label,
            clock,
            disable,
            antecedent,
            consequent,
            composition,
            overlapping,
            delay,
            repetition,
            message,
        } => ast::Item::AssertProperty {
            kind: *kind,
            label: label.clone(),
            clock: subst_sens(clock, var, value),
            disable: subst_expr(disable, var, value),
            antecedent: subst_expr(antecedent, var, value),
            consequent: subst_expr(consequent, var, value),
            composition: composition.as_ref().map(|composition| {
                Box::new(ast::PropertyComposition {
                    op: composition.op,
                    right: subst_expr(&composition.right, var, value),
                    right_delay: match &composition.right_delay {
                        ast::PropertyDelay::Fixed(delay) => {
                            ast::PropertyDelay::Fixed(subst_expr(delay, var, value))
                        }
                        ast::PropertyDelay::Range(start, end) => ast::PropertyDelay::Range(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        ),
                    },
                })
            }),
            overlapping: *overlapping,
            delay: Box::new(match delay.as_ref() {
                ast::PropertyDelay::Fixed(delay) => {
                    ast::PropertyDelay::Fixed(subst_expr(delay, var, value))
                }
                ast::PropertyDelay::Range(start, end) => ast::PropertyDelay::Range(
                    subst_expr(start, var, value),
                    subst_expr(end, var, value),
                ),
            }),
            repetition: repetition.as_deref().map(|repetition| {
                Box::new(match repetition {
                    ast::PropertyRepetition::Count(count) => {
                        ast::PropertyRepetition::Count(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::Range(start, end) => ast::PropertyRepetition::Range(
                        subst_expr(start, var, value),
                        subst_expr(end, var, value),
                    ),
                    ast::PropertyRepetition::Goto(count) => {
                        ast::PropertyRepetition::Goto(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::GotoRange(start, end) => {
                        ast::PropertyRepetition::GotoRange(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        )
                    }
                    ast::PropertyRepetition::Nonconsecutive(count) => {
                        ast::PropertyRepetition::Nonconsecutive(subst_expr(count, var, value))
                    }
                    ast::PropertyRepetition::NonconsecutiveRange(start, end) => {
                        ast::PropertyRepetition::NonconsecutiveRange(
                            subst_expr(start, var, value),
                            subst_expr(end, var, value),
                        )
                    }
                })
            }),
            message: message.clone(),
        },
        ast::Item::Generate(constructs) => ast::Item::Generate(
            constructs
                .iter()
                .map(|construct| subst_generate_construct(construct, var, value))
                .collect(),
        ),
        ast::Item::Port {
            dir,
            is_reg,
            is_signed,
            is_virtual,
            interface_name,
            modport_name,
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Item::Port {
            dir: *dir,
            is_reg: *is_reg,
            is_signed: *is_signed,
            is_virtual: *is_virtual,
            interface_name: interface_name.clone(),
            modport_name: modport_name.clone(),
            type_name: type_name.clone(),
            range: range.as_ref().map(|(a, b)| {
                (
                    Box::new(subst_expr(a, var, value)),
                    Box::new(subst_expr(b, var, value)),
                )
            }),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(a, b)| {
                        (
                            Box::new(subst_expr(a, var, value)),
                            Box::new(subst_expr(b, var, value)),
                        )
                    })
                })
                .collect(),
            names: names.clone(),
        },
        ast::Item::Net {
            is_reg,
            is_signed,
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Item::Net {
            is_reg: *is_reg,
            is_signed: *is_signed,
            type_name: type_name.clone(),
            range: range.as_ref().map(|(a, b)| {
                (
                    Box::new(subst_expr(a, var, value)),
                    Box::new(subst_expr(b, var, value)),
                )
            }),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(a, b)| {
                        (
                            Box::new(subst_expr(a, var, value)),
                            Box::new(subst_expr(b, var, value)),
                        )
                    })
                })
                .collect(),
            names: names
                .iter()
                .map(|(name, init)| {
                    (
                        name.clone(),
                        init.as_ref().map(|init| subst_expr(init, var, value)),
                    )
                })
                .collect(),
        },
        ast::Item::Assign { delay, lv, e } => ast::Item::Assign {
            delay: *delay,
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Item::Always { sens, body } => ast::Item::Always {
            sens: subst_sens(sens, var, value),
            body: subst_stmt(body, var, value),
        },
        ast::Item::Initial { body } => ast::Item::Initial {
            body: subst_stmt(body, var, value),
        },
        ast::Item::Gate { kind, name, args } => ast::Item::Gate {
            kind: *kind,
            name: name.clone(),
            args: args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
        },
        ast::Item::Inst {
            module,
            name,
            array_ranges,
            params,
            conns,
        } => ast::Item::Inst {
            module: module.clone(),
            name: name.clone(),
            array_ranges: array_ranges
                .iter()
                .map(|(msb, lsb)| {
                    (
                        Box::new(subst_expr(msb, var, value)),
                        Box::new(subst_expr(lsb, var, value)),
                    )
                })
                .collect(),
            params: params
                .iter()
                .map(|(name, value_expr)| (name.clone(), subst_expr(value_expr, var, value)))
                .collect(),
            conns: conns
                .iter()
                .map(|conn| match conn {
                    ast::Conn::Named(name, expr) => ast::Conn::Named(
                        name.clone(),
                        expr.as_ref().map(|expr| subst_expr(expr, var, value)),
                    ),
                    ast::Conn::Pos(expr) => {
                        ast::Conn::Pos(expr.as_ref().map(|expr| subst_expr(expr, var, value)))
                    }
                })
                .collect(),
        },
    }
}

fn subst_generate_construct(
    construct: &ast::GenerateConstruct,
    var: &str,
    value: i64,
) -> ast::GenerateConstruct {
    match construct {
        ast::GenerateConstruct::Block(block) => {
            ast::GenerateConstruct::Block(subst_generate_block(block, var, value))
        }
        ast::GenerateConstruct::Case {
            selector,
            arms,
            default,
        } => ast::GenerateConstruct::Case {
            selector: subst_expr(selector, var, value),
            arms: arms
                .iter()
                .map(|arm| ast::GenerateCaseArm {
                    labels: arm
                        .labels
                        .iter()
                        .map(|label| subst_expr(label, var, value))
                        .collect(),
                    body: subst_generate_block(&arm.body, var, value),
                })
                .collect(),
            default: default
                .as_ref()
                .map(|block| subst_generate_block(block, var, value)),
        },
        ast::GenerateConstruct::If {
            cond,
            then,
            otherwise,
        } => ast::GenerateConstruct::If {
            cond: subst_expr(cond, var, value),
            then: subst_generate_block(then, var, value),
            otherwise: otherwise
                .as_ref()
                .map(|block| subst_generate_block(block, var, value)),
        },
        ast::GenerateConstruct::For {
            var: loop_var,
            start,
            cond,
            step,
            body,
        } => ast::GenerateConstruct::For {
            var: loop_var.clone(),
            start: subst_expr(start, var, value),
            cond: subst_expr(cond, var, value),
            step: subst_expr(step, var, value),
            body: subst_generate_block(body, var, value),
        },
    }
}

fn subst_generate_block(block: &ast::GenerateBlock, var: &str, value: i64) -> ast::GenerateBlock {
    ast::GenerateBlock {
        label: block.label.clone(),
        items: block
            .items
            .iter()
            .map(|item| substitute_generate_var(item, var, value))
            .collect(),
    }
}

fn subst_expr(expr: &ast::Expr, var: &str, value: i64) -> ast::Expr {
    match expr {
        ast::Expr::Ident(name) if name == var => ast::Expr::Num {
            width: Some(32),
            word: LogicWord::from_u64(32, value as u64),
            orig: value.to_string(),
        },
        ast::Expr::Ident(_)
        | ast::Expr::ScopedIdent { .. }
        | ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::MethodCall { .. }
        | ast::Expr::MethodCallChain { .. }
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expr.clone(),
        ast::Expr::New(size) => ast::Expr::New(Box::new(subst_expr(size, var, value))),
        ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
            arguments
                .iter()
                .map(|argument| subst_expr(argument, var, value))
                .collect(),
        ),
        ast::Expr::Select(name, index) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::Select(name.clone(), Box::new(subst_expr(index, var, value)))
        }
        ast::Expr::ArraySelect(name, indices) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::ArraySelect(
                name.clone(),
                indices
                    .iter()
                    .map(|index| subst_expr(index, var, value))
                    .collect(),
            )
        }
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::ArrayIndexedPart {
                name: name.clone(),
                indexes: indexes
                    .iter()
                    .map(|index| subst_expr(index, var, value))
                    .collect(),
                start: Box::new(subst_expr(start, var, value)),
                width: Box::new(subst_expr(width, var, value)),
                ascending: *ascending,
            }
        }
        ast::Expr::Part(name, msb, lsb) => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::Part(
                name.clone(),
                Box::new(subst_expr(msb, var, value)),
                Box::new(subst_expr(lsb, var, value)),
            )
        }
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => {
            if name == var {
                return subst_expr(&ast::Expr::Ident(name.clone()), var, value);
            }
            ast::Expr::IndexedPart {
                name: name.clone(),
                start: Box::new(subst_expr(start, var, value)),
                width: Box::new(subst_expr(width, var, value)),
                ascending: *ascending,
            }
        }
        ast::Expr::Un(op, a) => ast::Expr::Un(*op, Box::new(subst_expr(a, var, value))),
        ast::Expr::Bin(op, a, b) => ast::Expr::Bin(
            *op,
            Box::new(subst_expr(a, var, value)),
            Box::new(subst_expr(b, var, value)),
        ),
        ast::Expr::Cond(c, t, f) => ast::Expr::Cond(
            Box::new(subst_expr(c, var, value)),
            Box::new(subst_expr(t, var, value)),
            Box::new(subst_expr(f, var, value)),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| subst_expr(part, var, value))
                .collect(),
        ),
        ast::Expr::Replicate(count, value_expr) => ast::Expr::Replicate(
            Box::new(subst_expr(count, var, value)),
            Box::new(subst_expr(value_expr, var, value)),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
        ),
        ast::Expr::Slice {
            value: expression,
            msb,
            lsb,
        } => ast::Expr::Slice {
            value: Box::new(subst_expr(expression, var, value)),
            msb: Box::new(subst_expr(msb, var, value)),
            lsb: Box::new(subst_expr(lsb, var, value)),
        },
        ast::Expr::Resize {
            width,
            value: expression,
        } => ast::Expr::Resize {
            width: *width,
            value: Box::new(subst_expr(expression, var, value)),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(subst_expr(selector, var, value)),
            label: Box::new(subst_expr(label, var, value)),
        },
    }
}

fn subst_sens(sens: &ast::Sens, var: &str, value: i64) -> ast::Sens {
    match sens {
        ast::Sens::Comb => ast::Sens::Comb,
        ast::Sens::Posedge(e) => ast::Sens::Posedge(subst_expr(e, var, value)),
        ast::Sens::Negedge(e) => ast::Sens::Negedge(subst_expr(e, var, value)),
        ast::Sens::List(list) => {
            ast::Sens::List(list.iter().map(|s| subst_sens(s, var, value)).collect())
        }
    }
}

fn validate_function_body(definition: &ast::FunctionDef) -> Result<(), String> {
    let mut known = HashSet::new();
    for (name, _) in &definition.args {
        if name == &definition.name || !known.insert(name.clone()) {
            return Err(format!(
                "function '{}': argument '{}' shadows the return or a prior argument",
                definition.name, name
            ));
        }
    }
    let mut has_return_assignment = false;
    fn visit(
        statement: &ast::Stmt,
        definition: &ast::FunctionDef,
        known: &mut HashSet<String>,
        allow_declarations: bool,
        has_return_assignment: &mut bool,
    ) -> Result<(), String> {
        match statement {
            ast::Stmt::Block(statements) => {
                for statement in statements {
                    visit(
                        statement,
                        definition,
                        known,
                        allow_declarations,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::Local { names, .. } => {
                if !allow_declarations {
                    return Err(format!(
                        "function '{}': local declarations are not supported inside branch control flow",
                        definition.name
                    ));
                }
                for (name, _) in names {
                    if name == &definition.name
                        || known.contains(name)
                        || !known.insert(name.clone())
                    {
                        return Err(format!(
                            "function '{}': local '{}' shadows the return, an argument, or a prior local",
                            definition.name, name
                        ));
                    }
                }
            }
            ast::Stmt::BL { lv, .. } => match lv {
                ast::Expr::Ident(name) => {
                    if name == &definition.name {
                        *has_return_assignment = true;
                    } else if !known.contains(name) {
                        return Err(format!(
                            "function '{}': assignment to undeclared local '{}'",
                            definition.name, name
                        ));
                    }
                }
                ast::Expr::Member(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared aggregate local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::Select(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::ArraySelect(object, _) => {
                    if !known.contains(object) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, object
                        ));
                    }
                }
                ast::Expr::ArrayIndexedPart { name, .. } => {
                    if !known.contains(name) {
                        return Err(format!(
                            "function '{}': assignment to undeclared array local '{}'",
                            definition.name, name
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "function '{}': unsupported assignment target {:?}",
                        definition.name, other
                    ));
                }
            },
            ast::Stmt::NBL { .. } => {
                return Err(format!(
                    "function '{}': non-blocking assignments are not supported in functions",
                    definition.name
                ));
            }
            ast::Stmt::If { t, f, .. } => {
                visit(t, definition, known, false, has_return_assignment)?;
                if let Some(false_branch) = f {
                    visit(
                        false_branch,
                        definition,
                        known,
                        false,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::Case { arms, dflt, .. } => {
                for (_, body) in arms {
                    visit(body, definition, known, false, has_return_assignment)?;
                }
                if let Some(default_body) = dflt {
                    visit(
                        default_body,
                        definition,
                        known,
                        false,
                        has_return_assignment,
                    )?;
                }
            }
            ast::Stmt::For { var, body, .. } => {
                if var == &definition.name || known.contains(var) {
                    return Err(format!(
                        "function '{}': loop variable '{}' shadows the return, an argument, or a local",
                        definition.name, var
                    ));
                }
                known.insert(var.clone());
                visit(body, definition, known, false, has_return_assignment)?;
                known.remove(var);
            }
            ast::Stmt::Repeat { body, .. } => {
                visit(body, definition, known, false, has_return_assignment)?;
            }
            ast::Stmt::While { body, .. } => {
                visit(body, definition, known, false, has_return_assignment)?;
            }
            other => {
                return Err(format!(
                    "function '{}': unsupported statement {:?} in function body",
                    definition.name, other
                ));
            }
        }
        Ok(())
    }
    for statement in &definition.body {
        visit(
            statement,
            definition,
            &mut known,
            true,
            &mut has_return_assignment,
        )?;
    }
    if !has_return_assignment {
        return Err(format!(
            "function '{}' never assigns its return value",
            definition.name
        ));
    }
    Ok(())
}

fn loop_value(value: i64) -> ast::Expr {
    ast::Expr::Num {
        width: Some(32),
        word: LogicWord::from_u64(32, value as u64),
        orig: value.to_string(),
    }
}

fn constant_expr(value: i64) -> ast::Expr {
    loop_value(value)
}

fn substitute_function_arguments(
    expression: &ast::Expr,
    arguments: &HashMap<String, ast::Expr>,
) -> ast::Expr {
    match expression {
        ast::Expr::Ident(name) => arguments
            .get(name.as_str())
            .cloned()
            .unwrap_or_else(|| expression.clone()),
        ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::MethodCall { .. }
        | ast::Expr::MethodCallChain { .. }
        | ast::Expr::ScopedIdent { .. }
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expression.clone(),
        ast::Expr::New(size) => {
            ast::Expr::New(Box::new(substitute_function_arguments(size, arguments)))
        }
        ast::Expr::NewClass(actuals) => ast::Expr::NewClass(
            actuals
                .iter()
                .map(|actual| substitute_function_arguments(actual, arguments))
                .collect(),
        ),
        ast::Expr::Select(name, index) => ast::Expr::Select(
            name.clone(),
            Box::new(substitute_function_arguments(index, arguments)),
        ),
        ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
            name.clone(),
            indexes
                .iter()
                .map(|index| substitute_function_arguments(index, arguments))
                .collect(),
        ),
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => ast::Expr::ArrayIndexedPart {
            name: name.clone(),
            indexes: indexes
                .iter()
                .map(|index| substitute_function_arguments(index, arguments))
                .collect(),
            start: Box::new(substitute_function_arguments(start, arguments)),
            width: Box::new(substitute_function_arguments(width, arguments)),
            ascending: *ascending,
        },
        ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
            name.clone(),
            Box::new(substitute_function_arguments(msb, arguments)),
            Box::new(substitute_function_arguments(lsb, arguments)),
        ),
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => ast::Expr::IndexedPart {
            name: name.clone(),
            start: Box::new(substitute_function_arguments(start, arguments)),
            width: Box::new(substitute_function_arguments(width, arguments)),
            ascending: *ascending,
        },
        ast::Expr::Un(op, value) => ast::Expr::Un(
            *op,
            Box::new(substitute_function_arguments(value, arguments)),
        ),
        ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
            *op,
            Box::new(substitute_function_arguments(left, arguments)),
            Box::new(substitute_function_arguments(right, arguments)),
        ),
        ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
            Box::new(substitute_function_arguments(condition, arguments)),
            Box::new(substitute_function_arguments(true_value, arguments)),
            Box::new(substitute_function_arguments(false_value, arguments)),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| substitute_function_arguments(part, arguments))
                .collect(),
        ),
        ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
            Box::new(substitute_function_arguments(count, arguments)),
            Box::new(substitute_function_arguments(value, arguments)),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter()
                .map(|arg| substitute_function_arguments(arg, arguments))
                .collect(),
        ),
        ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
            value: Box::new(substitute_function_arguments(value, arguments)),
            msb: Box::new(substitute_function_arguments(msb, arguments)),
            lsb: Box::new(substitute_function_arguments(lsb, arguments)),
        },
        ast::Expr::Resize { width, value } => ast::Expr::Resize {
            width: *width,
            value: Box::new(substitute_function_arguments(value, arguments)),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(substitute_function_arguments(selector, arguments)),
            label: Box::new(substitute_function_arguments(label, arguments)),
        },
    }
}

fn subst_stmt(stmt: &ast::Stmt, var: &str, value: i64) -> ast::Stmt {
    match stmt {
        ast::Stmt::Block(list) => {
            ast::Stmt::Block(list.iter().map(|s| subst_stmt(s, var, value)).collect())
        }
        ast::Stmt::Local {
            type_name,
            range,
            array_ranges,
            names,
        } => ast::Stmt::Local {
            type_name: type_name.clone(),
            array_ranges: array_ranges
                .iter()
                .map(|range| {
                    range.as_ref().map(|(msb, lsb)| {
                        (
                            Box::new(subst_expr(msb, var, value)),
                            Box::new(subst_expr(lsb, var, value)),
                        )
                    })
                })
                .collect(),
            range: range.as_ref().map(|(msb, lsb)| {
                (
                    Box::new(subst_expr(msb, var, value)),
                    Box::new(subst_expr(lsb, var, value)),
                )
            }),
            names: names
                .iter()
                .map(|(name, init)| {
                    (
                        name.clone(),
                        init.as_ref().map(|expr| subst_expr(expr, var, value)),
                    )
                })
                .collect(),
        },
        ast::Stmt::For {
            var,
            start,
            cond,
            step,
            body,
        } => ast::Stmt::For {
            var: var.clone(),
            start: subst_expr(start, var, value),
            cond: subst_expr(cond, var, value),
            step: subst_expr(step, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Repeat { count, body } => ast::Stmt::Repeat {
            count: subst_expr(count, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::While { cond, body } => ast::Stmt::While {
            cond: subst_expr(cond, var, value),
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Assert { cond, message } => ast::Stmt::Assert {
            cond: subst_expr(cond, var, value),
            message: message.clone(),
        },
        ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } => ast::Stmt::MethodCall {
            object: object.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| subst_expr(argument, var, value))
                .collect(),
        },
        ast::Stmt::Forever(s) => ast::Stmt::Forever(Box::new(subst_stmt(s, var, value))),
        ast::Stmt::NBL { lv, e } => ast::Stmt::NBL {
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Stmt::BL { lv, e } => ast::Stmt::BL {
            lv: subst_expr(lv, var, value),
            e: subst_expr(e, var, value),
        },
        ast::Stmt::If { c, t, f } => ast::Stmt::If {
            c: subst_expr(c, var, value),
            t: Box::new(subst_stmt(t, var, value)),
            f: f.as_ref().map(|s| Box::new(subst_stmt(s, var, value))),
        },
        ast::Stmt::Case {
            e,
            arms,
            dflt,
            kind,
        } => ast::Stmt::Case {
            e: subst_expr(e, var, value),
            arms: arms
                .iter()
                .map(|(labels, body)| {
                    (
                        labels
                            .iter()
                            .map(|label| subst_expr(label, var, value))
                            .collect(),
                        subst_stmt(body, var, value),
                    )
                })
                .collect(),
            dflt: dflt.as_ref().map(|s| Box::new(subst_stmt(s, var, value))),
            kind: *kind,
        },
        ast::Stmt::Delay { ns, body } => ast::Stmt::Delay {
            ns: *ns,
            body: Box::new(subst_stmt(body, var, value)),
        },
        ast::Stmt::Display { fmt, args, monitor } => ast::Stmt::Display {
            fmt: fmt.clone(),
            args: args.iter().map(|arg| subst_expr(arg, var, value)).collect(),
            monitor: *monitor,
        },
        ast::Stmt::Finish => ast::Stmt::Finish,
    }
}

fn wildcard_case_as_if(
    selector: ExprIR,
    arms: Vec<(ExprIR, StmtIR)>,
    default: Option<StmtIR>,
    kind: ast::CaseKind,
) -> StmtIR {
    let mut result = default.unwrap_or_else(|| StmtIR::Block(Vec::new()));
    for (label, body) in arms.into_iter().rev() {
        let condition = wildcard_label_condition(selector.clone(), &label, kind);
        result = StmtIR::If {
            c: condition,
            t: Box::new(body),
            f: Some(Box::new(result)),
        };
    }
    result
}

fn wildcard_label_condition(selector: ExprIR, label: &ExprIR, kind: ast::CaseKind) -> ExprIR {
    let width = selector.width().max(label.width());
    let mut conditions = Vec::new();
    for bit in (0..width).rev() {
        let selector_bit = if bit < selector.width() {
            ExprIR::Index {
                e: Box::new(selector.clone()),
                msb: bit,
                lsb: bit,
            }
        } else {
            ExprIR::Const(LogicWord::zero(1))
        };
        let label_bit = label_bit(label, bit, width);
        let wildcard = match kind {
            ast::CaseKind::Exact => ExprIR::Const(LogicWord::zero(1)),
            ast::CaseKind::CaseZ => {
                let selector_is_z = case_equal(selector_bit.clone(), const_bit("'bz"));
                let label_is_z = case_equal(label_bit.clone(), const_bit("'bz"));
                bit_or(selector_is_z, label_is_z)
            }
            ast::CaseKind::CaseX => {
                let selector_is_z = case_equal(selector_bit.clone(), const_bit("'bz"));
                let selector_is_x = case_equal(selector_bit.clone(), const_bit("'bx"));
                let label_is_z = case_equal(label_bit.clone(), const_bit("'bz"));
                let label_is_x = case_equal(label_bit.clone(), const_bit("'bx"));
                bit_or(
                    bit_or(selector_is_z, selector_is_x),
                    bit_or(label_is_z, label_is_x),
                )
            }
        };
        let equal = case_equal(selector_bit, label_bit);
        conditions.push(bit_or(wildcard, equal));
    }
    conditions
        .into_iter()
        .reduce(bit_and)
        .unwrap_or_else(|| ExprIR::Const(LogicWord::ones(1)))
}

fn label_bit(label: &ExprIR, bit: usize, width: usize) -> ExprIR {
    if bit < label.width() {
        ExprIR::Index {
            e: Box::new(label.clone()),
            msb: bit,
            lsb: bit,
        }
    } else {
        let _ = width;
        ExprIR::Const(LogicWord::zero(1))
    }
}

fn const_bit(text: &str) -> ExprIR {
    ExprIR::Const(LogicWord::parse(1, text).expect("one-bit case literal"))
}

fn case_equal(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::CaseEq,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn bit_or(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::Or,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn bit_and(left: ExprIR, right: ExprIR) -> ExprIR {
    ExprIR::Bin {
        op: BinOp::And,
        a: Box::new(left),
        b: Box::new(right),
    }
}

fn flatten_generate_items(
    items: &[ast::Item],
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
) -> Result<Vec<ast::Item>, String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            ast::Item::Generate(constructs) => {
                for construct in constructs {
                    flatten_generate_construct(construct, params, module_name, packages, &mut out)?;
                }
            }
            other => out.push(other.clone()),
        }
    }
    Ok(out)
}

fn flatten_generate_construct(
    construct: &ast::GenerateConstruct,
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
    out: &mut Vec<ast::Item>,
) -> Result<(), String> {
    match construct {
        ast::GenerateConstruct::Block(block) => {
            let expanded = flatten_generate_items(&block.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::Case {
            selector,
            arms,
            default,
        } => {
            let selector_value = const_eval_packages(selector, params, module_name, packages)?;
            let mut selected = None;
            for arm in arms {
                for label in &arm.labels {
                    if const_eval_packages(label, params, module_name, packages)? == selector_value
                    {
                        selected = Some(&arm.body);
                        break;
                    }
                }
                if selected.is_some() {
                    break;
                }
            }
            let selected = selected.or(default.as_ref()).ok_or_else(|| {
                format!(
                    "module '{}': generate case selector {selector_value} has no matching item or default",
                    module_name
                )
            })?;
            let expanded = flatten_generate_items(&selected.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::If {
            cond,
            then,
            otherwise,
        } => {
            let selected = if const_eval_packages(cond, params, module_name, packages)? != 0 {
                then
            } else {
                otherwise
                    .as_ref()
                    .ok_or_else(|| "generate if has no else branch".to_string())?
            };
            let expanded = flatten_generate_items(&selected.items, params, module_name, packages)?;
            out.extend(expanded);
        }
        ast::GenerateConstruct::For {
            var,
            start,
            cond,
            step,
            body,
        } => {
            let mut value = const_eval_packages(start, params, module_name, packages)?;
            let mut loop_params = params.clone();
            for _ in 0..10_000 {
                loop_params.insert(var.clone(), value);
                if const_eval_packages(cond, &loop_params, module_name, packages)? == 0 {
                    break;
                }
                let replaced: Vec<_> = body
                    .items
                    .iter()
                    .map(|item| substitute_generate_var(item, var, value))
                    .collect();
                let expanded =
                    flatten_generate_items(&replaced, &loop_params, module_name, packages)?;
                out.extend(expanded);
                let next = const_eval_packages(step, &loop_params, module_name, packages)?;
                if next <= value {
                    return Err(format!(
                        "module '{}': generate for-loop does not advance '{var}'",
                        module_name
                    ));
                }
                value = next;
            }
        }
    }
    Ok(())
}

impl<'a> Elaborator<'a> {
    fn apply_import(
        &self,
        import: &ast::Import,
        params: &mut HashMap<String, i64>,
        types: &mut HashMap<String, ResolvedType>,
        functions: &mut HashMap<String, ast::FunctionDef>,
        parent_params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<(), String> {
        let values = self
            .packages
            .constants
            .get(&import.package)
            .ok_or_else(|| {
                format!(
                    "module '{}': unknown import package '{}'",
                    module_name, import.package
                )
            })?;
        let package_types = self.packages.types.get(&import.package).ok_or_else(|| {
            format!(
                "module '{}': import package '{}' has no type table",
                module_name, import.package
            )
        })?;
        let package_functions = self
            .packages
            .functions
            .get(&import.package)
            .ok_or_else(|| {
                format!(
                    "module '{}': import package '{}' has no function table",
                    module_name, import.package
                )
            })?;
        if import.names.is_empty() {
            for (name, value) in values {
                if !parent_params.contains_key(name) {
                    params.insert(name.clone(), *value);
                }
            }
            for (name, definition) in package_types {
                types.insert(name.clone(), definition.clone());
            }
            for (name, definition) in package_functions {
                functions.insert(name.clone(), definition.clone());
            }
        } else {
            for name in &import.names {
                if let Some(value) = values.get(name).copied() {
                    if !parent_params.contains_key(name) {
                        params.insert(name.clone(), value);
                    }
                } else if let Some(definition) = package_types.get(name).cloned() {
                    if let ResolvedType::Enum(enum_definition) = &definition {
                        for (constant, value) in &enum_definition.constants {
                            if !parent_params.contains_key(constant) {
                                params.insert(constant.clone(), *value);
                            }
                        }
                    }
                    types.insert(name.clone(), definition);
                } else if let Some(definition) = package_functions.get(name).cloned() {
                    functions.insert(name.clone(), definition);
                } else {
                    return Err(format!(
                        "module '{}': package '{}' has no imported name '{}'",
                        module_name, import.package, name
                    ));
                }
            }
        }
        Ok(())
    }

    fn elab_module(
        &mut self,
        module_name: &str,
        prefix: &str,
        parent_params: &HashMap<String, i64>,
        interface_ports: &HashMap<String, HashMap<String, NetEntry>>,
    ) -> Result<(), String> {
        if self.depth > 64 {
            return Err("module instantiation too deep (recursive instance?)".into());
        }
        let module = self.mods[module_name];
        self.depth += 1;
        let r = self.elab_module_inner(module, prefix, parent_params, interface_ports);
        self.depth -= 1;
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn declare_net(
        &mut self,
        name: &str,
        prefix: &str,
        range: &Option<(Box<ast::Expr>, Box<ast::Expr>)>,
        params: &HashMap<String, i64>,
        is_reg: bool,
        is_signed: bool,
        type_width: Option<usize>,
        type_name: Option<&str>,
    ) -> Result<NetEntry, String> {
        let (msb, lsb) = match (range, type_width) {
            (Some((a, b)), _) => (
                self.const_eval(a, params, "declaration")?,
                self.const_eval(b, params, "declaration")?,
            ),
            (None, Some(width)) => (width as i64 - 1, 0),
            (None, None) => (0, 0),
        };
        let width = (msb - lsb).unsigned_abs() as usize + 1;
        let base = self.design.n_nets;
        self.design.net_decls.push(NetDecl {
            name: format!("{}{}", prefix, name),
            width,
            is_reg,
            is_signed,
            base,
        });
        self.design.n_nets += width;
        Ok(NetEntry {
            base,
            width,
            msb,
            lsb,
            type_name: type_name.map(str::to_string),
            is_signed,
            array: None,
        })
    }

    fn declaration_type_width(
        &self,
        type_name: Option<&str>,
        local_types: &HashMap<String, ResolvedType>,
        module_name: &str,
    ) -> Result<Option<usize>, String> {
        let Some(name) = type_name else {
            return Ok(None);
        };
        if self.classes.contains_key(name) {
            return Ok(Some(1));
        }
        if name == "string" {
            return Ok(Some(ast::STRING_WIDTH));
        }
        local_types
            .get(name)
            .map(|definition| Some(definition.width()))
            .ok_or_else(|| {
                format!(
                    "module '{}': unknown type definition '{}'",
                    module_name, name
                )
            })
    }

    fn indexed_part_range(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<(usize, usize), String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return Err(format!(
                "indexed part-select on reversed packed range [{}:{}] is not yet supported",
                entry.msb, entry.lsb
            ));
        }
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        let (high, low) = if ascending {
            (start + width as i64 - 1, start)
        } else {
            (start, start - width as i64 + 1)
        };
        let high_offset = entry.offset(high)?;
        let low_offset = entry.offset(low)?;
        Ok((high_offset.max(low_offset), high_offset.min(low_offset)))
    }

    fn reversed_indexed_part_lvalue(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<LValue, String> {
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let logical_start = if ascending {
            start
        } else {
            start - width as i64 + 1
        };
        let logical_end = logical_start + width as i64 - 1;
        let high_offset = entry.offset(logical_end)?;
        let low_offset = entry.offset(logical_start)?;
        Ok(LValue {
            net: entry.base,
            decl_width: entry.width,
            range: Range::Span {
                msb: high_offset.max(low_offset),
                lsb: high_offset.min(low_offset),
            },
        })
    }

    fn array_element_location(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<(NetEntry, usize, usize), String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        let mut resolved = Vec::with_capacity(indexes.len());
        for index in indexes {
            resolved.push(self.const_eval(index, env.params, env.module_name)?);
        }
        let offset = entry.array_offset(&resolved)?;
        Ok((entry.clone(), offset, array.element_width))
    }

    fn array_element_indexed_expr(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if indexes
            .iter()
            .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
        {
            return self
                .dynamic_array_element_indexed_expr(name, indexes, start, width, ascending, env);
        }
        let (entry, offset, element_width) = self.array_element_location(name, indexes, env)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }

        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        };
        let normalized_source = if reversed {
            ExprIR::Concat(
                (offset..offset + element_width)
                    .map(|physical_bit| ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Bit(physical_bit),
                        signed: false,
                    })
                    .collect(),
            )
        } else {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: Range::Span {
                    msb: offset + element_width - 1,
                    lsb: offset,
                },
                signed: false,
            }
        };
        let max_offset = element_width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(normalized_source),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn dynamic_array_element_indexed_expr(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        let element_width = array.element_width;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }

        let runtime_indexes = indexes
            .iter()
            .map(|index| self.elab_expr(index, env))
            .collect::<Result<Vec<_>, _>>()?;
        let mut selected_element = ExprIR::Const(LogicWord::xes(element_width));
        let mut any_index_valid = ExprIR::Const(LogicWord::from_u64(1, 0));
        for physical_element in 0..array.length {
            let mut physical_offset = physical_element;
            let mut scale = array.length;
            let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
            for dimension in &array.dimensions {
                scale /= dimension.length;
                let dimension_offset = physical_offset / scale;
                physical_offset %= scale;
                logical_indexes.push(dimension.base + dimension.step * dimension_offset as i64);
            }

            let mut candidate_valid = ExprIR::Const(LogicWord::from_u64(1, 1));
            for (runtime_index, logical_index) in runtime_indexes.iter().zip(&logical_indexes) {
                let equals = ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(runtime_index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(
                        32,
                        *logical_index as u64,
                    ))),
                };
                candidate_valid = ExprIR::Bin {
                    op: BinOp::And,
                    a: Box::new(candidate_valid),
                    b: Box::new(equals),
                };
            }
            any_index_valid = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(any_index_valid),
                b: Box::new(candidate_valid.clone()),
            };

            let physical_lsb = physical_element * element_width;
            let candidate = if array.element_msb < array.element_lsb {
                ExprIR::Concat(
                    (physical_lsb..physical_lsb + element_width)
                        .map(|physical_bit| ExprIR::Ref {
                            net: entry.base,
                            decl_width: entry.width,
                            range: Range::Bit(physical_bit),
                            signed: false,
                        })
                        .collect(),
                )
            } else {
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: physical_lsb + element_width - 1,
                        lsb: physical_lsb,
                    },
                    signed: false,
                }
            };
            selected_element = ExprIR::Cond {
                c: Box::new(candidate_valid),
                t: Box::new(candidate),
                f: Box::new(selected_element),
            };
        }

        let (element_msb, element_lsb) = (array.element_msb, array.element_lsb);
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        };
        let max_offset = element_width - width;
        let packed_in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let packed_selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(selected_element),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        let packed_result = ExprIR::Cond {
            c: Box::new(packed_in_range),
            t: Box::new(packed_selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(any_index_valid),
            t: Box::new(packed_result),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn array_element_indexed_span(
        &self,
        entry: &NetEntry,
        offset: usize,
        element_width: usize,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<(usize, usize, bool), String> {
        let start = self.const_eval(start, env.params, env.module_name)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }
        let logical_start = if ascending {
            start
        } else {
            start - width as i64 + 1
        };
        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        let normalized_start = logical_start - logical_base;
        if normalized_start < 0 || normalized_start + width as i64 > element_width as i64 {
            return Err(format!(
                "indexed part-select [{logical_start}+:{}] is out of array-element range",
                width
            ));
        }
        let normalized_start = normalized_start as usize;
        let physical_lsb = if reversed {
            offset + element_width - 1 - (normalized_start + width - 1)
        } else {
            offset + normalized_start
        };
        Ok((physical_lsb, width, reversed))
    }

    fn array_element_indexed_start(
        &self,
        entry: &NetEntry,
        _element_width: usize,
        start: &ast::Expr,
        width: usize,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let (element_msb, element_lsb) = entry
            .array
            .as_ref()
            .map(|array| (array.element_msb, array.element_lsb))
            .unwrap_or((entry.msb, entry.lsb));
        let reversed = element_msb < element_lsb;
        let logical_base = if reversed { element_msb } else { element_lsb };
        Ok(ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_base as u64))),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn array_element_indexed_statement(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        if indexes
            .iter()
            .any(|index| self.const_eval(index, env.params, env.module_name).is_err())
        {
            return self.runtime_array_element_indexed_statement(
                name,
                indexes,
                start,
                width,
                ascending,
                value,
                nonblocking,
                env,
            );
        }
        let (entry, base_offset, element_width) =
            self.array_element_location(name, indexes, env)?;
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > element_width {
            return Err(format!(
                "indexed part-select width {width} exceeds array element width {element_width}"
            ));
        }
        let start =
            self.array_element_indexed_start(&entry, element_width, start, width, ascending, env)?;
        let value = self.elab_expr(value, env)?;
        let value = Self::resize_indexed_value(value, width);
        let reversed = entry
            .array
            .as_ref()
            .map(|array| array.element_msb < array.element_lsb)
            .unwrap_or_else(|| entry.msb < entry.lsb);
        let common = (
            entry.base,
            entry.width,
            base_offset,
            element_width,
            start,
            width,
        );
        if nonblocking {
            Ok(StmtIR::NBlIndexed {
                net: common.0,
                decl_width: common.1,
                base_offset: common.2,
                element_width: common.3,
                start: common.4,
                width: common.5,
                ascending: true,
                reversed,
                e: value,
            })
        } else {
            Ok(StmtIR::BlIndexed {
                net: common.0,
                decl_width: common.1,
                base_offset: common.2,
                element_width: common.3,
                start: common.4,
                width: common.5,
                ascending: true,
                reversed,
                e: value,
            })
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn runtime_array_element_indexed_statement(
        &self,
        name: &str,
        indexes: &[ast::Expr],
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .as_ref()
            .ok_or_else(|| format!("'{name}' is not an unpacked array"))?;
        if indexes.len() != array.dimensions.len() {
            return Err(format!(
                "array expects {} index(es), received {}",
                array.dimensions.len(),
                indexes.len()
            ));
        }
        let runtime_indexes = indexes
            .iter()
            .map(|index| self.elab_expr(index, env))
            .collect::<Result<Vec<_>, _>>()?;

        let mut result = None;
        for physical_element in 0..array.length {
            let mut physical_offset = physical_element;
            let mut scale = array.length;
            let mut constant_indexes = Vec::with_capacity(array.dimensions.len());
            let mut candidate_valid = ExprIR::Const(LogicWord::from_u64(1, 1));
            for dimension in &array.dimensions {
                scale /= dimension.length;
                let dimension_offset = physical_offset / scale;
                physical_offset %= scale;
                let logical_index = dimension.base + dimension.step * dimension_offset as i64;
                candidate_valid = ExprIR::Bin {
                    op: BinOp::And,
                    a: Box::new(candidate_valid),
                    b: Box::new(ExprIR::Bin {
                        op: BinOp::Eq,
                        a: Box::new(runtime_indexes[constant_indexes.len()].clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(32, logical_index as u64))),
                    }),
                };
                constant_indexes.push(constant_expr(logical_index));
            }
            let candidate = self.array_element_indexed_statement(
                name,
                &constant_indexes,
                start,
                width,
                ascending,
                value,
                nonblocking,
                env,
            )?;
            result = Some(StmtIR::If {
                c: candidate_valid,
                t: Box::new(candidate),
                f: result.map(Box::new),
            });
        }
        result.ok_or_else(|| format!("array '{name}' has no elements"))
    }

    fn dynamic_indexed_part_expr(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return self.reversed_indexed_part_expr(entry, start, width, ascending, env);
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let start = if entry.lsb == 0 {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    32,
                    entry.lsb.unsigned_abs(),
                ))),
            }
        };
        let offset = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let max_offset = entry.width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(offset.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(offset.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                    signed: false,
                }),
                b: Box::new(offset),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    fn reversed_indexed_part_expr(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        let normalized_start = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, entry.msb as u64))),
        };

        // A reversed declaration stores logical index `msb` in the highest
        // physical bit. Concatenating physical bits in ascending offset order
        // normalizes bit 0 to that logical left bound.
        let normalized_source = ExprIR::Concat(
            (0..entry.width)
                .map(|offset| ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Bit(offset),
                    signed: false,
                })
                .collect(),
        );
        let max_offset = entry.width - width;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::zero(32))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(normalized_start.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, max_offset as u64))),
            }),
        };
        let selected = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(normalized_source),
                b: Box::new(normalized_start),
            }),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(
                width,
                (1_u64 << width) - 1,
            ))),
        };
        Ok(ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(selected),
            f: Box::new(ExprIR::Const(LogicWord::xes(width))),
        })
    }

    fn dynamic_indexed_part_nbl(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            let reversed_width = self.const_eval(width, env.params, env.module_name)?;
            if reversed_width <= 0 {
                return Err(format!(
                    "indexed part-select width must be positive, got {reversed_width}"
                ));
            }
            let reversed_width = reversed_width as usize;
            let start =
                self.reversed_indexed_start(entry, start, reversed_width, ascending, env)?;
            let value = self.elab_expr(value, env)?;
            let value = Self::resize_indexed_value(value, reversed_width);
            return Ok(StmtIR::NBlIndexed {
                net: entry.base,
                decl_width: entry.width,
                base_offset: 0,
                element_width: entry.width,
                start,
                width: reversed_width,
                ascending: true,
                reversed: true,
                e: value,
            });
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let start = self.elab_expr(start, env)?;
        let start = if entry.lsb == 0 {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    32,
                    entry.lsb.unsigned_abs(),
                ))),
            }
        };
        let value = self.elab_expr(value, env)?;
        let value = if value.width() < width {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(width - value.width())),
                value,
            ])
        } else if value.width() > width {
            ExprIR::Index {
                e: Box::new(value),
                msb: width - 1,
                lsb: 0,
            }
        } else {
            value
        };
        Ok(StmtIR::NBlIndexed {
            net: entry.base,
            decl_width: entry.width,
            base_offset: 0,
            element_width: entry.width,
            start,
            width,
            ascending,
            reversed: false,
            e: value,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn dynamic_indexed_part_bl(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        match self.dynamic_indexed_part_nbl(name, start, width, ascending, value, env)? {
            StmtIR::NBlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            } => Ok(StmtIR::BlIndexed {
                net,
                decl_width,
                base_offset,
                element_width,
                start,
                width,
                ascending,
                reversed,
                e,
            }),
            _ => unreachable!("dynamic indexed NBA lowering always returns BlIndexed fields"),
        }
    }

    fn reversed_indexed_start(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: usize,
        ascending: bool,
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let start = self.elab_expr(start, env)?;
        let logical_start = if ascending {
            start
        } else {
            ExprIR::Bin {
                op: BinOp::Sub,
                a: Box::new(start),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, width as u64 - 1))),
            }
        };
        Ok(ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(logical_start),
            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, entry.msb as u64))),
        })
    }

    fn dynamic_indexed_part_bl_reversed(
        &self,
        entry: &NetEntry,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }
        let start = self.reversed_indexed_start(entry, start, width, ascending, env)?;
        let value = self.elab_expr(value, env)?;
        let value = Self::resize_indexed_value(value, width);
        Ok(StmtIR::BlIndexed {
            net: entry.base,
            decl_width: entry.width,
            base_offset: 0,
            element_width: entry.width,
            start,
            width,
            ascending: true,
            reversed: true,
            e: value,
        })
    }

    fn resize_indexed_value(value: ExprIR, width: usize) -> ExprIR {
        if value.width() < width {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(width - value.width())),
                value,
            ])
        } else if value.width() > width {
            ExprIR::Index {
                e: Box::new(value),
                msb: width - 1,
                lsb: 0,
            }
        } else {
            value
        }
    }

    fn reverse_ast_bits(value: ast::Expr, width: usize) -> ast::Expr {
        ast::Expr::Concat(
            (0..width)
                .map(|bit| ast::Expr::Slice {
                    value: Box::new(value.clone()),
                    msb: Box::new(constant_expr(bit as i64)),
                    lsb: Box::new(constant_expr(bit as i64)),
                })
                .collect(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn function_array_indexed_update(
        old_element: ast::Expr,
        element_width: usize,
        offset: ast::Expr,
        width: usize,
        value: ast::Expr,
    ) -> ast::Expr {
        let max_offset = element_width - width;
        let in_range = ast::Expr::Bin(
            ast::BinOpSrc::LogAnd,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Ge,
                Box::new(offset.clone()),
                Box::new(constant_expr(0)),
            )),
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Le,
                Box::new(offset.clone()),
                Box::new(constant_expr(max_offset as i64)),
            )),
        );
        let mask = ast::Expr::Num {
            width: Some(element_width),
            word: {
                let mut bits = LogicWord::zero(element_width).bits;
                for bit in bits.iter_mut().take(width) {
                    *bit = LogicValue::One;
                }
                LogicWord { bits }
            },
            orig: format!("function_array_indexed_mask_{width}"),
        };
        let shifted_mask =
            ast::Expr::Bin(ast::BinOpSrc::Shl, Box::new(mask), Box::new(offset.clone()));
        let value = ast::Expr::Resize {
            width: element_width,
            value: Box::new(value),
        };
        let inserted = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Shl,
                Box::new(value),
                Box::new(offset),
            )),
            Box::new(shifted_mask.clone()),
        );
        let preserved = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(old_element.clone()),
            Box::new(ast::Expr::Un(ast::UnOpSrc::Not, Box::new(shifted_mask))),
        );
        let merged = ast::Expr::Bin(ast::BinOpSrc::Or, Box::new(preserved), Box::new(inserted));
        ast::Expr::Cond(Box::new(in_range), Box::new(merged), Box::new(old_element))
    }

    fn expand_runtime_indexed_part_assignment(
        &self,
        name: &str,
        start: &ast::Expr,
        width: &ast::Expr,
        ascending: bool,
        value: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<ast::Stmt, String> {
        let entry = env.net(name)?;
        if entry.array.is_some() {
            return Err(format!(
                "indexed part-select is not supported on unpacked array '{}'",
                name
            ));
        }
        if entry.msb < entry.lsb {
            return Err(format!(
                "indexed part-select on reversed packed range [{}:{}] is not yet supported",
                entry.msb, entry.lsb
            ));
        }
        let width = self.const_eval(width, env.params, env.module_name)?;
        if width <= 0 {
            return Err(format!(
                "indexed part-select width must be positive, got {width}"
            ));
        }
        let width = width as usize;
        if width > entry.width {
            return Err(format!(
                "indexed part-select width {width} exceeds declaration width {}",
                entry.width
            ));
        }

        let offset = if ascending {
            start.clone()
        } else {
            ast::Expr::Bin(
                ast::BinOpSrc::Sub,
                Box::new(start.clone()),
                Box::new(constant_expr(width as i64 - 1)),
            )
        };
        let mut mask = LogicWord::zero(entry.width);
        for bit in mask.bits.iter_mut().take(width) {
            *bit = LogicValue::One;
        }
        let mask = ast::Expr::Num {
            width: Some(entry.width),
            word: mask,
            orig: format!("runtime_indexed_mask_{width}"),
        };
        let shifted_mask =
            ast::Expr::Bin(ast::BinOpSrc::Shl, Box::new(mask), Box::new(offset.clone()));
        let target = ast::Expr::Ident(name.to_string());
        let inserted = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Shl,
                Box::new(ast::Expr::Resize {
                    width: entry.width,
                    value: Box::new(value.clone()),
                }),
                Box::new(offset.clone()),
            )),
            Box::new(shifted_mask.clone()),
        );
        let preserved = ast::Expr::Bin(
            ast::BinOpSrc::And,
            Box::new(target.clone()),
            Box::new(ast::Expr::Un(ast::UnOpSrc::Not, Box::new(shifted_mask))),
        );
        let merged = ast::Expr::Bin(ast::BinOpSrc::Or, Box::new(preserved), Box::new(inserted));
        let max_offset = entry.width - width;
        let in_range = ast::Expr::Bin(
            ast::BinOpSrc::LogAnd,
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Ge,
                Box::new(offset.clone()),
                Box::new(constant_expr(0)),
            )),
            Box::new(ast::Expr::Bin(
                ast::BinOpSrc::Le,
                Box::new(offset),
                Box::new(constant_expr(max_offset as i64)),
            )),
        );
        let updated = ast::Expr::Cond(Box::new(in_range), Box::new(merged), Box::new(target));
        Ok(ast::Stmt::BL {
            lv: ast::Expr::Ident(name.to_string()),
            e: updated,
        })
    }

    fn resolve_unpacked_array(
        &self,
        array_ranges: &[ast::RangeAst],
        element_width: usize,
        params: &HashMap<String, i64>,
        module_name: &str,
        object_name: &str,
        object_kind: &str,
    ) -> Result<Option<NetArray>, String> {
        if array_ranges.len() == 1 && array_ranges[0].is_none() {
            element_width
                .checked_mul(256)
                .filter(|total| *total <= 4096)
                .ok_or_else(|| {
                    format!(
                        "module '{module_name}': dynamic {object_kind} '{object_name}' exceeds the packed-width limit"
                    )
                })?;
            return Ok(Some(NetArray {
                dimensions: vec![NetArrayDimension {
                    base: 0,
                    step: 1,
                    length: 256,
                }],
                length: 256,
                element_width,
                element_msb: element_width as i64 - 1,
                element_lsb: 0,
                dynamic: true,
                dynamic_size_base: usize::MAX,
            }));
        }
        let mut dimensions = Vec::with_capacity(array_ranges.len());
        let mut length = 1_usize;
        for array_range in array_ranges {
            let Some((left, right)) = array_range else {
                return Err(format!(
                    "module '{module_name}': unpacked {object_kind} '{object_name}' has an empty range"
                ));
            };
            let base = self.const_eval(left, params, module_name)?;
            let last = self.const_eval(right, params, module_name)?;
            let step = if base <= last { 1 } else { -1 };
            let dimension_length = (base - last).unsigned_abs() as usize + 1;
            if dimension_length == 0 || dimension_length > 256 {
                return Err(format!(
                    "module '{module_name}': unpacked {object_kind} '{object_name}' has invalid dimensions"
                ));
            }
            length = length
                .checked_mul(dimension_length)
                .filter(|total| *total <= 4096)
                .ok_or_else(|| {
                    format!(
                        "module '{module_name}': unpacked {object_kind} '{object_name}' exceeds the element limit"
                    )
                })?;
            dimensions.push(NetArrayDimension {
                base,
                step,
                length: dimension_length,
            });
        }
        if dimensions.is_empty() {
            return Ok(None);
        }
        element_width
            .checked_mul(length)
            .filter(|total| *total <= 4096)
            .ok_or_else(|| {
                format!(
                    "module '{module_name}': unpacked {object_kind} '{object_name}' exceeds the packed-width limit"
                )
            })?;
        Ok(Some(NetArray {
            dimensions,
            length,
            element_width,
            element_msb: element_width as i64 - 1,
            element_lsb: 0,
            dynamic: false,
            dynamic_size_base: usize::MAX,
        }))
    }

    fn apply_element_range(
        &self,
        array: &mut Option<NetArray>,
        range: &ast::RangeAst,
        params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<(), String> {
        let Some(resolved) = array.as_mut() else {
            return Ok(());
        };
        if let Some((msb, lsb)) = range {
            resolved.element_msb = self.const_eval(msb, params, module_name)?;
            resolved.element_lsb = self.const_eval(lsb, params, module_name)?;
        }
        Ok(())
    }

    fn flat_array_range(total_width: usize) -> ast::RangeAst {
        Some((
            Box::new(ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, total_width as u64 - 1),
                orig: (total_width - 1).to_string(),
            }),
            Box::new(ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, 0),
                orig: "0".to_string(),
            }),
        ))
    }

    fn member_reference(
        &self,
        env: &Env<'_>,
        object: &str,
        field: &str,
        write: bool,
    ) -> Result<(usize, usize, Range), String> {
        if let Some(result) = self.class_field_reference(env, object, field) {
            return result;
        }
        if let Some(fields) = env.interface_ports.get(object) {
            let entry = fields
                .get(field)
                .ok_or_else(|| format!("interface port '{}' has no signal '{}'", object, field))?;
            if let Some(directions) = env.interface_directions.get(object) {
                match directions.get(field).copied() {
                    None => {
                        return Err(format!(
                            "modport for interface '{}' does not expose '{}'",
                            object, field
                        ));
                    }
                    Some(ast::DirSrc::In) if write => {
                        return Err(format!(
                            "modport '{}' exposes '{}' as input; cannot assign it",
                            object, field
                        ));
                    }
                    Some(ast::DirSrc::Out) if !write => {
                        return Err(format!(
                            "modport '{}' exposes '{}' as output; cannot read it",
                            object, field
                        ));
                    }
                    _ => {}
                }
            }
            return Ok((entry.base, entry.width, full_range(entry.width)));
        }
        let entry = env.net(object)?;
        let type_name = entry
            .type_name
            .as_deref()
            .ok_or_else(|| format!("'{}' is not a typed struct object", object))?;
        let definition = env
            .types
            .get(type_name)
            .ok_or_else(|| format!("unknown struct type '{}'", type_name))?;
        let definition = match definition {
            ResolvedType::Struct(definition) | ResolvedType::Union(definition) => definition,
            _ => return Err(format!("type '{}' is not a packed struct/union", type_name)),
        };
        let &(offset, width) = definition
            .fields
            .get(field)
            .ok_or_else(|| format!("packed type '{}' has no field '{}'", type_name, field))?;
        Ok((
            entry.base,
            entry.width,
            Range::Span {
                msb: offset + width - 1,
                lsb: offset,
            },
        ))
    }

    fn elab_module_inner(
        &mut self,
        module: &ast::Module,
        prefix: &str,
        parent_params: &HashMap<String, i64>,
        interface_ports: &HashMap<String, HashMap<String, NetEntry>>,
    ) -> Result<(), String> {
        let root_instance = prefix.is_empty();
        let net_start = self.design.n_nets;
        let mut root_index = None;
        if root_instance {
            root_index = Some(self.design.hierarchy.len());
            self.design.hierarchy.push(HierarchicalInstance {
                path: module.name.clone(),
                module: module.name.clone(),
                parent: None,
                depth: 0,
                net_start,
                net_end: net_start,
                ports: Vec::new(),
            });
        }
        // 1. parameters (may reference earlier parameters). Values passed
        //    down from a parent instance take precedence over defaults.
        let mut params = parent_params.clone();
        let mut local_types = HashMap::new();
        let mut local_functions = HashMap::new();
        for import in self.file_imports {
            self.apply_import(
                import,
                &mut params,
                &mut local_types,
                &mut local_functions,
                parent_params,
                &module.name,
            )?;
        }
        for item in &module.items {
            if let ast::Item::Imports(imports) = item {
                for import in imports {
                    self.apply_import(
                        import,
                        &mut params,
                        &mut local_types,
                        &mut local_functions,
                        parent_params,
                        &module.name,
                    )?;
                }
            }
        }
        for item in &module.items {
            if let ast::Item::Params(list) = item {
                for (name, value) in list {
                    if parent_params.contains_key(name) {
                        continue;
                    }
                    let v = self.const_eval(value, &params, &module.name)?;
                    params.insert(name.clone(), v);
                }
            }
        }
        for item in &module.items {
            if let ast::Item::TypeDef(definition) = item {
                let type_name = match definition {
                    ast::TypeDef::Enum(definition) => &definition.name,
                    ast::TypeDef::Struct(definition) => &definition.name,
                    ast::TypeDef::Union(definition) => &definition.name,
                };
                if local_types.contains_key(type_name) {
                    return Err(format!(
                        "module '{}': duplicate type definition '{}'",
                        module.name, type_name
                    ));
                }
                let resolved = resolve_type(definition, &params, &module.name, &local_types)?;
                if let ResolvedType::Enum(enum_definition) = &resolved {
                    for (constant, value) in &enum_definition.constants {
                        if !parent_params.contains_key(constant) {
                            params.insert(constant.clone(), *value);
                        }
                    }
                }
                local_types.insert(type_name.clone(), resolved);
            }
        }
        let effective_items =
            flatten_generate_items(&module.items, &params, &module.name, self.packages)?;
        for item in &effective_items {
            if let ast::Item::Function(definition) = item {
                if local_functions.contains_key(&definition.name) {
                    return Err(format!(
                        "module '{}': duplicate function '{}'",
                        module.name, definition.name
                    ));
                }
                let mut argument_names = std::collections::HashSet::new();
                for (argument, _) in &definition.args {
                    if !argument_names.insert(argument.as_str()) {
                        return Err(format!(
                            "module '{}': function '{}' has duplicate argument '{}'",
                            module.name, definition.name, argument
                        ));
                    }
                }
                local_functions.insert(definition.name.clone(), definition.clone());
            }
        }

        // 2. Ordered port list (header order) with directions.
        let mut port_dirs: HashMap<String, ast::DirSrc> = HashMap::new();
        let mut port_order: Vec<String> = Vec::new();
        {
            let mut seen = std::collections::HashSet::new();
            for item in &effective_items {
                if let ast::Item::Port {
                    dir,
                    interface_name: None,
                    names,
                    ..
                } = item
                {
                    for n in names {
                        port_dirs.insert(n.clone(), *dir);
                        if seen.insert(n.clone()) {
                            port_order.push(n.clone());
                        }
                    }
                }
            }
        }

        // 3. Declare nets from explicit declarations and ports.
        let mut nets: HashMap<String, NetEntry> = HashMap::new();
        let mut class_handles: HashMap<String, String> = HashMap::new();
        let mut interface_directions = HashMap::new();
        for item in &effective_items {
            match item {
                ast::Item::Net {
                    is_reg,
                    is_signed,
                    type_name,
                    range,
                    array_ranges,
                    names,
                } => {
                    for (n, _) in names {
                        if nets.contains_key(n) {
                            return Err(format!(
                                "module '{}': duplicate declaration of '{}'",
                                module.name, n
                            ));
                        }
                        let type_width = self.declaration_type_width(
                            type_name.as_deref(),
                            &local_types,
                            &module.name,
                        )?;
                        let is_class_handle = type_name
                            .as_deref()
                            .is_some_and(|t| self.classes.contains_key(t));
                        if is_class_handle {
                            class_handles.insert(n.clone(), type_name.clone().unwrap_or_default());
                        }
                        // Class handles are procedurally assigned storage.
                        let is_reg = *is_reg || is_class_handle;
                        let element_width = match type_width {
                            Some(width) => width,
                            None => match range {
                                Some((msb, lsb)) => {
                                    let hi = self.const_eval(msb, &params, &module.name)?;
                                    let lo = self.const_eval(lsb, &params, &module.name)?;
                                    (hi - lo).unsigned_abs() as usize + 1
                                }
                                None => 1,
                            },
                        };
                        let mut array = self.resolve_unpacked_array(
                            array_ranges,
                            element_width,
                            &params,
                            &module.name,
                            n,
                            "net",
                        )?;
                        self.apply_element_range(&mut array, range, &params, &module.name)?;
                        let declaration_range = if array.is_some() {
                            let length = array.as_ref().map_or(1, |resolved| resolved.length);
                            Self::flat_array_range(element_width * length)
                        } else {
                            range.clone()
                        };
                        let mut e = self.declare_net(
                            n,
                            prefix,
                            &declaration_range,
                            &params,
                            is_reg,
                            *is_signed,
                            type_width,
                            type_name.as_deref(),
                        )?;
                        e.array = array;
                        if e.array.as_ref().is_some_and(|array| array.dynamic) {
                            let size_name = format!("__{n}_dynamic_size");
                            let size_entry = self.declare_net(
                                &size_name,
                                prefix,
                                &Self::flat_array_range(9),
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?;
                            let size_base = size_entry.base;
                            e.array
                                .as_mut()
                                .expect("dynamic array metadata was just checked")
                                .dynamic_size_base = size_base;
                        }
                        nets.insert(n.clone(), e);
                    }
                }
                ast::Item::Port {
                    interface_name: Some(definition),
                    modport_name,
                    names,
                    ..
                } => {
                    for n in names {
                        if !interface_ports.contains_key(n) {
                            return Err(format!(
                                "module '{}': interface port '{}' has no binding",
                                module.name, n
                            ));
                        }
                        let fields = &interface_ports[n];
                        let interface = self.mods.get(definition).ok_or_else(|| {
                            format!(
                                "module '{}': unknown interface '{}'",
                                module.name, definition
                            )
                        })?;
                        if !interface.is_interface {
                            return Err(format!("'{}' is not an interface", definition));
                        }
                        let expected = interface
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                ast::Item::Net { names, .. } => Some(names),
                                _ => None,
                            })
                            .flatten()
                            .count();
                        if fields.len() != expected {
                            return Err(format!(
                                "module '{}': interface port '{}' has incorrect signal binding",
                                module.name, n
                            ));
                        }
                        if let Some(modport_name) = modport_name {
                            let modport = interface
                                .items
                                .iter()
                                .find_map(|item| match item {
                                    ast::Item::Modport { name, ports } if name == modport_name => {
                                        Some(ports)
                                    }
                                    _ => None,
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "module '{}': interface '{}' has no modport '{}'",
                                        module.name, definition, modport_name
                                    )
                                })?;
                            let signal_names = interface
                                .items
                                .iter()
                                .filter_map(|item| match item {
                                    ast::Item::Net { names, .. } => Some(names),
                                    _ => None,
                                })
                                .flatten()
                                .map(|(name, _)| name.as_str())
                                .collect::<std::collections::HashSet<_>>();
                            for port in modport {
                                if !signal_names.contains(port.name.as_str()) {
                                    return Err(format!(
                                        "interface '{}': modport '{}' lists unknown signal '{}'",
                                        definition, modport_name, port.name
                                    ));
                                }
                            }
                            interface_directions.insert(
                                n.clone(),
                                modport
                                    .iter()
                                    .map(|port| (port.name.clone(), port.dir))
                                    .collect::<HashMap<_, _>>(),
                            );
                        }
                    }
                }
                ast::Item::Port {
                    range,
                    array_ranges,
                    type_name,
                    names,
                    is_reg,
                    is_signed,
                    interface_name: None,
                    ..
                } => {
                    for n in names {
                        if !nets.contains_key(n) {
                            let type_width = self.declaration_type_width(
                                type_name.as_deref(),
                                &local_types,
                                &module.name,
                            )?;
                            let element_width = match type_width {
                                Some(width) => width,
                                None => match range {
                                    Some((msb, lsb)) => {
                                        let hi = self.const_eval(msb, &params, &module.name)?;
                                        let lo = self.const_eval(lsb, &params, &module.name)?;
                                        (hi - lo).unsigned_abs() as usize + 1
                                    }
                                    None => 1,
                                },
                            };
                            let mut array = self.resolve_unpacked_array(
                                array_ranges,
                                element_width,
                                &params,
                                &module.name,
                                n,
                                "port",
                            )?;
                            if array.as_ref().is_some_and(|resolved| resolved.dynamic) {
                                return Err(format!(
                                    "module '{}': dynamic unpacked array port '{}' is not supported",
                                    module.name, n
                                ));
                            }
                            self.apply_element_range(&mut array, range, &params, &module.name)?;
                            let declaration_range = if array.is_some() {
                                let length = array.as_ref().map_or(1, |resolved| resolved.length);
                                Self::flat_array_range(element_width * length)
                            } else {
                                range.clone()
                            };
                            let mut e = self.declare_net(
                                n,
                                prefix,
                                &declaration_range,
                                &params,
                                *is_reg,
                                *is_signed,
                                type_width,
                                type_name.as_deref(),
                            )?;
                            e.array = array;
                            nets.insert(n.clone(), e);
                        } else if !array_ranges.is_empty() {
                            return Err(format!(
                                "module '{}': cannot refine existing port '{}' as an unpacked array",
                                module.name, n
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        // 3.5 Class storage: per-handle instance fields and shared statics.
        for (handle, class) in class_handles.clone() {
            self.declare_handle_fields(&handle, &class, prefix, &params, &mut nets, 0)?;
        }
        let mut static_storage_classes: HashSet<String> = self
            .classes
            .keys()
            .filter(|class| !BUILTIN_UVM_CLASS_NAMES.contains(&class.as_str()))
            .cloned()
            .collect();
        static_storage_classes.extend(class_handles.values().cloned());
        let effective_item_debug = format!("{effective_items:?}");
        for class in BUILTIN_UVM_CLASS_NAMES {
            if effective_item_debug.contains(&format!("\"{class}\"")) {
                static_storage_classes.insert(class.to_string());
            }
        }
        let class_static_initials = self.declare_class_storage(
            prefix,
            &params,
            &module.name,
            &mut nets,
            &static_storage_classes,
        )?;

        // 4. Record top-level ports.
        if prefix.is_empty() {
            for n in &port_order {
                let e = &nets[n];
                let dir = port_dirs.get(n).copied().unwrap_or(ast::DirSrc::In);
                self.design.ports.push(PortDecl {
                    name: n.clone(),
                    dir: if dir == ast::DirSrc::Out {
                        Dir::Out
                    } else {
                        Dir::In
                    },
                    base: e.base,
                    width: e.width,
                });
            }
        }

        // 5. Elaborate bodies.
        let mut interface_instances = HashMap::new();
        let env = Env {
            class_aliases: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),
            nets: &nets,
            interface_ports,
            interface_directions: &interface_directions,
            params: &params,
            types: &local_types,
            functions: &local_functions,
            module_name: &module.name,
            prefix,
            class_handles: &class_handles,
            this_handle: None,
            method_owner: None,
            method_args: &HashMap::new(),
            method_return: None,
        };
        let mut body_items = self.expand_module_instance_arrays(&effective_items, &env)?;
        // Static-field seeds must run before any user initial block that may
        // observe or mutate them.
        body_items.splice(0..0, class_static_initials);
        for item in &body_items {
            match item {
                ast::Item::Net { names, .. } => {
                    for (n, init) in names {
                        if let Some(e) = init {
                            if let ast::Expr::NewClass(actuals) = e {
                                let body = self.elab_stmt(
                                    &ast::Stmt::BL {
                                        lv: ast::Expr::Ident(n.clone()),
                                        e: ast::Expr::NewClass(actuals.clone()),
                                    },
                                    &env,
                                )?;
                                self.design.initials.push(body);
                                continue;
                            }
                            let e = self.elab_expr(e, &env)?;
                            let lv = self.elab_lvalue(&ast::Expr::Ident(n.clone()), &env)?;
                            self.design.assigns.push(Assign { lv, e, delay: 0 });
                        }
                    }
                }
                ast::Item::Assign { delay, lv, e } => {
                    if let ast::Expr::ArrayIndexedPart {
                        name,
                        indexes,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        if indexes.iter().any(|index| {
                            self.const_eval(index, env.params, env.module_name).is_err()
                        }) {
                            if *delay != 0 {
                                return Err(
                                    "delayed runtime indexed continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            if self.const_eval(start, env.params, env.module_name).is_err() {
                                return Err(
                                    "runtime indexed part-select continuous-assignment lvalue is not supported"
                                        .to_string(),
                                );
                            }
                            let body = self.runtime_array_element_indexed_statement(
                                name, indexes, start, width, *ascending, e, false, &env,
                            )?;
                            self.design.processes.push(Process {
                                sens: SensIR::Comb,
                                body,
                            });
                            continue;
                        }
                        if self.const_eval(start, env.params, env.module_name).is_err() {
                            return Err(
                                "runtime indexed part-select continuous-assignment lvalue is not supported"
                                    .to_string(),
                            );
                        }
                        let (entry, offset, element_width) =
                            self.array_element_location(name, indexes, &env)?;
                        let (physical_lsb, width, reversed) = self.array_element_indexed_span(
                            &entry,
                            offset,
                            element_width,
                            start,
                            width,
                            *ascending,
                            &env,
                        )?;
                        let target = LValue {
                            net: entry.base,
                            decl_width: entry.width,
                            range: Range::Span {
                                msb: physical_lsb + width - 1,
                                lsb: physical_lsb,
                            },
                        };
                        let value = self.elab_expr(e, &env)?;
                        let value = Self::resize_indexed_value(value, width);
                        let physical_value = if reversed {
                            ExprIR::Concat(
                                (0..width)
                                    .map(|bit| ExprIR::Index {
                                        e: Box::new(value.clone()),
                                        msb: bit,
                                        lsb: bit,
                                    })
                                    .collect(),
                            )
                        } else {
                            value
                        };
                        self.design.assigns.push(Assign {
                            lv: target,
                            e: physical_value,
                            delay: *delay,
                        });
                        continue;
                    }
                    if let ast::Expr::IndexedPart {
                        name,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        if let Ok(entry) = env.net(name) {
                            if entry.array.is_none() && entry.msb < entry.lsb {
                                if self.const_eval(start, env.params, env.module_name).is_err() {
                                    if *delay != 0 {
                                        return Err(
                                            "delayed runtime indexed continuous-assignment lvalue is not supported"
                                                .to_string(),
                                        );
                                    }
                                    let body = self.dynamic_indexed_part_bl_reversed(
                                        entry, start, width, *ascending, e, &env,
                                    )?;
                                    self.design.processes.push(Process {
                                        sens: SensIR::Comb,
                                        body,
                                    });
                                    continue;
                                }
                                let target = self.reversed_indexed_part_lvalue(
                                    entry, start, width, *ascending, &env,
                                )?;
                                let value = self.elab_expr(e, &env)?;
                                let value_width = target.range.width();
                                let value = Self::resize_indexed_value(value, value_width);
                                let physical_value = ExprIR::Concat(
                                    (0..value_width)
                                        .map(|bit| ExprIR::Index {
                                            e: Box::new(value.clone()),
                                            msb: bit,
                                            lsb: bit,
                                        })
                                        .collect(),
                                );
                                self.design.assigns.push(Assign {
                                    lv: target,
                                    e: physical_value,
                                    delay: *delay,
                                });
                                continue;
                            }
                            if entry.array.is_none()
                                && self.const_eval(start, env.params, env.module_name).is_err()
                            {
                                if *delay != 0 {
                                    return Err(
                                        "delayed runtime indexed continuous-assignment lvalue is not supported"
                                            .to_string(),
                                    );
                                }
                                let body = self.dynamic_indexed_part_bl(
                                    name, start, width, *ascending, e, &env,
                                )?;
                                self.design.processes.push(Process {
                                    sens: SensIR::Comb,
                                    body,
                                });
                                continue;
                            }
                        }
                    }
                    if let ast::Expr::Concat(parts) = lv {
                        let rhs = self.elab_expr(e, &env)?;
                        let total: usize = parts
                            .iter()
                            .map(|p| self.elab_lvalue(p, &env).map(|lv| lv.range.width()))
                            .collect::<Result<Vec<_>, _>>()?
                            .iter()
                            .sum();
                        let mut cursor = total;
                        for part in parts {
                            let lv = self.elab_lvalue(part, &env)?;
                            let width = lv.range.width();
                            cursor -= width;
                            let sliced = ExprIR::Index {
                                e: Box::new(rhs.clone()),
                                msb: cursor + width - 1,
                                lsb: cursor,
                            };
                            self.design.assigns.push(Assign {
                                lv,
                                e: sliced,
                                delay: *delay,
                            });
                        }
                        continue;
                    }
                    let lv = self.elab_lvalue(lv, &env)?;
                    let e = self.elab_expr(e, &env)?;
                    self.design.assigns.push(Assign {
                        lv,
                        e,
                        delay: *delay,
                    });
                }
                ast::Item::Always { sens, body } => {
                    let s = self.elab_sens(sens, &env)?;
                    let b = self.elab_stmt(body, &env)?;
                    self.design.processes.push(Process { sens: s, body: b });
                }
                ast::Item::AssertProperty {
                    kind,
                    label,
                    clock,
                    disable,
                    antecedent,
                    consequent,
                    composition,
                    overlapping,
                    delay,
                    repetition,
                    message,
                } => {
                    let sens = self.elab_sens(clock, &env)?;
                    if !matches!(sens, SensIR::Posedge(_) | SensIR::Negedge(_)) {
                        return Err(format!(
                            "module '{}': concurrent property requires a posedge or negedge clock",
                            module.name
                        ));
                    }
                    let antecedent = self.elab_expr(antecedent, &env)?;
                    let disable = self.elab_expr(disable, &env)?;
                    let mut consequent = self.elab_expr(consequent, &env)?;
                    let mut composed_right = None;
                    let mut composed_op = ast::PropertyCompositionOp::And;
                    let mut composition_delay_cycles = 0_usize;
                    if let Some(composition) = composition.as_deref() {
                        let right = self.elab_expr(&composition.right, &env)?;
                        match &composition.right_delay {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay = self.const_eval(delay, env.params, env.module_name)?;
                                if !(0..=256).contains(&delay) {
                                    return Err(format!(
                                        "module '{}': sequence composition delay must be between 0 and 256",
                                        module.name
                                    ));
                                }
                                composition_delay_cycles = delay as usize;
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': sequence composition supports fixed delay only",
                                    module.name
                                ));
                            }
                        }
                        composed_op = composition.op;
                        if composition_delay_cycles == 0 {
                            consequent = ExprIR::Bin {
                                op: match composition.op {
                                    ast::PropertyCompositionOp::And => BinOp::LogAnd,
                                    ast::PropertyCompositionOp::Or => BinOp::LogOr,
                                },
                                a: Box::new(consequent),
                                b: Box::new(right),
                            };
                        } else {
                            composed_right = Some(right);
                        }
                    }
                    let (delay_cycles, delay_range) = match delay.as_ref() {
                        ast::PropertyDelay::Fixed(delay) => {
                            let explicit_delay =
                                self.const_eval(delay, env.params, env.module_name)?;
                            if !(0..=256).contains(&explicit_delay) {
                                return Err(format!(
                                    "module '{}': property sequence delay must be between 0 and 256",
                                    module.name
                                ));
                            }
                            let cycles = if *overlapping {
                                explicit_delay as usize
                            } else {
                                1
                            };
                            (Some(cycles), None)
                        }
                        ast::PropertyDelay::Range(start, end) => {
                            let start = self.const_eval(start, env.params, env.module_name)?;
                            let end = self.const_eval(end, env.params, env.module_name)?;
                            if !(0..=256).contains(&start) || !(0..=256).contains(&end) {
                                return Err(format!(
                                    "module '{}': property sequence delay range must stay within 0..=256",
                                    module.name
                                ));
                            }
                            if start > end {
                                return Err(format!(
                                    "module '{}': property sequence delay range start exceeds end",
                                    module.name
                                ));
                            }
                            (None, Some((start as usize, end as usize)))
                        }
                    };
                    let property_index = self.design.processes.len();
                    let property_state_base = self.design.net_decls.len();
                    let mut composition_stages = Vec::with_capacity(composition_delay_cycles);
                    for stage in 0..composition_delay_cycles {
                        let stage_name = format!("__property_composed_{property_index}_{stage}");
                        composition_stages.push(self.declare_net(
                            &stage_name,
                            prefix,
                            &None,
                            &HashMap::new(),
                            true,
                            false,
                            None,
                            None,
                        )?);
                    }
                    let composition_stage_references = composition_stages
                        .iter()
                        .map(|pending| ExprIR::Ref {
                            net: pending.base,
                            decl_width: pending.width,
                            range: full_range(pending.width),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let cover_label = label
                        .clone()
                        .unwrap_or_else(|| format!("property_{property_index}"));
                    let make_direct_check = |consequent: ExprIR| -> StmtIR {
                        match kind {
                            ast::PropertyKind::Assert => StmtIR::Assert {
                                cond: consequent,
                                message: message.clone(),
                            },
                            ast::PropertyKind::Assume => StmtIR::Assume {
                                cond: consequent,
                                message: message.clone(),
                            },
                            ast::PropertyKind::Cover => StmtIR::Cover {
                                label: cover_label.clone(),
                                cond: consequent,
                            },
                        }
                    };
                    let make_check = |left: ExprIR| -> StmtIR {
                        if composed_right.is_none() {
                            return make_direct_check(left);
                        };
                        let Some(first_stage) = composition_stage_references.first() else {
                            return make_direct_check(left);
                        };
                        let ExprIR::Ref {
                            net: first_stage_net,
                            decl_width: first_stage_width,
                            ..
                        } = first_stage
                        else {
                            return make_direct_check(left);
                        };
                        let first_stage_lvalue = LValue {
                            net: *first_stage_net,
                            decl_width: *first_stage_width,
                            range: full_range(*first_stage_width),
                        };
                        match composed_op {
                            ast::PropertyCompositionOp::And => StmtIR::If {
                                c: left,
                                t: Box::new(StmtIR::NBl {
                                    lv: first_stage_lvalue,
                                    e: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                }),
                                f: Some(Box::new(make_direct_check(ExprIR::Const(
                                    LogicWord::from_u64(1, 0),
                                )))),
                            },
                            ast::PropertyCompositionOp::Or => StmtIR::If {
                                c: left,
                                t: Box::new(make_direct_check(ExprIR::Const(LogicWord::from_u64(
                                    1, 1,
                                )))),
                                f: Some(Box::new(StmtIR::NBl {
                                    lv: first_stage_lvalue,
                                    e: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                })),
                            },
                        }
                    };
                    let body = if let Some((goto_start, goto_end)) = match repetition.as_deref() {
                        Some(ast::PropertyRepetition::Goto(count)) => Some((count, None)),
                        Some(ast::PropertyRepetition::GotoRange(start, end)) => {
                            Some((start, Some(end)))
                        }
                        Some(ast::PropertyRepetition::Nonconsecutive(count)) => Some((count, None)),
                        Some(ast::PropertyRepetition::NonconsecutiveRange(start, end)) => {
                            Some((start, Some(end)))
                        }
                        _ => None,
                    } {
                        let count = self
                            .const_eval(goto_start, env.params, env.module_name)
                            .map_err(|error| {
                                format!(
                                    "module '{}': goto repetition range must be constant: {error}",
                                    module.name
                                )
                            })?;
                        if let Some(end) = goto_end {
                            let end = self.const_eval(end, env.params, env.module_name)?;
                            if !(1..=256).contains(&end) {
                                return Err(format!(
                                    "module '{}': goto repetition range must stay within 1..=256",
                                    module.name
                                ));
                            }
                            if count > end {
                                return Err(format!(
                                    "module '{}': goto repetition range start exceeds end",
                                    module.name
                                ));
                            }
                        }
                        if !(1..=256).contains(&count) {
                            return Err(format!(
                                "module '{}': goto repetition count must be between 1 and 256",
                                module.name
                            ));
                        }
                        if !*overlapping {
                            return Err(format!(
                                "module '{}': goto repetition requires '|->'",
                                module.name
                            ));
                        }
                        match delay.as_ref() {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay_value =
                                    self.const_eval(delay, env.params, env.module_name)?;
                                if delay_value != 0 {
                                    return Err(format!(
                                        "module '{}': goto repetition does not yet compose with sequence delay",
                                        module.name
                                    ));
                                }
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': goto repetition does not yet compose with delay ranges",
                                    module.name
                                ));
                            }
                        }

                        let count = count as usize;
                        if count == 1 {
                            StmtIR::If {
                                c: antecedent,
                                t: Box::new(make_check(consequent)),
                                f: None,
                            }
                        } else {
                            let mut stages = Vec::with_capacity(count - 1);
                            for seen in 1..count {
                                let state_name = format!("__property_goto_{property_index}_{seen}");
                                stages.push(self.declare_net(
                                    &state_name,
                                    prefix,
                                    &None,
                                    &HashMap::new(),
                                    true,
                                    false,
                                    None,
                                    None,
                                )?);
                            }
                            let references = stages
                                .iter()
                                .map(|state| ExprIR::Ref {
                                    net: state.base,
                                    decl_width: state.width,
                                    range: full_range(state.width),
                                    signed: false,
                                })
                                .collect::<Vec<_>>();
                            let mut statements = vec![StmtIR::If {
                                c: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[count - 2].clone()),
                                    b: Box::new(consequent.clone()),
                                },
                                t: Box::new(make_check(consequent.clone())),
                                f: None,
                            }];

                            for seen in (2..count).rev() {
                                let retain = ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[seen - 1].clone()),
                                    b: Box::new(ExprIR::Un {
                                        op: UnOp::Not,
                                        a: Box::new(consequent.clone()),
                                    }),
                                };
                                let advance = ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(references[seen - 2].clone()),
                                    b: Box::new(consequent.clone()),
                                };
                                let next = ExprIR::Bin {
                                    op: BinOp::Or,
                                    a: Box::new(retain),
                                    b: Box::new(advance),
                                };
                                statements.push(StmtIR::Bl {
                                    lv: LValue {
                                        net: stages[seen - 1].base,
                                        decl_width: stages[seen - 1].width,
                                        range: full_range(stages[seen - 1].width),
                                    },
                                    e: next,
                                });
                            }
                            let retain_first = ExprIR::Bin {
                                op: BinOp::And,
                                a: Box::new(references[0].clone()),
                                b: Box::new(ExprIR::Un {
                                    op: UnOp::Not,
                                    a: Box::new(consequent.clone()),
                                }),
                            };
                            let start = ExprIR::Bin {
                                op: BinOp::And,
                                a: Box::new(antecedent),
                                b: Box::new(consequent),
                            };
                            let first_next = ExprIR::Bin {
                                op: BinOp::Or,
                                a: Box::new(retain_first),
                                b: Box::new(start),
                            };
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: first_next,
                            });
                            StmtIR::Block(statements)
                        }
                    } else if let Some(repetition) = repetition.as_deref() {
                        let (minimum, maximum) = match repetition {
                            ast::PropertyRepetition::Count(count) => {
                                let count = self
                                    .const_eval(count, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition count must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                (count, count)
                            }
                            ast::PropertyRepetition::Range(start, end) => {
                                let start = self
                                    .const_eval(start, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition range must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                let end = self
                                    .const_eval(end, env.params, env.module_name)
                                    .map_err(|error| {
                                        format!(
                                            "module '{}': property repetition range must be constant: {error}",
                                            module.name
                                        )
                                    })?;
                                (start, end)
                            }
                            ast::PropertyRepetition::Goto(_) => {
                                unreachable!("goto repetition is handled separately")
                            }
                            ast::PropertyRepetition::GotoRange(..) => {
                                unreachable!("goto repetition range is handled separately")
                            }
                            ast::PropertyRepetition::Nonconsecutive(_) => {
                                unreachable!("nonconsecutive repetition is handled separately")
                            }
                            ast::PropertyRepetition::NonconsecutiveRange(..) => {
                                unreachable!(
                                    "nonconsecutive repetition range is handled separately"
                                )
                            }
                        };
                        if !(1..=256).contains(&minimum) || !(1..=256).contains(&maximum) {
                            return Err(format!(
                                "module '{}': property repetition range must stay within 1..=256",
                                module.name
                            ));
                        }
                        if minimum > maximum {
                            return Err(format!(
                                "module '{}': property repetition range start exceeds end",
                                module.name
                            ));
                        }
                        if !*overlapping {
                            return Err(format!(
                                "module '{}': consecutive repetition requires '|->'",
                                module.name
                            ));
                        }
                        match delay.as_ref() {
                            ast::PropertyDelay::Fixed(delay) => {
                                let delay_value =
                                    self.const_eval(delay, env.params, env.module_name)?;
                                if delay_value != 0 {
                                    return Err(format!(
                                        "module '{}': consecutive repetition does not yet compose with sequence delay",
                                        module.name
                                    ));
                                }
                            }
                            ast::PropertyDelay::Range(..) => {
                                return Err(format!(
                                    "module '{}': consecutive repetition does not yet compose with delay ranges",
                                    module.name
                                ));
                            }
                        }

                        let count = minimum as usize;
                        if count == 1 {
                            StmtIR::If {
                                c: antecedent,
                                t: Box::new(make_check(consequent)),
                                f: None,
                            }
                        } else {
                            let mut stages = Vec::with_capacity(count - 1);
                            for age in 1..count {
                                let pending_name =
                                    format!("__property_repeat_{property_index}_{age}");
                                stages.push(self.declare_net(
                                    &pending_name,
                                    prefix,
                                    &None,
                                    &HashMap::new(),
                                    true,
                                    false,
                                    None,
                                    None,
                                )?);
                            }
                            let references = stages
                                .iter()
                                .map(|pending| ExprIR::Ref {
                                    net: pending.base,
                                    decl_width: pending.width,
                                    range: full_range(pending.width),
                                    signed: false,
                                })
                                .collect::<Vec<_>>();
                            let mut statements = Vec::new();
                            if *kind == ast::PropertyKind::Cover {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[count - 2].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Cover {
                                        label: cover_label.clone(),
                                        cond: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                    }),
                                    f: None,
                                });
                            } else {
                                let active_or_new = std::iter::once(antecedent.clone())
                                    .chain(references.iter().cloned())
                                    .reduce(|left, right| ExprIR::Bin {
                                        op: BinOp::Or,
                                        a: Box::new(left),
                                        b: Box::new(right),
                                    })
                                    .expect("repetition property has an antecedent");
                                statements.push(StmtIR::If {
                                    c: active_or_new,
                                    t: Box::new(make_check(consequent.clone())),
                                    f: None,
                                });
                            }

                            for age in (2..count).rev() {
                                statements.push(StmtIR::Bl {
                                    lv: LValue {
                                        net: stages[age - 1].base,
                                        decl_width: stages[age - 1].width,
                                        range: full_range(stages[age - 1].width),
                                    },
                                    e: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[age - 2].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                });
                            }
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(antecedent),
                                    b: Box::new(consequent),
                                },
                            });
                            StmtIR::Block(statements)
                        }
                    } else if let Some((start, end)) = delay_range {
                        let mut stages = Vec::with_capacity(end);
                        for stage in 0..end {
                            let pending_name =
                                format!("__property_pending_{property_index}_{stage}");
                            stages.push(self.declare_net(
                                &pending_name,
                                prefix,
                                &None,
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?);
                        }
                        let references = stages
                            .iter()
                            .map(|pending| ExprIR::Ref {
                                net: pending.base,
                                decl_width: pending.width,
                                range: full_range(pending.width),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let candidate = |delay: usize| -> ExprIR {
                            if delay == 0 {
                                antecedent.clone()
                            } else {
                                references[delay - 1].clone()
                            }
                        };
                        let mut statements = Vec::new();

                        if *kind == ast::PropertyKind::Cover {
                            let any_candidate = (start..=end)
                                .map(&candidate)
                                .reduce(|left, right| ExprIR::Bin {
                                    op: BinOp::Or,
                                    a: Box::new(left),
                                    b: Box::new(right),
                                })
                                .expect("property delay range is nonempty");
                            statements.push(StmtIR::If {
                                c: ExprIR::Bin {
                                    op: BinOp::And,
                                    a: Box::new(any_candidate),
                                    b: Box::new(consequent.clone()),
                                },
                                t: Box::new(StmtIR::Cover {
                                    label: cover_label.clone(),
                                    cond: ExprIR::Const(LogicWord::from_u64(1, 1)),
                                }),
                                f: None,
                            });
                        }

                        for delay in start..=end {
                            if delay > 0 {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(references[delay - 1].clone()),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Bl {
                                        lv: LValue {
                                            net: stages[delay - 1].base,
                                            decl_width: stages[delay - 1].width,
                                            range: full_range(stages[delay - 1].width),
                                        },
                                        e: ExprIR::Const(LogicWord::zero(1)),
                                    }),
                                    f: None,
                                });
                            }
                        }

                        if *kind != ast::PropertyKind::Cover {
                            let expired = candidate(end);
                            let failure = make_check(consequent.clone());
                            if end > 0 {
                                statements.push(StmtIR::If {
                                    c: expired,
                                    t: Box::new(StmtIR::Block(vec![
                                        StmtIR::Bl {
                                            lv: LValue {
                                                net: stages[end - 1].base,
                                                decl_width: stages[end - 1].width,
                                                range: full_range(stages[end - 1].width),
                                            },
                                            e: ExprIR::Const(LogicWord::zero(1)),
                                        },
                                        failure,
                                    ])),
                                    f: None,
                                });
                            } else {
                                statements.push(StmtIR::If {
                                    c: expired,
                                    t: Box::new(failure),
                                    f: None,
                                });
                            }
                        }

                        for stage in (1..end).rev() {
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[stage].base,
                                    decl_width: stages[stage].width,
                                    range: full_range(stages[stage].width),
                                },
                                e: references[stage - 1].clone(),
                            });
                        }
                        if end > 0 {
                            let pending_value = ExprIR::Un {
                                op: UnOp::OrR,
                                a: Box::new(antecedent.clone()),
                            };
                            statements.push(StmtIR::Bl {
                                lv: LValue {
                                    net: stages[0].base,
                                    decl_width: stages[0].width,
                                    range: full_range(stages[0].width),
                                },
                                e: pending_value.clone(),
                            });
                            if start == 0 {
                                statements.push(StmtIR::If {
                                    c: ExprIR::Bin {
                                        op: BinOp::And,
                                        a: Box::new(pending_value),
                                        b: Box::new(consequent.clone()),
                                    },
                                    t: Box::new(StmtIR::Bl {
                                        lv: LValue {
                                            net: stages[0].base,
                                            decl_width: stages[0].width,
                                            range: full_range(stages[0].width),
                                        },
                                        e: ExprIR::Const(LogicWord::zero(1)),
                                    }),
                                    f: None,
                                });
                            }
                        }
                        StmtIR::Block(statements)
                    } else if delay_cycles == Some(0) {
                        StmtIR::If {
                            c: antecedent,
                            t: Box::new(make_check(consequent)),
                            f: None,
                        }
                    } else {
                        let Some(delay_cycles) = delay_cycles else {
                            unreachable!("property delay is either fixed or ranged")
                        };
                        let mut stages = Vec::with_capacity(delay_cycles);
                        for stage in 0..delay_cycles {
                            let pending_name =
                                format!("__property_pending_{property_index}_{stage}");
                            stages.push(self.declare_net(
                                &pending_name,
                                prefix,
                                &None,
                                &HashMap::new(),
                                true,
                                false,
                                None,
                                None,
                            )?);
                        }
                        let stage_references = stages
                            .iter()
                            .map(|pending| ExprIR::Ref {
                                net: pending.base,
                                decl_width: pending.width,
                                range: full_range(pending.width),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let trigger = stage_references[delay_cycles - 1].clone();
                        let pending_value = ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(antecedent),
                        };
                        let mut statements = vec![StmtIR::If {
                            c: trigger,
                            t: Box::new(make_check(consequent)),
                            f: None,
                        }];
                        for stage in (1..delay_cycles).rev() {
                            statements.push(StmtIR::NBl {
                                lv: LValue {
                                    net: stages[stage].base,
                                    decl_width: stages[stage].width,
                                    range: full_range(stages[stage].width),
                                },
                                e: stage_references[stage - 1].clone(),
                            });
                        }
                        statements.push(StmtIR::NBl {
                            lv: LValue {
                                net: stages[0].base,
                                decl_width: stages[0].width,
                                range: full_range(stages[0].width),
                            },
                            e: pending_value,
                        });
                        StmtIR::Block(statements)
                    };
                    let body = if composition_delay_cycles > 0 {
                        let right = composed_right
                            .clone()
                            .expect("delayed composition has a right expression");
                        let trigger =
                            composition_stage_references[composition_delay_cycles - 1].clone();
                        let mut composition_statements = vec![StmtIR::If {
                            c: trigger,
                            t: Box::new(make_direct_check(right)),
                            f: None,
                        }];
                        for stage in (1..composition_delay_cycles).rev() {
                            composition_statements.push(StmtIR::NBl {
                                lv: LValue {
                                    net: composition_stages[stage].base,
                                    decl_width: composition_stages[stage].width,
                                    range: full_range(composition_stages[stage].width),
                                },
                                e: composition_stage_references[stage - 1].clone(),
                            });
                        }
                        let first = &composition_stages[0];
                        composition_statements.push(StmtIR::NBl {
                            lv: LValue {
                                net: first.base,
                                decl_width: first.width,
                                range: full_range(first.width),
                            },
                            e: ExprIR::Const(LogicWord::zero(1)),
                        });
                        StmtIR::Block(vec![StmtIR::Block(composition_statements), body])
                    } else {
                        body
                    };
                    let pending_prefix = format!("{prefix}__property_pending_{property_index}_");
                    let composed_prefix = format!("{prefix}__property_composed_{property_index}_");
                    let repeat_prefix = format!("{prefix}__property_repeat_{property_index}_");
                    let goto_prefix = format!("{prefix}__property_goto_{property_index}_");
                    let clear_states = self.design.net_decls[property_state_base..]
                        .iter()
                        .filter(|declaration| {
                            declaration.name.starts_with(&pending_prefix)
                                || declaration.name.starts_with(&composed_prefix)
                                || declaration.name.starts_with(&repeat_prefix)
                                || declaration.name.starts_with(&goto_prefix)
                        })
                        .map(|declaration| StmtIR::Bl {
                            lv: LValue {
                                net: declaration.base,
                                decl_width: declaration.width,
                                range: full_range(declaration.width),
                            },
                            e: ExprIR::Const(LogicWord::zero(declaration.width)),
                        })
                        .collect::<Vec<_>>();
                    let body = StmtIR::If {
                        c: disable,
                        t: Box::new(StmtIR::Block(clear_states)),
                        f: Some(Box::new(body)),
                    };
                    self.design.processes.push(Process { sens, body });
                }
                ast::Item::Initial { body } => {
                    let b = self.elab_stmt(body, &env)?;
                    self.design.initials.push(b);
                }
                ast::Item::Gate { kind, name, args } => {
                    self.elab_gate(*kind, name, args, &env)?;
                }
                ast::Item::Inst {
                    module: sub,
                    name,
                    array_ranges,
                    params: overrides,
                    conns,
                } => {
                    self.elab_inst(
                        module,
                        sub,
                        name,
                        array_ranges,
                        overrides,
                        conns,
                        &env,
                        &mut interface_instances,
                    )?;
                }
                _ => {}
            }
        }
        if root_instance {
            let net_end = self.design.n_nets;
            let root = self
                .design
                .hierarchy
                .get_mut(root_index.expect("root hierarchy index"))
                .expect("root hierarchy record");
            root.net_end = net_end;
            root.ports = self
                .design
                .ports
                .iter()
                .map(|port| HierarchicalPort {
                    name: port.name.clone(),
                    direction: port.dir,
                    base: port.base,
                    width: port.width,
                    connections: vec![None; port.width],
                })
                .collect();
        }
        Ok(())
    }

    fn elab_gate(
        &mut self,
        kind: ast::GateKind,
        name: &str,
        args: &[ast::Expr],
        env: &Env,
    ) -> Result<(), String> {
        if args.len() < 2 {
            return Err(format!("gate '{}' needs at least 2 connections", name));
        }
        let n_out = match kind {
            ast::GateKind::Buf | ast::GateKind::Not => args.len() - 1,
            _ => 1,
        };
        let mut ins = Vec::new();
        for i in &args[n_out..] {
            ins.push(self.elab_expr(i, env)?);
        }
        let combined = match kind {
            ast::GateKind::And => reduce(&ins, BinOp::And),
            ast::GateKind::Nand => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::And)),
            },
            ast::GateKind::Or => reduce(&ins, BinOp::Or),
            ast::GateKind::Nor => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::Or)),
            },
            ast::GateKind::Xor => reduce(&ins, BinOp::Xor),
            ast::GateKind::Xnor => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(reduce(&ins, BinOp::Xor)),
            },
            ast::GateKind::Buf => ins[0].clone(),
            ast::GateKind::Not => ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(ins[0].clone()),
            },
        };
        for out in &args[..n_out] {
            let lv = self.elab_lvalue(out, env)?;
            self.design.assigns.push(Assign {
                lv,
                e: combined.clone(),
                delay: 0,
            });
        }
        Ok(())
    }

    fn interface_fields(
        &self,
        interface: &ast::Module,
        prefix: &str,
        decl_base: usize,
        inst_name: &str,
    ) -> Result<HashMap<String, NetEntry>, String> {
        let mut fields = HashMap::new();
        for item in &interface.items {
            if let ast::Item::Net { names, .. } = item {
                for (field, _) in names {
                    let full = format!("{}{}", prefix, field);
                    let Some(declaration) = self.design.net_decls[decl_base..]
                        .iter()
                        .find(|d| d.name == full)
                    else {
                        return Err(format!(
                            "interface instance '{}': missing signal '{}'",
                            inst_name, field
                        ));
                    };
                    fields.insert(
                        field.clone(),
                        NetEntry {
                            base: declaration.base,
                            width: declaration.width,
                            msb: declaration.width as i64 - 1,
                            lsb: 0,
                            type_name: None,
                            is_signed: false,
                            array: None,
                        },
                    );
                }
            }
        }
        Ok(fields)
    }

    #[allow(clippy::too_many_arguments)]
    fn elab_inst(
        &mut self,
        parent: &ast::Module,
        sub_name: &str,
        inst_name: &str,
        array_ranges: &[(Box<ast::Expr>, Box<ast::Expr>)],
        overrides: &[(Option<String>, ast::Expr)],
        conns: &[ast::Conn],
        env: &Env,
        interface_instances: &mut HashMap<String, InterfaceInstanceBinding>,
    ) -> Result<(), String> {
        let sub = match self.mods.get(sub_name) {
            Some(m) => *m,
            None => {
                return Err(format!(
                    "module '{}': instance '{}' of unknown module '{}'",
                    parent.name, inst_name, sub_name
                ))
            }
        };
        // Child parameter defaults, then overrides.
        let mut child_params: HashMap<String, i64> = HashMap::new();
        let mut declared_order: Vec<String> = Vec::new();
        for item in &sub.items {
            if let ast::Item::Params(list) = item {
                for (n, v) in list {
                    let val = self.const_eval(v, &child_params, &sub.name)?;
                    child_params.insert(n.clone(), val);
                    declared_order.push(n.clone());
                }
            }
        }
        for (i, (name, v)) in overrides.iter().enumerate() {
            let val = self.const_eval(v, env.params, &parent.name)?;
            match name {
                Some(n) => {
                    child_params.insert(n.clone(), val);
                }
                None => {
                    if i < declared_order.len() {
                        child_params.insert(declared_order[i].clone(), val);
                    }
                }
            }
        }

        let child_prefix = format!("{}{}.", env.prefix, inst_name);
        let child_net_start = self.design.n_nets;
        let decl_base = self.design.net_decls.len();
        let parent_path = if env.prefix.is_empty() {
            Some(self.design.name.clone())
        } else {
            Some(format!(
                "{}.{}",
                self.design.name,
                env.prefix.trim_end_matches('.')
            ))
        };

        if sub.is_interface {
            if !conns.is_empty() {
                return Err(format!(
                    "instance '{}': interface '{}' does not support port connections",
                    inst_name, sub_name
                ));
            }
            let binding = if !array_ranges.is_empty() {
                let mut dimensions = Vec::with_capacity(array_ranges.len());
                let mut total_elements = 1_usize;
                for range in array_ranges {
                    let (left, right) = range;
                    let base = self.const_eval(left, env.params, &parent.name)?;
                    let last = self.const_eval(right, env.params, &parent.name)?;
                    let step = if base <= last { 1 } else { -1 };
                    let length = (base - last).unsigned_abs() as usize + 1;
                    if length == 0 || length > 256 {
                        return Err(format!(
                            "interface instance '{}': each array length must be between 1 and 256",
                            inst_name
                        ));
                    }
                    total_elements = total_elements
                        .checked_mul(length)
                        .filter(|total| *total <= 4096)
                        .ok_or_else(|| {
                            format!(
                                "interface instance '{}': total array elements exceed 4096",
                                inst_name
                            )
                        })?;
                    dimensions.push(InterfaceArrayDimension { base, step, length });
                }

                let mut index_sets = vec![Vec::<i64>::new()];
                for dimension in &dimensions {
                    let mut next_index_sets = Vec::new();
                    for index_set in &index_sets {
                        for element in 0..dimension.length {
                            let mut next = index_set.clone();
                            next.push(dimension.base + dimension.step * element as i64);
                            next_index_sets.push(next);
                        }
                    }
                    index_sets = next_index_sets;
                }

                let mut elements = Vec::with_capacity(total_elements);
                for index_set in &index_sets {
                    let index_text = index_set
                        .iter()
                        .map(|index| index.to_string())
                        .collect::<Vec<_>>()
                        .join("_");
                    let element_prefix = format!("{}{}_{index_text}.", env.prefix, inst_name);
                    let element_net_start = self.design.n_nets;
                    let element_decl_base = self.design.net_decls.len();
                    self.elab_module(sub_name, &element_prefix, &child_params, &HashMap::new())?;
                    let element_net_end = self.design.n_nets;
                    elements.push(self.interface_fields(
                        sub,
                        &element_prefix,
                        element_decl_base,
                        inst_name,
                    )?);
                    let hierarchy_indexes = index_set
                        .iter()
                        .map(|index| format!("[{index}]"))
                        .collect::<String>();
                    self.design.hierarchy.push(HierarchicalInstance {
                        path: format!(
                            "{}.{}{hierarchy_indexes}",
                            parent_path.clone().unwrap_or_default(),
                            inst_name
                        ),
                        module: sub.name.clone(),
                        parent: parent_path.clone(),
                        depth: env.prefix.matches('.').count() + 1,
                        net_start: element_net_start,
                        net_end: element_net_end,
                        ports: Vec::new(),
                    });
                }
                InterfaceInstanceBinding::Array {
                    dimensions,
                    elements,
                }
            } else {
                self.elab_module(sub_name, &child_prefix, &child_params, &HashMap::new())?;
                let child_net_end = self.design.n_nets;
                let fields = self.interface_fields(sub, &child_prefix, decl_base, inst_name)?;
                self.design.hierarchy.push(HierarchicalInstance {
                    path: format!("{}.{}", parent_path.clone().unwrap_or_default(), inst_name),
                    module: sub.name.clone(),
                    parent: parent_path,
                    depth: env.prefix.matches('.').count() + 1,
                    net_start: child_net_start,
                    net_end: child_net_end,
                    ports: Vec::new(),
                });
                InterfaceInstanceBinding::Single(fields)
            };
            if interface_instances
                .insert(inst_name.to_string(), binding)
                .is_some()
            {
                return Err(format!("duplicate interface instance '{}'", inst_name));
            }
            return Ok(());
        }
        if !array_ranges.is_empty() {
            return Err(format!(
                "instance '{}': module instance arrays are not yet supported",
                inst_name
            ));
        }

        // Child port list in declaration order.
        let sub_ports: Vec<(String, ast::DirSrc)> = {
            let mut dirs: HashMap<String, ast::DirSrc> = HashMap::new();
            let mut order: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for item in &sub.items {
                if let ast::Item::Port {
                    dir,
                    interface_name: None,
                    names,
                    ..
                } = item
                {
                    for n in names {
                        dirs.insert(n.clone(), *dir);
                        if seen.insert(n.clone()) {
                            order.push(n.clone());
                        }
                    }
                }
            }
            order.into_iter().map(|n| (n.clone(), dirs[&n])).collect()
        };

        let mut child_interfaces = HashMap::new();
        let mut interface_port_index = 0_usize;
        for item in &sub.items {
            if let ast::Item::Port {
                interface_name: Some(definition),
                names,
                ..
            } = item
            {
                for local_name in names {
                    let expression = find_conn(conns, local_name, interface_port_index)
                        .ok_or_else(|| {
                            format!(
                                "instance '{}': missing interface port '{}'",
                                inst_name, local_name
                            )
                        })?;
                    let fields: HashMap<String, NetEntry> = match expression {
                        ast::Expr::Ident(object_name) => interface_instances
                            .get(object_name)
                            .ok_or_else(|| {
                                format!(
                                    "instance '{}': '{}' is not an interface object",
                                    inst_name, object_name
                                )
                            })?
                            .select(&[])?
                            .clone(),
                        ast::Expr::Select(object_name, index) => {
                            let index = self.const_eval(index, env.params, &parent.name)?;
                            interface_instances
                                .get(object_name)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': '{}' is not an interface object",
                                        inst_name, object_name
                                    )
                                })?
                                .select(&[index])?
                                .clone()
                        }
                        ast::Expr::ArraySelect(object_name, indexes) => {
                            let mut resolved = Vec::with_capacity(indexes.len());
                            for index in indexes {
                                resolved.push(self.const_eval(index, env.params, &parent.name)?);
                            }
                            interface_instances
                                .get(object_name)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': '{}' is not an interface object",
                                        inst_name, object_name
                                    )
                                })?
                                .select(&resolved)?
                                .clone()
                        }
                        other => {
                            return Err(format!(
                                "instance '{}': interface port '{}' must connect an interface object or array element, found {:?}",
                                inst_name, local_name, other
                            ));
                        }
                    };
                    let interface = self.mods.get(definition).ok_or_else(|| {
                        format!(
                            "instance '{}': unknown interface '{}'",
                            inst_name, definition
                        )
                    })?;
                    if !interface.is_interface {
                        return Err(format!("'{}' is not an interface", definition));
                    }
                    child_interfaces.insert(local_name.clone(), fields);
                }
                interface_port_index += names.len();
            }
        }

        // Elaborate the child in place (flat design, prefixed names).
        self.elab_module(sub_name, &child_prefix, &child_params, &child_interfaces)?;
        let child_net_end = self.design.n_nets;

        // Locate child port nets.
        let mut port_nets: HashMap<String, (usize, usize)> = HashMap::new();
        for (n, _) in &sub_ports {
            let full = format!("{}{}", child_prefix, n);
            for d in &self.design.net_decls[decl_base..] {
                if d.name == full {
                    port_nets.insert(n.clone(), (d.base, d.width));
                    break;
                }
            }
        }

        let ports = sub_ports
            .iter()
            .filter_map(|(name, direction)| {
                let (base, width) = port_nets.get(name).copied()?;
                Some(HierarchicalPort {
                    name: name.clone(),
                    direction: match direction {
                        ast::DirSrc::In => Dir::In,
                        ast::DirSrc::Out => Dir::Out,
                        ast::DirSrc::InOut => Dir::In,
                    },
                    base,
                    width,
                    connections: vec![None; width],
                })
            })
            .collect::<Vec<_>>();
        let hierarchy_index = self.design.hierarchy.len();
        self.design.hierarchy.push(HierarchicalInstance {
            path: format!("{}.{}", parent_path.clone().unwrap_or_default(), inst_name),
            module: sub.name.clone(),
            parent: parent_path,
            depth: env.prefix.matches('.').count() + 1,
            net_start: child_net_start,
            net_end: child_net_end,
            ports,
        });
        let mut port_connections: HashMap<String, Vec<Option<usize>>> = sub_ports
            .iter()
            .filter_map(|(name, _)| {
                port_nets
                    .get(name)
                    .map(|(_, width)| (name.clone(), vec![None; *width]))
            })
            .collect();

        // Connect ports.
        for (i, (pname, pdir)) in sub_ports.iter().enumerate() {
            let (base, pwidth) = match port_nets.get(pname) {
                Some(v) => *v,
                None => continue,
            };
            let lv = LValue {
                net: base,
                decl_width: pwidth,
                range: full_range(pwidth),
            };
            if let Some(expr) = find_conn(conns, pname, i) {
                match pdir {
                    ast::DirSrc::In => {
                        let e = self.elab_expr(expr, env)?;
                        if let ExprIR::Ref { net, range, .. } = &e {
                            let parent_bits = range_bits(net, range);
                            if let Some(connections) = port_connections.get_mut(pname) {
                                for (connection, parent_bit) in
                                    connections.iter_mut().zip(parent_bits)
                                {
                                    *connection = Some(parent_bit);
                                }
                            }
                        }
                        self.design.assigns.push(Assign { lv, e, delay: 0 });
                    }
                    ast::DirSrc::Out => {
                        let target = self.elab_lvalue(expr, env).map_err(|_| {
                            format!(
                                "instance '{}': output port '{}' must connect to a net",
                                inst_name, pname
                            )
                        })?;
                        let parent_bits = range_bits(&target.net, &target.range);
                        if let Some(connections) = port_connections.get_mut(pname) {
                            for (connection, parent_bit) in connections.iter_mut().zip(parent_bits)
                            {
                                *connection = Some(parent_bit);
                            }
                        }
                        let e = ExprIR::Ref {
                            net: base,
                            decl_width: pwidth,
                            range: full_range(pwidth),
                            signed: false,
                        };
                        self.design.assigns.push(Assign {
                            lv: target,
                            e,
                            delay: 0,
                        });
                    }
                    ast::DirSrc::InOut => {
                        return Err(format!(
                            "instance '{}': inout ports are not supported",
                            inst_name
                        ))
                    }
                }
            }
        }
        for port in &mut self.design.hierarchy[hierarchy_index].ports {
            if let Some(connections) = port_connections.remove(&port.name) {
                port.connections = connections;
            }
        }
        Ok(())
    }

    fn elab_sens(&self, sens: &ast::Sens, env: &Env) -> Result<SensIR, String> {
        Ok(match sens {
            ast::Sens::Comb => SensIR::Comb,
            ast::Sens::Posedge(e) => SensIR::Posedge(self.single_net(e, env)?),
            ast::Sens::Negedge(e) => SensIR::Negedge(self.single_net(e, env)?),
            ast::Sens::List(v) => SensIR::List(
                v.iter()
                    .map(|s| self.elab_sens(s, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        })
    }

    fn single_net(&self, e: &ast::Expr, env: &Env) -> Result<usize, String> {
        let ir = self.elab_expr(e, env)?;
        match ir {
            ExprIR::Ref {
                net,
                decl_width,
                range,
                signed: _,
            } => {
                if decl_width == 1 {
                    Ok(net)
                } else if range.width() == 1 {
                    Ok(net
                        + match range {
                            Range::Bit(b) => b,
                            Range::Span { lsb, .. } => lsb,
                        })
                } else {
                    Err("edge-sensitive expression must be 1 bit wide".into())
                }
            }
            _ => Err("edge-sensitive expression must be a net".into()),
        }
    }

    fn dynamic_array_entry(&self, name: &str, env: &Env) -> Result<(NetEntry, NetArray), String> {
        let entry = env.net(name)?;
        let array = entry
            .array
            .clone()
            .ok_or_else(|| format!("'{name}' is not a dynamic array"))?;
        if !array.dynamic {
            return Err(format!("'{name}' is not a dynamic array"));
        }
        Ok((entry.clone(), array))
    }

    fn dynamic_array_new(
        &self,
        name: &str,
        size: &ast::Expr,
        nonblocking: bool,
        env: &Env,
    ) -> Result<StmtIR, String> {
        let (entry, array) = self.dynamic_array_entry(name, env)?;
        let mut size_value = self.elab_expr(size, env)?;
        size_value = if size_value.width() < 9 {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(9 - size_value.width())),
                size_value,
            ])
        } else if size_value.width() > 9 {
            ExprIR::Index {
                e: Box::new(size_value),
                msb: 8,
                lsb: 0,
            }
        } else {
            size_value
        };
        let array_lvalue = LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let size_lvalue = LValue {
            net: array.dynamic_size_base,
            decl_width: 9,
            range: full_range(9),
        };
        let assignment = |lv: LValue, value: ExprIR| {
            if nonblocking {
                StmtIR::NBl { lv, e: value }
            } else {
                StmtIR::Bl { lv, e: value }
            }
        };
        Ok(StmtIR::Block(vec![
            assignment(array_lvalue, ExprIR::Const(LogicWord::zero(entry.width))),
            assignment(size_lvalue, size_value),
        ]))
    }

    fn dynamic_array_method(
        &self,
        object: &str,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        if method != "delete" || !arguments.is_empty() {
            return Err(format!(
                "unsupported dynamic-array method '{object}.{method}'"
            ));
        }
        let (entry, array) = self.dynamic_array_entry(object, env)?;
        Ok(StmtIR::Block(vec![
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: ExprIR::Const(LogicWord::zero(entry.width)),
            },
            StmtIR::Bl {
                lv: LValue {
                    net: array.dynamic_size_base,
                    decl_width: 9,
                    range: full_range(9),
                },
                e: ExprIR::Const(LogicWord::zero(9)),
            },
        ]))
    }

    // ---------- SystemVerilog class elaboration ----------

    /// Class inheritance chain from base to derived.
    fn class_chain(&self, name: &str) -> Result<Vec<String>, String> {
        let mut chain = Vec::new();
        let mut current = name.to_string();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return Err(format!("class inheritance cycle through '{}'", current));
            }
            let Some(definition) = self.classes.get(&current) else {
                if chain.is_empty() {
                    return Err(format!("unknown class '{}'", name));
                }
                return Err(format!(
                    "class '{}' extends unknown class '{}'",
                    name, current
                ));
            };
            chain.push(current.clone());
            match &definition.extends {
                Some(base) => current = base.clone(),
                None => break,
            }
        }
        chain.reverse();
        Ok(chain)
    }

    fn find_class_field(
        &self,
        class: &str,
        field: &str,
    ) -> Result<Option<(String, &ast::ClassProperty)>, String> {
        let chain = self.class_chain(class)?;
        for owner in chain {
            let definition = &self.classes[&owner];
            for property in &definition.properties {
                if property.names.iter().any(|(name, _)| name == field) {
                    return Ok(Some((owner, property)));
                }
            }
        }
        Ok(None)
    }

    fn find_class_method(
        &self,
        class: &str,
        method: &str,
    ) -> Result<Option<(String, &ast::FunctionDef)>, String> {
        let chain = self.class_chain(class)?;
        // Method dispatch starts at the most-derived class.
        for owner in chain.iter().rev() {
            let definition = &self.classes[owner.as_str()];
            for candidate in &definition.methods {
                if candidate.name == method {
                    return Ok(Some((owner.clone(), candidate)));
                }
            }
        }
        Ok(None)
    }

    fn static_field_net(class: &str, field: &str) -> String {
        format!("__{class}__{field}")
    }

    fn resolve_class_path(
        &self,
        env: &Env<'_>,
        path: &[String],
    ) -> Result<ClassPathTarget, String> {
        let Some(first) = path.first() else {
            return Err("empty class member path".into());
        };
        let (mut prefix, mut class, mut flag_net): (String, String, String);
        if first == "this" || first == "super" {
            let (handle, dynamic_class) = env
                .this_handle
                .ok_or_else(|| format!("'{first}' is valid only inside a class method"))?;
            prefix = env.canon(handle);
            class = dynamic_class.to_string();
            flag_net = prefix.clone();
        } else if let Some(handle_class) = env.class_handles.get(first) {
            let canonical = env.canon(first);
            prefix = canonical.clone();
            class = handle_class.clone();
            flag_net = canonical;
        } else if env.class_aliases.borrow().contains_key(first) {
            let canonical = env.canon(first);
            if let Some(handle_class) = env.class_handles.get(&canonical) {
                prefix = canonical.clone();
                class = handle_class.clone();
                flag_net = canonical;
            } else {
                return Err(format!("'{first}' is not a class handle or class name"));
            }
        } else if self.classes.contains_key(first) {
            // Static class access `ClassName.field`.
            if path.len() != 2 {
                return Err(format!(
                    "class name '{first}' must select exactly one static property"
                ));
            }
            let field = &path[1];
            let (owner, property) = self
                .find_class_field(first, field)?
                .ok_or_else(|| format!("class '{first}' has no property '{field}'"))?;
            if !Self::is_static_qual(property) {
                return Err(format!(
                    "class property '{first}.{field}' is not static; access it through a handle"
                ));
            }
            return Ok(ClassPathTarget::Field(Self::static_field_net(
                &owner, field,
            )));
        } else {
            return Err(format!("'{first}' is not a class handle or class name"));
        }
        for (index, segment) in path.iter().enumerate().skip(1) {
            let is_last = index + 1 == path.len();
            let (owner, property) = self
                .find_class_field(&class, segment)?
                .ok_or_else(|| format!("class '{class}' has no property '{segment}'"))?;
            if Self::is_static_qual(property) {
                if !is_last {
                    return Err(format!(
                        "cannot select further from static property '{segment}'"
                    ));
                }
                return Ok(ClassPathTarget::Field(Self::static_field_net(
                    &owner, segment,
                )));
            }
            if let Some(nested_class) = property.type_name.clone() {
                if self.classes.contains_key(&nested_class) {
                    let raw = format!("{prefix}_{segment}");
                    let canonical = env.canon(&raw);
                    flag_net = if env.class_handles.contains_key(&canonical) {
                        canonical.clone()
                    } else {
                        format!("__{canonical}")
                    };
                    prefix = canonical;
                    class = nested_class;
                    if is_last {
                        return Ok(ClassPathTarget::Handle {
                            flag_net,
                            prefix,
                            class,
                        });
                    }
                    continue;
                }
            }
            if !is_last {
                return Err(format!(
                    "cannot select further from non-class property '{segment}'"
                ));
            }
            return Ok(ClassPathTarget::Field(Self::handle_field_net(
                &prefix, segment,
            )));
        }
        Ok(ClassPathTarget::Handle {
            flag_net,
            prefix,
            class,
        })
    }

    fn handle_field_net(handle: &str, field: &str) -> String {
        format!("__{handle}_{field}")
    }

    fn is_static_qual(property: &ast::ClassProperty) -> bool {
        property.quals.contains(&ast::ClassPropQual::Static)
    }

    /// Resolve `obj.field` or `this.field` to a class storage net.
    fn class_field_reference(
        &self,
        env: &Env<'_>,
        object: &str,
        field: &str,
    ) -> Option<Result<(usize, usize, Range), String>> {
        if object == "this" || object == "super" {
            let (handle, class) = env.this_handle?;
            let handle = env.canon(handle);
            let result = (|| {
                let (owner, property) = self
                    .find_class_field(class, field)?
                    .ok_or_else(|| format!("class '{class}' has no property '{field}'"))?;
                if Self::is_static_qual(property) {
                    let net = Self::static_field_net(&owner, field);
                    let entry = env.net(&net)?;
                    return Ok((entry.base, entry.width, full_range(entry.width)));
                }
                let net = Self::handle_field_net(&handle, field);
                let entry = env.net(&net)?;
                Ok((entry.base, entry.width, full_range(entry.width)))
            })();
            return Some(result);
        }
        if let Some(class) = env.class_handles.get(object) {
            let canonical = env.canon(object);
            let result = (|| {
                let (owner, property) = self
                    .find_class_field(class, field)?
                    .ok_or_else(|| format!("class '{class}' has no property '{field}'"))?;
                if Self::is_static_qual(property) {
                    let net = Self::static_field_net(&owner, field);
                    let entry = env.net(&net)?;
                    return Ok((entry.base, entry.width, full_range(entry.width)));
                }
                let net = Self::handle_field_net(&canonical, field);
                let entry = env.net(&net)?;
                Ok((entry.base, entry.width, full_range(entry.width)))
            })();
            return Some(result);
        }
        if let Some(definition) = self.classes.get(object) {
            // Static access `ClassName.field`.
            let class = definition.name.clone();
            let result = (|| {
                let chain = self.class_chain(&class)?;
                for owner in chain {
                    let owner_definition = &self.classes[&owner];
                    for property in &owner_definition.properties {
                        if !Self::is_static_qual(property) {
                            continue;
                        }
                        if property.names.iter().any(|(name, _)| name == field) {
                            let net = Self::static_field_net(&owner, field);
                            let entry = env.net(&net)?;
                            return Ok((entry.base, entry.width, full_range(entry.width)));
                        }
                    }
                }
                Err(format!("class '{class}' has no static property '{field}'"))
            })();
            return Some(result);
        }
        None
    }

    fn resize_class_arg(&self, value: ExprIR, width: usize) -> ExprIR {
        if value.width() < width {
            ExprIR::Concat(vec![
                ExprIR::Const(LogicWord::zero(width - value.width())),
                value,
            ])
        } else if value.width() > width {
            ExprIR::Index {
                e: Box::new(value),
                msb: width - 1,
                lsb: 0,
            }
        } else {
            value
        }
    }

    /// 类类型形式参按引用绑定：形参名在方法体内别到实参句柄的组代表。
    /// 本子集在方法体内联期间保持绑定（形参名与用户句柄重名时以形参为准）。
    fn bind_class_formals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<(), String> {
        let actuals = self.expand_function_actuals(definition, actuals)?;
        for (((formal, _), type_name), actual) in definition
            .args
            .iter()
            .zip(&definition.arg_types)
            .zip(&actuals)
        {
            let Some(class_name) = type_name else {
                continue;
            };
            if !self.classes.contains_key(class_name) {
                continue;
            }
            // 实参可以是顶层句柄，也可以是嵌套路径（如 env.mon）。
            let canonical = match actual {
                ast::Expr::Ident(handle) => {
                    if !env.class_handles.contains_key(handle) {
                        return Err(format!(
                            "class-typed argument '{}' of '{}' must name a class handle",
                            formal, definition.name
                        ));
                    }
                    env.canon(handle)
                }
                ast::Expr::Member(object, field) => {
                    let path = vec![object.clone(), field.clone()];
                    match self.resolve_class_path(env, &path) {
                        Ok(ClassPathTarget::Handle { prefix, .. }) => prefix,
                        _ => {
                            return Err(format!(
                                "class-typed argument '{}' of '{}' must name a class handle",
                                formal, definition.name
                            ))
                        }
                    }
                }
                ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path) {
                    Ok(ClassPathTarget::Handle { prefix, .. }) => prefix,
                    _ => {
                        return Err(format!(
                            "class-typed argument '{}' of '{}' must name a class handle",
                            formal, definition.name
                        ))
                    }
                },
                _ => {
                    return Err(format!(
                        "class-typed argument '{}' of '{}' must be a handle",
                        formal, definition.name
                    ))
                }
            };
            env.class_aliases
                .borrow_mut()
                .insert(formal.clone(), canonical.clone());
        }
        Ok(())
    }

    fn expand_function_actuals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
    ) -> Result<Vec<ast::Expr>, String> {
        if actuals.len() != definition.args.len()
            && definition.arg_defaults.iter().all(Option::is_none)
        {
            return Err(format!(
                "method '{}' expects {} argument(s), received {}",
                definition.name,
                definition.args.len(),
                actuals.len()
            ));
        }
        if actuals.len() > definition.args.len() {
            return Err(format!(
                "method '{}' expects at most {} argument(s), received {}",
                definition.name,
                definition.args.len(),
                actuals.len()
            ));
        }
        let mut expanded = actuals.to_vec();
        for index in actuals.len()..definition.args.len() {
            let Some(default) = definition.arg_defaults.get(index).and_then(Option::as_ref) else {
                return Err(format!(
                    "method '{}' has no default for argument {}",
                    definition.name,
                    index + 1
                ));
            };
            expanded.push(default.clone());
        }
        Ok(expanded)
    }

    fn class_actuals(
        &self,
        definition: &ast::FunctionDef,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<HashMap<String, ExprIR>, String> {
        let actuals = self.expand_function_actuals(definition, actuals)?;
        let mut bound = HashMap::new();
        for ((formal, range), actual) in definition.args.iter().zip(&actuals) {
            let width = self.ast_range_width(range, env)?;
            let value = self.elab_expr(actual, env)?;
            bound.insert(formal.clone(), self.resize_class_arg(value, width));
        }
        Ok(bound)
    }

    /// Declare storage for every instance field of `handle`'s class and for
    /// every static field of every visible class. Returns synthetic initial
    /// items that seed static-field initializers.
    fn declare_class_storage(
        &mut self,
        prefix: &str,
        params: &HashMap<String, i64>,
        module_name: &str,
        nets: &mut HashMap<String, NetEntry>,
        static_storage_classes: &HashSet<String>,
    ) -> Result<Vec<ast::Item>, String> {
        let _ = params;
        let mut initials = Vec::new();
        let class_names: Vec<String> = self.classes.keys().cloned().collect();
        for class in class_names {
            if !static_storage_classes.contains(&class) {
                continue;
            }
            let chain = self.class_chain(&class)?;
            let mut static_initials = Vec::new();
            for owner in &chain {
                let definition = self.classes[owner].clone();
                for property in &definition.properties {
                    if !Self::is_static_qual(property) {
                        continue;
                    }
                    for (field, init) in &property.names {
                        let net_name = Self::static_field_net(owner, field);
                        let range = property.range.clone();
                        let entry = self.declare_net(
                            &net_name,
                            prefix,
                            &range,
                            &HashMap::new(),
                            true,
                            property.is_signed,
                            None,
                            property.type_name.clone().as_deref(),
                        )?;
                        nets.insert(net_name.clone(), entry.clone());
                        let value = init.clone().unwrap_or_else(|| ast::Expr::Num {
                            width: Some(entry.width),
                            word: LogicWord::zero(entry.width),
                            orig: "0".to_string(),
                        });
                        static_initials.push(ast::Stmt::BL {
                            lv: ast::Expr::Ident(net_name),
                            e: value,
                        });
                    }
                }
            }
            if !static_initials.is_empty() {
                initials.push(ast::Item::Initial {
                    body: ast::Stmt::Block(static_initials),
                });
            }
        }
        let _ = module_name;
        Ok(initials)
    }

    /// Declare the flattened per-object fields of one class handle.
    fn declare_handle_fields(
        &mut self,
        handle_prefix: &str,
        class: &str,
        prefix: &str,
        params: &HashMap<String, i64>,
        nets: &mut HashMap<String, NetEntry>,
        depth: usize,
    ) -> Result<(), String> {
        let _ = params;
        if depth > 8 {
            return Err(format!(
                "class composition nesting under '{handle_prefix}' exceeds 8 levels"
            ));
        }
        // 运行时类型标记（虚分派用）。
        let tid_name = type_id_net(handle_prefix);
        if !nets.contains_key(&tid_name) {
            let width = type_id_width(self.classes.len()).max(1);
            let range = Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, width as u64 - 1),
                    orig: (width - 1).to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            ));
            let entry = self.declare_net(
                &tid_name,
                prefix,
                &range,
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )?;
            nets.insert(tid_name.clone(), entry);
        }
        let oid_name = object_id_net(handle_prefix);
        if nets.get(&oid_name).is_none() {
            let range = Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, 31),
                    orig: "31".to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            ));
            let entry = self.declare_net(
                &oid_name,
                prefix,
                &range,
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )?;
            nets.insert(oid_name, entry);
        }
        let chain = self.class_chain(class)?;
        for owner in chain {
            let definition = self.classes[&owner].clone();
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, _init) in &property.names {
                    let net_name = Self::handle_field_net(handle_prefix, field);
                    if !nets.contains_key(&net_name) {
                        let entry = self.declare_net(
                            &net_name,
                            prefix,
                            &property.range.clone(),
                            &HashMap::new(),
                            true,
                            property.is_signed,
                            None,
                            property.type_name.clone().as_deref(),
                        )?;
                        nets.insert(net_name.clone(), entry);
                    }
                    if let Some(nested_class) = property.type_name.clone() {
                        if self.classes.contains_key(&nested_class) {
                            // A directly self-referential class property is a
                            // handle reference, not recursive inline storage.
                            if nested_class == class {
                                continue;
                            }
                            let nested_prefix = format!("{handle_prefix}_{field}");
                            self.declare_handle_fields(
                                &nested_prefix,
                                &nested_class,
                                prefix,
                                params,
                                nets,
                                depth + 1,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `h = new(args);` — allocate fields and run the constructor chain.
    #[allow(clippy::too_many_arguments)]
    fn class_shallow_copy_statement(
        &mut self,
        destination: &str,
        source: &str,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let source_canonical = env.canon(source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(source))
            .ok_or_else(|| format!("'{source}' is not a class handle"))?
            .clone();
        let destination_class = env
            .class_handles
            .get(destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        if source_class != destination_class {
            return Err(format!(
                "class shallow copy in this subset requires the exact source class '{}', not '{}'",
                destination_class, source_class
            ));
        }

        let promotion = self.detach_class_handle_alias(env, destination);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }

        let destination_entry = env.net(destination)?;
        let assignment = |lv: LValue, value: ExprIR| -> StmtIR {
            if nonblocking {
                StmtIR::NBl { lv, e: value }
            } else {
                StmtIR::Bl { lv, e: value }
            }
        };
        let lvalue = |entry: &NetEntry| LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let reference = |entry: &NetEntry| ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        };

        statements.push(assignment(
            lvalue(destination_entry),
            ExprIR::Const(LogicWord::ones(destination_entry.width)),
        ));
        let object_id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| "class object identity counter overflowed".to_string())?;
        if let Ok(destination_oid) = env.net(&object_id_net(destination)) {
            statements.push(assignment(
                lvalue(destination_oid),
                ExprIR::Const(LogicWord::from_u64(destination_oid.width, object_id)),
            ));
        }
        if let (Ok(destination_tid), Ok(source_tid)) = (
            env.net(&type_id_net(destination)),
            env.net(&type_id_net(&source_canonical)),
        ) {
            statements.push(assignment(lvalue(destination_tid), reference(source_tid)));
        }

        let mut property_aliases = Vec::new();
        for owner in self.class_chain(&source_class)? {
            let definition = self.classes[&owner].clone();
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, _) in &property.names {
                    if property
                        .type_name
                        .as_deref()
                        .is_some_and(|class| self.classes.contains_key(class))
                    {
                        let source_property = format!("{source_canonical}_{field}");
                        let source_property = env.canon(&source_property);
                        let destination_property = format!("{destination}_{field}");
                        property_aliases.push((destination_property, source_property));
                    } else {
                        let source_name = Self::handle_field_net(&source_canonical, field);
                        let destination_name = Self::handle_field_net(destination, field);
                        let (Some(source_entry), Some(destination_entry)) =
                            (env.nets.get(&source_name), env.nets.get(&destination_name))
                        else {
                            return Err(format!(
                                "missing class copy storage '{source_name}' or '{destination_name}'"
                            ));
                        };
                        statements.push(assignment(
                            lvalue(destination_entry),
                            reference(source_entry),
                        ));
                    }
                }
            }
        }
        if !property_aliases.is_empty() {
            let mut aliases = env.class_aliases.borrow_mut();
            for (destination_property, source_property) in property_aliases {
                aliases.insert(destination_property, source_property);
            }
        }
        Ok(StmtIR::Block(statements))
    }

    #[allow(clippy::too_many_arguments)]
    fn class_new_statement(
        &mut self,
        handle: &str,
        field_prefix: &str,
        handle_class: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let class = handle_class.to_string();
        // new 让该句柄指向新对象：若它原是别名组代表，则提升另一成员。
        let promotion = self.detach_class_handle_alias(env, handle);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }
        let canonical = env.canon(handle);
        let handle = canonical.as_str();
        let handle_entry = env.net(handle)?;
        let make = |net: usize, width: usize, value: ExprIR| {
            if nonblocking {
                StmtIR::NBl {
                    lv: LValue {
                        net,
                        decl_width: width,
                        range: full_range(width),
                    },
                    e: value,
                }
            } else {
                StmtIR::Bl {
                    lv: LValue {
                        net,
                        decl_width: width,
                        range: full_range(width),
                    },
                    e: value,
                }
            }
        };
        // Mark the handle allocated and record its runtime type id.
        statements.push(make(
            handle_entry.base,
            handle_entry.width,
            ExprIR::Const(LogicWord::ones(handle_entry.width)),
        ));
        let object_id = self.next_object_id;
        self.next_object_id = self
            .next_object_id
            .checked_add(1)
            .ok_or_else(|| "class object identity counter overflowed".to_string())?;
        if let Ok(oid_entry) = env.net(&object_id_net(field_prefix)) {
            statements.push(make(
                oid_entry.base,
                oid_entry.width,
                ExprIR::Const(LogicWord::from_u64(oid_entry.width, object_id)),
            ));
        }
        let ids = class_type_ids(&self.classes);
        let Some(&type_id) = ids.get(&class) else {
            return Err(format!("unknown class '{class}'"));
        };
        if let Ok(tid_entry) = env.net(&type_id_net(field_prefix)) {
            statements.push(make(
                tid_entry.base,
                tid_entry.width,
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
            ));
        }
        let chain = self.class_chain(&class)?;
        for owner in &chain {
            let definition = self.classes[owner].clone();
            // Property initializers execute before the constructor body.
            for property in &definition.properties {
                if Self::is_static_qual(property) {
                    continue;
                }
                for (field, init) in &property.names {
                    let net_name = Self::handle_field_net(field_prefix, field);
                    let entry = env.net(&net_name)?;
                    let value = match init {
                        Some(expression) => {
                            let empty = HashMap::new();
                            let child = self.class_env(
                                env,
                                field_prefix,
                                class.as_str(),
                                None,
                                &empty,
                                None,
                            );
                            self.elab_expr(expression, &child)?
                        }
                        None => ExprIR::Const(LogicWord::zero(entry.width)),
                    };
                    let value = self.resize_class_arg(value, entry.width);
                    statements.push(make(entry.base, entry.width, value));
                }
            }
            // Run this class's constructor if present.
            if let Some(constructor) = definition
                .methods
                .iter()
                .find(|method| method.name == "new")
                .cloned()
            {
                // If the immediately derived class explicitly invokes
                // `super.new(...)`, skip the implicit base-constructor run to
                // avoid constructing the base twice.
                let derived_ctor_calls_super = chain
                    .iter()
                    .position(|c| c == owner)
                    .and_then(|index| chain.get(index + 1))
                    .and_then(|derived| self.classes.get(derived))
                    .and_then(|derived_definition| {
                        derived_definition
                            .methods
                            .iter()
                            .find(|method| method.name == "new")
                    })
                    .is_some_and(|derived_ctor| ctor_body_calls_super(&derived_ctor.body));
                if derived_ctor_calls_super && owner != &class {
                    continue;
                }
                let ctor_actuals: Vec<ast::Expr> = if owner == &class {
                    actuals.to_vec()
                } else {
                    // Implicit base construction uses the actuals that the
                    // immediately derived class forwards via `extends`.
                    chain
                        .iter()
                        .position(|c| c == owner)
                        .and_then(|index| chain.get(index + 1))
                        .and_then(|derived| self.classes.get(derived))
                        .map(|derived_definition| derived_definition.extends_args.clone())
                        .unwrap_or_default()
                };
                self.bind_class_formals(&constructor, &ctor_actuals, env)?;
                let bound = self.class_actuals(&constructor, &ctor_actuals, env)?;
                let child = self.class_env(
                    env,
                    field_prefix,
                    class.as_str(),
                    Some(owner.as_str()),
                    &bound,
                    None,
                );
                for statement in &constructor.body {
                    let ir = self.elab_stmt(statement, &child)?;
                    statements.push(ir);
                }
            }
        }
        Ok(StmtIR::Block(statements))
    }

    fn class_env<'b, 'c>(
        &self,
        parent: &Env<'b>,
        handle: &'c str,
        class: &'c str,
        owner: Option<&'c str>,
        args: &'c HashMap<String, ExprIR>,
        method_return: Option<(&'c str, usize, usize)>,
    ) -> Env<'c>
    where
        'b: 'c,
    {
        Env {
            class_aliases: std::rc::Rc::clone(&parent.class_aliases),
            nets: parent.nets,
            interface_ports: parent.interface_ports,
            interface_directions: parent.interface_directions,
            params: parent.params,
            types: parent.types,
            functions: parent.functions,
            module_name: parent.module_name,
            prefix: parent.prefix,
            class_handles: parent.class_handles,
            this_handle: Some((handle, class)),
            method_owner: owner,
            method_args: args,
            method_return,
        }
    }

    fn run_class_constructor(
        &mut self,
        handle: &str,
        dynamic_class: &str,
        owner: &str,
        constructor: &ast::FunctionDef,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Vec<StmtIR>, String> {
        self.bind_class_formals(constructor, arguments, env)?;
        let bound = self.class_actuals(constructor, arguments, env)?;
        let child = self.class_env(env, handle, dynamic_class, Some(owner), &bound, None);
        let mut statements = Vec::new();
        for statement in &constructor.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(statements)
    }

    /// Statement-form method call `h.m(args);` (or `this.m`/`super.m`).
    fn class_method_statement(
        &mut self,
        object: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let (handle, class, owner_hint) = if object == "this" || object == "super" {
            let (handle, class) = env
                .this_handle
                .ok_or_else(|| "'{object}' is valid only inside a class method".to_string())?;
            let start = if object == "super" {
                env.method_owner
                    .and_then(|owner| self.classes.get(owner))
                    .and_then(|definition| definition.extends.clone())
                    .ok_or_else(|| "super call has no base class".to_string())?
            } else {
                class.to_string()
            };
            (handle.to_string(), class.to_string(), start)
        } else {
            let class = env
                .class_handles
                .get(object)
                .cloned()
                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
            (object.to_string(), class.clone(), class)
        };
        self.class_method_statement_on(
            handle.as_str(),
            class.as_str(),
            owner_hint.as_str(),
            object == "super",
            method,
            actuals,
            env,
        )
    }

    #[allow(clippy::too_many_arguments)]
    /// If `class::method` is virtual, build a runtime-type dispatch over the
    /// handle's type-id net. Each arm resolves the most-derived override for
    /// one concrete derived class (overrides may reference base fields only).
    #[allow(clippy::type_complexity)]
    fn virtual_dispatch_arms(
        &self,
        _handle: &str,
        class: &str,
        method: &str,
    ) -> Result<Option<(usize, Vec<(u32, String)>)>, String> {
        let Some((_, base_def)) = self.find_class_method(class, method)? else {
            return Ok(None);
        };
        if !base_def.is_virtual {
            return Ok(None);
        }
        let ids = class_type_ids(&self.classes);
        let tid_width = type_id_width(self.classes.len()).max(1);
        let mut arms = Vec::new();
        for derived in derived_classes(&self.classes, class) {
            let Some((owner, _)) = self.find_class_method(derived, method)? else {
                continue;
            };
            let Some(&id) = ids.get(derived) else {
                continue;
            };
            arms.push((id, owner.to_string()));
        }
        Ok(Some((tid_width, arms)))
    }

    #[allow(clippy::too_many_arguments)]
    fn virtual_dispatch_statement(
        &mut self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Option<StmtIR>, String> {
        let Some((tid_width, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let _ = tid_width;
        let mut cases = Vec::new();
        for (type_id, owner) in arms {
            let Some(definition) = self
                .classes
                .get(owner.as_str())
                .and_then(|d| d.methods.iter().find(|m| m.name == method))
            else {
                continue;
            };
            let definition = definition.clone();
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let child = self.class_env(env, &canonical, &owner, Some(&owner), &bound, None);
            let mut body = Vec::new();
            for statement in &definition.body {
                body.push(self.elab_stmt(statement, &child)?);
            }
            cases.push((
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
                StmtIR::Block(body),
            ));
        }
        Ok(Some(StmtIR::Case {
            e: ExprIR::Ref {
                net: tid_entry.base,
                decl_width: tid_entry.width,
                range: full_range(tid_entry.width),
                signed: false,
            },
            arms: cases,
            dflt: Some(Box::new(StmtIR::Block(Vec::new()))),
        }))
    }

    fn virtual_dispatch_expression(
        &self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        // 表达式形式的虚分派需要每个候选体是单赋值表达式；用嵌套
        // 条件表达式按 tid 选择。
        let Some((_, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        if arms.is_empty() {
            return Ok(None);
        }
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let selector = ExprIR::Ref {
            net: tid_entry.base,
            decl_width: tid_entry.width,
            range: full_range(tid_entry.width),
            signed: false,
        };
        let mut result: Option<ExprIR> = None;
        for (type_id, owner) in arms.iter().rev() {
            let Some(definition) = self
                .classes
                .get(owner.as_str())
                .and_then(|d| d.methods.iter().find(|m| m.name == method))
            else {
                continue;
            };
            let mut assigned = None;
            for statement in &definition.body {
                match statement {
                    ast::Stmt::BL {
                        lv: ast::Expr::Ident(name),
                        e,
                    } if name == &definition.name => assigned = Some(e),
                    _ => return Err(format!(
                        "virtual method '{}.{}' used in an expression must be a single result assignment",
                        class, method
                    )),
                }
            }
            let Some(expression) = assigned else {
                return Err(format!(
                    "virtual method '{}.{}' used in an expression must assign its result",
                    class, method
                ));
            };
            self.bind_class_formals(definition, actuals, env)?;
            let bound = self.class_actuals(definition, actuals, env)?;
            let child = self.class_env(env, &canonical, owner, Some(owner), &bound, None);
            let value = self.elab_expr(expression, &child)?;
            let cond = ExprIR::Bin {
                op: BinOp::Eq,
                a: Box::new(selector.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(
                    tid_entry.width,
                    *type_id as u64,
                ))),
            };
            result = Some(match result {
                Some(else_value) => ExprIR::Cond {
                    c: Box::new(cond),
                    t: Box::new(value),
                    f: Box::new(else_value),
                },
                None => value,
            });
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn class_method_statement_on(
        &mut self,
        handle: &str,
        class: &str,
        owner_hint: &str,
        from_super: bool,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        // 虚方法经句柄调用：按运行时类型标记分派（super. 显式静态绑定）。
        if !from_super {
            if let Some(dispatch) =
                self.virtual_dispatch_statement(handle, class, method, actuals, env)?
            {
                return Ok(dispatch);
            }
        }
        if method == "new" {
            if !from_super {
                return Err(
                    "constructors are invoked by assignment, as in 'h = new(args);'".to_string(),
                );
            }
            // Explicit `super.new(args);` inside a derived constructor.
            let base = owner_hint;
            let Some((base_owner, base_ctor)) = self.find_class_method(base, "new")? else {
                return Ok(StmtIR::Block(Vec::new()));
            };
            let base_ctor = base_ctor.clone();
            let base_owner = base_owner.clone();
            let statements =
                self.run_class_constructor(handle, class, &base_owner, &base_ctor, actuals, env)?;
            return Ok(StmtIR::Block(statements));
        }
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let definition = definition.clone();
        let owner = owner.clone();
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        // Value-returning methods invoked as statements write their result to
        // a scratch net so return assignments still elaborate.
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__method_ret_{}_{}", handle, method);
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            )),
            &HashMap::new(),
            true,
            false,
            None,
            None,
        )?;
        // 引用别名后，句柄的运行时类取其组代表句柄的声明类（嵌套
        // 前缀无直接表项时回退到静态类）。
        let canonical_handle = env.canon(handle);
        let dynamic_class = env
            .class_handles
            .get(&canonical_handle)
            .map(String::as_str)
            .unwrap_or(class);
        let child = self.class_env(
            env,
            handle,
            dynamic_class,
            Some(owner.as_str()),
            &bound,
            Some((definition.name.as_str(), scratch.base, scratch.width)),
        );
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(StmtIR::Block(statements))
    }

    /// Expression-form method call `h.m(args)` limited to methods whose body
    /// assigns the method name exactly once from a single expression.
    fn class_method_expression_on(
        &self,
        handle: &str,
        class: &str,
        owner_hint: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        if let Some(dispatch) =
            self.virtual_dispatch_expression(handle, class, method, actuals, env)?
        {
            return Ok(dispatch);
        }
        if method == "new" {
            return Err(
                "constructors are invoked by assignment, as in 'h = new(args);'".to_string(),
            );
        }
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let mut assigned = None;
        for statement in &definition.body {
            match statement {
                ast::Stmt::BL {
                    lv: ast::Expr::Ident(name),
                    e,
                } if name == &definition.name => {
                    if assigned.is_some() {
                        return Err(format!(
                            "class method '{}.{}' used in an expression must assign its result exactly once",
                            class, method
                        ));
                    }
                    assigned = Some(e);
                }
                _ => {
                    return Err(format!(
                    "class method '{}.{}' used in an expression must be a single result assignment",
                    class, method
                ))
                }
            }
        }
        let expression = assigned.ok_or_else(|| {
            format!(
                "class method '{}.{}' used in an expression must assign its result",
                class, method
            )
        })?;
        self.bind_class_formals(definition, actuals, env)?;
        let bound = self.class_actuals(definition, actuals, env)?;
        let child = self.class_env(env, handle, class, Some(owner.as_str()), &bound, None);
        self.elab_expr(expression, &child)
    }

    fn class_method_expression(
        &self,
        object: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let (handle, class, owner_hint) = if object == "this" || object == "super" {
            let (handle, class) = env
                .this_handle
                .ok_or_else(|| format!("'{object}' is valid only inside a class method"))?;
            let start = if object == "super" {
                env.method_owner
                    .and_then(|owner| self.classes.get(owner))
                    .and_then(|definition| definition.extends.clone())
                    .ok_or_else(|| "super call has no base class".to_string())?
            } else {
                class.to_string()
            };
            (handle.to_string(), class.to_string(), start)
        } else {
            let class = env
                .class_handles
                .get(object)
                .cloned()
                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
            (object.to_string(), class.clone(), class)
        };
        self.class_method_expression_on(&handle, &class, &owner_hint, method, actuals, env)
    }

    fn class_static_method_statement(
        &mut self,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(class, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        if !definition.is_static {
            return Err(format!(
                "non-static method '{}.{}' must be called through a class handle",
                class, method
            ));
        }
        let definition = definition.clone();
        let owner = owner.clone();
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        let owner_name = owner.clone();
        let child = Env {
            class_aliases: std::rc::Rc::clone(&env.class_aliases),
            nets: env.nets,
            interface_ports: env.interface_ports,
            interface_directions: env.interface_directions,
            params: env.params,
            types: env.types,
            functions: env.functions,
            module_name: env.module_name,
            prefix: env.prefix,
            class_handles: env.class_handles,
            this_handle: None,
            method_owner: Some(owner_name.as_str()),
            method_args: &bound,
            method_return: None,
        };
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        Ok(StmtIR::Block(statements))
    }

    fn class_static_method_value_assignment(
        &mut self,
        target: LValue,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(class, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        if !definition.is_static {
            return Err(format!(
                "non-static method '{}.{}' must be called through a class handle",
                class, method
            ));
        }
        if definition.return_range.is_none() {
            return Err(format!(
                "void static method '{}.{}' cannot be used as a value",
                class, method
            ));
        }
        let definition = definition.clone();
        let owner = owner.clone();
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__static_method_ret_{class}_{method}");
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            )),
            &HashMap::new(),
            true,
            false,
            None,
            None,
        )?;
        self.bind_class_formals(&definition, actuals, env)?;
        let bound = self.class_actuals(&definition, actuals, env)?;
        let owner_name = owner.clone();
        let method_name = definition.name.clone();
        let child = Env {
            class_aliases: std::rc::Rc::clone(&env.class_aliases),
            nets: env.nets,
            interface_ports: env.interface_ports,
            interface_directions: env.interface_directions,
            params: env.params,
            types: env.types,
            functions: env.functions,
            module_name: env.module_name,
            prefix: env.prefix,
            class_handles: env.class_handles,
            this_handle: None,
            method_owner: Some(owner_name.as_str()),
            method_args: &bound,
            method_return: Some((method_name.as_str(), scratch.base, scratch.width)),
        };
        let mut statements = Vec::new();
        for statement in &definition.body {
            statements.push(self.elab_stmt(statement, &child)?);
        }
        let result = ExprIR::Ref {
            net: scratch.base,
            decl_width: scratch.width,
            range: full_range(scratch.width),
            signed: false,
        };
        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: result,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: result,
            }
        };
        statements.push(assignment);
        Ok(StmtIR::Block(statements))
    }

    #[allow(clippy::too_many_arguments)]
    fn class_method_value_assignment(
        &mut self,
        lv: &ast::Expr,
        handle: &str,
        class: &str,
        owner_hint: &str,
        from_super: bool,
        method: &str,
        actuals: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let Some((owner, definition)) = self.find_class_method(owner_hint, method)? else {
            return Err(format!("class '{class}' has no method '{method}'"));
        };
        let definition = definition.clone();
        let owner = owner.clone();
        if definition.return_range.is_none() {
            return Err(format!(
                "void class method '{}.{}' cannot be used as a value",
                class, method
            ));
        }

        let target = self.elab_lvalue(lv, env)?;
        let return_width = self
            .ast_range_width(&definition.return_range, env)
            .unwrap_or(32);
        let scratch_name = format!("__method_value_{}_{}", handle, method);
        let scratch = self.declare_net(
            &scratch_name,
            env.prefix,
            &Some((
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::from_u64(32, return_width as u64 - 1),
                    orig: (return_width - 1).to_string(),
                }),
                Box::new(ast::Expr::Num {
                    width: Some(32),
                    word: LogicWord::zero(32),
                    orig: "0".to_string(),
                }),
            )),
            &HashMap::new(),
            true,
            false,
            None,
            None,
        )?;
        let result = ExprIR::Ref {
            net: scratch.base,
            decl_width: scratch.width,
            range: full_range(scratch.width),
            signed: false,
        };

        let body = if !from_super && definition.is_virtual {
            match self.virtual_dispatch_value_body(
                handle,
                class,
                method,
                actuals,
                scratch.base,
                scratch.width,
                env,
            )? {
                Some(body) => body,
                None => {
                    return Err(format!(
                        "virtual method '{}.{}' has no runtime type identity",
                        class, method
                    ))
                }
            }
        } else {
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let canonical_handle = env.canon(handle);
            let dynamic_class = env
                .class_handles
                .get(&canonical_handle)
                .map(String::as_str)
                .unwrap_or(class);
            let child = self.class_env(
                env,
                handle,
                dynamic_class,
                Some(owner.as_str()),
                &bound,
                Some((definition.name.as_str(), scratch.base, scratch.width)),
            );
            let mut statements = Vec::new();
            for statement in &definition.body {
                statements.push(self.elab_stmt(statement, &child)?);
            }
            StmtIR::Block(statements)
        };

        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: result,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: result,
            }
        };
        Ok(StmtIR::Block(vec![body, assignment]))
    }

    #[allow(clippy::too_many_arguments)]
    fn virtual_dispatch_value_body(
        &mut self,
        handle: &str,
        class: &str,
        method: &str,
        actuals: &[ast::Expr],
        result_base: usize,
        result_width: usize,
        env: &Env<'_>,
    ) -> Result<Option<StmtIR>, String> {
        let Some((_, arms)) = self.virtual_dispatch_arms(handle, class, method)? else {
            return Ok(None);
        };
        let canonical = env.canon(handle);
        let tid_net = type_id_net(&canonical);
        let Ok(tid_entry) = env.net(&tid_net) else {
            return Ok(None);
        };
        let mut cases = Vec::new();
        for (type_id, owner) in arms {
            let Some(definition) = self.classes.get(owner.as_str()).and_then(|definitions| {
                definitions
                    .methods
                    .iter()
                    .find(|definition| definition.name == method)
            }) else {
                continue;
            };
            let definition = definition.clone();
            self.bind_class_formals(&definition, actuals, env)?;
            let bound = self.class_actuals(&definition, actuals, env)?;
            let child = self.class_env(
                env,
                &canonical,
                owner.as_str(),
                Some(owner.as_str()),
                &bound,
                Some((definition.name.as_str(), result_base, result_width)),
            );
            let mut body = Vec::new();
            for statement in &definition.body {
                body.push(self.elab_stmt(statement, &child)?);
            }
            cases.push((
                ExprIR::Const(LogicWord::from_u64(tid_entry.width, type_id as u64)),
                StmtIR::Block(body),
            ));
        }
        Ok(Some(StmtIR::Case {
            e: ExprIR::Ref {
                net: tid_entry.base,
                decl_width: tid_entry.width,
                range: full_range(tid_entry.width),
                signed: false,
            },
            arms: cases,
            dflt: Some(Box::new(StmtIR::Block(Vec::new()))),
        }))
    }

    /// `h2 = h1;` between handles of the same class copies every field.
    fn class_handle_copy(
        &mut self,
        dst: &str,
        src: &str,
        _nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let dst_class = env
            .class_handles
            .get(dst)
            .ok_or_else(|| format!("'{dst}' is not a class handle"))?
            .clone();
        let src_canonical = env.canon(src);
        let src_class = env
            .class_handles
            .get(&src_canonical)
            .or_else(|| env.class_handles.get(src))
            .ok_or_else(|| format!("'{src}' is not a class handle"))?
            .clone();
        let src_chain = self.class_chain(&src_class)?;
        if !src_chain.contains(&dst_class) {
            return Err(format!(
                "cannot assign '{src}' of class '{src_class}' into '{dst}' of class '{dst_class}'"
            ));
        }
        // 引用语义：dst 的别名组整体并入 src 的组；此后经任一成员的
        // 字段读写都解析到共享存储。
        let src_canon = env.canon(src);
        let dst_canon = env.canon(dst);
        {
            let mut aliases = env.class_aliases.borrow_mut();
            let members: Vec<String> = aliases
                .iter()
                .filter(|(_, canonical)| *canonical == &dst_canon)
                .map(|(handle, _)| handle.clone())
                .collect();
            for member in members {
                aliases.insert(member, src_canon.clone());
            }
            if dst_canon != dst {
                aliases.insert(dst.to_string(), src_canon.clone());
            } else if dst != src_canon {
                // dst 原是自身组的代表：并入后其自身条目也指向新代表。
                aliases.insert(dst.to_string(), src_canon.clone());
            }
        }
        Ok(StmtIR::Block(Vec::new()))
    }

    fn detach_class_handle_alias(&self, env: &Env<'_>, handle: &str) -> Option<(String, String)> {
        let mut aliases = env.class_aliases.borrow_mut();
        let old_canon = aliases
            .get(handle)
            .cloned()
            .unwrap_or_else(|| handle.to_string());
        let mut promoted_handle = None;
        if old_canon == handle {
            let members: Vec<String> = aliases
                .iter()
                .filter(|(h, canonical)| **canonical == old_canon && h.as_str() != handle)
                .map(|(h, _)| h.clone())
                .collect();
            if let Some(promoted) = members.first().cloned() {
                aliases.remove(&promoted);
                for member in members {
                    aliases.insert(member, promoted.clone());
                }
                promoted_handle = Some(promoted);
            }
        }
        aliases.remove(handle);
        promoted_handle.map(|new_handle| (old_canon, new_handle))
    }

    fn promote_class_handle_storage(
        &self,
        env: &Env<'_>,
        old_handle: &str,
        promoted_handle: &str,
    ) -> Vec<StmtIR> {
        let old_prefix = format!("__{old_handle}_");
        let promoted_prefix = format!("__{promoted_handle}_");
        let mut assignments = Vec::new();
        for (source_name, source) in env.nets {
            let destination_name = if source_name == old_handle {
                promoted_handle.to_string()
            } else if let Some(suffix) = source_name.strip_prefix(&old_prefix) {
                format!("{promoted_prefix}{suffix}")
            } else {
                continue;
            };
            let Some(destination) = env.nets.get(&destination_name) else {
                continue;
            };
            assignments.push(StmtIR::Bl {
                lv: LValue {
                    net: destination.base,
                    decl_width: destination.width,
                    range: full_range(destination.width),
                },
                e: ExprIR::Ref {
                    net: source.base,
                    decl_width: source.width,
                    range: full_range(source.width),
                    signed: false,
                },
            });
        }
        assignments
    }

    fn class_null_statement(
        &self,
        flag_net: &str,
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let promotion = self.detach_class_handle_alias(env, flag_net);
        let mut statements = Vec::new();
        if let Some((old_handle, promoted_handle)) = promotion {
            statements.extend(self.promote_class_handle_storage(
                env,
                &old_handle,
                &promoted_handle,
            ));
        }
        let entry = env.net(flag_net)?;
        let lvalue = LValue {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
        };
        let value = ExprIR::Const(LogicWord::zero(entry.width));
        statements.push(if nonblocking {
            StmtIR::NBl {
                lv: lvalue,
                e: value,
            }
        } else {
            StmtIR::Bl {
                lv: lvalue,
                e: value,
            }
        });
        Ok(StmtIR::Block(statements))
    }

    fn class_cast_effects(
        &mut self,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let [destination, source] = arguments else {
            return Err("$cast expects destination and source class handles".to_string());
        };
        let (destination, source) = match (destination, source) {
            (ast::Expr::Ident(destination), ast::Expr::Ident(source)) => {
                (destination.clone(), source.clone())
            }
            _ => {
                return Err(
                    "$cast in this subset requires top-level class handle arguments".to_string(),
                )
            }
        };
        let destination_class = env
            .class_handles
            .get(&destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        let source_canonical = env.canon(&source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(&source))
            .cloned()
            .ok_or_else(|| format!("'{source}' is not a class handle"))?;
        let source_chain = self.class_chain(&source_class)?;
        let destination_chain = self.class_chain(&destination_class)?;
        if source_chain.contains(&destination_class) || destination_chain.contains(&source_class) {
            return self.class_handle_copy(&destination, &source, false, env);
        }
        Ok(StmtIR::Block(Vec::new()))
    }

    fn class_cast_assignment(
        &mut self,
        target: LValue,
        arguments: &[ast::Expr],
        nonblocking: bool,
        env: &Env<'_>,
    ) -> Result<StmtIR, String> {
        let [destination, source] = arguments else {
            return Err("$cast expects destination and source class handles".to_string());
        };
        let (destination, source) = match (destination, source) {
            (ast::Expr::Ident(destination), ast::Expr::Ident(source)) => {
                (destination.clone(), source.clone())
            }
            _ => {
                return Err(
                    "$cast in this subset requires top-level class handle arguments".to_string(),
                )
            }
        };
        let destination_class = env
            .class_handles
            .get(&destination)
            .cloned()
            .ok_or_else(|| format!("'{destination}' is not a class handle"))?;
        let source_canonical = env.canon(&source);
        let source_class = env
            .class_handles
            .get(&source_canonical)
            .or_else(|| env.class_handles.get(&source))
            .cloned()
            .ok_or_else(|| format!("'{source}' is not a class handle"))?;
        let source_chain = self.class_chain(&source_class)?;
        let destination_chain = self.class_chain(&destination_class)?;
        let compatible =
            source_chain.contains(&destination_class) || destination_chain.contains(&source_class);
        let effects = if compatible {
            self.class_handle_copy(&destination, &source, false, env)?
        } else {
            StmtIR::Block(Vec::new())
        };
        let status = ExprIR::Const(LogicWord::from_u64(32, u64::from(compatible)));
        let assignment = if nonblocking {
            StmtIR::NBl {
                lv: target,
                e: status,
            }
        } else {
            StmtIR::Bl {
                lv: target,
                e: status,
            }
        };
        Ok(StmtIR::Block(vec![effects, assignment]))
    }

    fn elab_stmt(&mut self, s: &ast::Stmt, env: &Env) -> Result<StmtIR, String> {
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            if object == "$cast" && method == "cast" {
                return self.class_cast_effects(arguments, env);
            }
        }
        // Class handles can be explicitly cleared with `h = null;`. As with
        // construction, clearing detaches the handle from its alias group.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        e: ast::Expr::Ident(rhs),
                        ..
                    } if rhs == "null"
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        e: ast::Expr::Ident(rhs),
                        ..
                    } if rhs == "null"
                )
            };
            if !matches {
                continue;
            }
            let lv = match s {
                ast::Stmt::BL { lv, .. } | ast::Stmt::NBL { lv, .. } => lv,
                _ => unreachable!("guard matched a null assignment"),
            };
            let path: Vec<String> = match lv {
                ast::Expr::Ident(name) => vec![name.clone()],
                ast::Expr::Member(object, field) => vec![object.clone(), field.clone()],
                ast::Expr::MemberChain(path) => path.clone(),
                _ => return Err("null can only be assigned to a class handle".into()),
            };
            match self.resolve_class_path(env, &path)? {
                ClassPathTarget::Handle { flag_net, .. } => {
                    return self.class_null_statement(&flag_net, nonblocking, env);
                }
                ClassPathTarget::Field(_) => {
                    return Err("null can only be assigned to a class handle".to_string());
                }
            }
        }
        // Class shallow-copy construction: `dst = new source;` (without the
        // parentheses used for an ordinary constructor call).
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        lv: ast::Expr::Ident(_),
                        e: ast::Expr::New(_),
                        ..
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        lv: ast::Expr::Ident(_),
                        e: ast::Expr::New(_),
                        ..
                    }
                )
            };
            if !matches {
                continue;
            }
            let (destination, source) = match s {
                ast::Stmt::BL {
                    lv: ast::Expr::Ident(destination),
                    e: ast::Expr::New(source),
                }
                | ast::Stmt::NBL {
                    lv: ast::Expr::Ident(destination),
                    e: ast::Expr::New(source),
                } => (destination, source),
                _ => unreachable!("guard matched class shallow copy"),
            };
            if let ast::Expr::Ident(source_handle) = &**source {
                if env.class_handles.contains_key(destination)
                    && (env.class_handles.contains_key(source_handle)
                        || env.class_aliases.borrow().contains_key(source_handle))
                {
                    return self.class_shallow_copy_statement(
                        destination,
                        source_handle,
                        nonblocking,
                        env,
                    );
                }
            }
        }
        // Nested class construction: `this.d = new(...)` or `h.d = new(...)`.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        lv: ast::Expr::MemberChain(_),
                        e: ast::Expr::NewClass(_),
                    } | ast::Stmt::NBL {
                        lv: ast::Expr::Member(_, _),
                        e: ast::Expr::NewClass(_),
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        lv: ast::Expr::MemberChain(_),
                        e: ast::Expr::NewClass(_),
                    } | ast::Stmt::BL {
                        lv: ast::Expr::Member(_, _),
                        e: ast::Expr::NewClass(_),
                    }
                )
            };
            if matches {
                let (lv, actuals) = match s {
                    ast::Stmt::BL { lv, e } | ast::Stmt::NBL { lv, e } => match e {
                        ast::Expr::NewClass(actuals) => (lv, actuals),
                        _ => unreachable!("guard matched NewClass"),
                    },
                    _ => unreachable!("guard matched BL/NBL"),
                };
                let path: Vec<String> = match lv {
                    ast::Expr::MemberChain(path) => path.clone(),
                    ast::Expr::Member(object, field) => vec![object.clone(), field.clone()],
                    _ => unreachable!("guard matched member lvalue"),
                };
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle {
                        flag_net,
                        prefix,
                        class,
                    } => {
                        return self.class_new_statement(
                            &flag_net,
                            &prefix,
                            &class,
                            actuals,
                            nonblocking,
                            env,
                        );
                    }
                    ClassPathTarget::Field(_) => {
                        return Err("new can only construct a class-typed property".to_string());
                    }
                }
            }
        }
        // 嵌套类属性 = 类句柄实参：引用别名（如 this.sub = s）。
        if let ast::Stmt::BL {
            lv,
            e: ast::Expr::Ident(src),
        } = s
        {
            let src_is_handle = src == "this"
                || src == "super"
                || env.class_handles.contains_key(src)
                || env.class_aliases.borrow().contains_key(src);
            if src_is_handle {
                let path: Vec<String> = match lv {
                    ast::Expr::Member(o, f) => vec![o.clone(), f.clone()],
                    ast::Expr::MemberChain(p) => p.clone(),
                    _ => Vec::new(),
                };
                if path.len() >= 2 {
                    if let Ok(ClassPathTarget::Handle { prefix, .. }) =
                        self.resolve_class_path(env, &path)
                    {
                        let src_canon = if src == "this" || src == "super" {
                            env.this_handle
                                .map(|(handle, _)| env.canon(handle))
                                .ok_or_else(|| {
                                    format!("'{src}' is valid only inside a class method")
                                })?
                        } else {
                            env.canon(src)
                        };
                        let dst_canon = env.canon(&prefix);
                        let mut aliases = env.class_aliases.borrow_mut();
                        let members: Vec<String> = aliases
                            .iter()
                            .filter(|(_, c)| **c == dst_canon)
                            .map(|(h, _)| h.clone())
                            .collect();
                        for member in members {
                            aliases.insert(member, src_canon.clone());
                        }
                        aliases.insert(prefix, src_canon);
                        return Ok(StmtIR::Block(Vec::new()));
                    }
                }
            }
        }
        // Chained method call statement: `this.d.m(...)` / `h.d.m(...)`.
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            let path: Vec<String> = object.split('.').map(str::to_string).collect();
            if path.len() >= 2
                && (path[0] == "this"
                    || path[0] == "super"
                    || env.class_handles.contains_key(&path[0])
                    || self.classes.contains_key(&path[0]))
            {
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { prefix, class, .. } => {
                        return self.class_method_statement_on(
                            prefix.as_str(),
                            class.as_str(),
                            class.as_str(),
                            false,
                            method,
                            arguments,
                            env,
                        );
                    }
                    ClassPathTarget::Field(net) => {
                        let entry = env.net(&net)?.clone();
                        if method == "putc" && entry.type_name.as_deref() == Some("string") {
                            return self.string_putc_statement(&entry, arguments, env);
                        }
                        if entry.type_name.as_deref() == Some("string")
                            && matches!(method.as_str(), "itoa" | "hextoa" | "bintoa")
                        {
                            return if method == "itoa" {
                                self.string_itoa_statement(&entry, arguments, env)
                            } else {
                                self.string_power_radix_statement(&entry, method, arguments, env)
                            };
                        }
                        return Err(format!(
                            "method '{method}' called on a non-object field path"
                        ));
                    }
                }
            }
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::NewClass(actuals),
        } = s
        {
            let class = env
                .class_handles
                .get(name)
                .cloned()
                .ok_or_else(|| format!("'{name}' is not a class handle"))?;
            return self.class_new_statement(name, name, &class, actuals, false, env);
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::NewClass(actuals),
        } = s
        {
            let class = env
                .class_handles
                .get(name)
                .cloned()
                .ok_or_else(|| format!("'{name}' is not a class handle"))?;
            return self.class_new_statement(name, name, &class, actuals, true, env);
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(dst),
            e: ast::Expr::Ident(src),
        } = s
        {
            if env.class_handles.contains_key(dst) && env.class_handles.contains_key(src) {
                return self.class_handle_copy(dst, src, false, env);
            }
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(dst),
            e: ast::Expr::Ident(src),
        } = s
        {
            if env.class_handles.contains_key(dst) && env.class_handles.contains_key(src) {
                return self.class_handle_copy(dst, src, true, env);
            }
        }
        // Direct assignment from a class method is the one expression-call
        // context that can safely lower method side effects before reading a
        // shared result scratch net.
        for nonblocking in [false, true] {
            let matches = if nonblocking {
                matches!(
                    s,
                    ast::Stmt::NBL {
                        e: ast::Expr::MethodCall { .. },
                        ..
                    } | ast::Stmt::NBL {
                        e: ast::Expr::MethodCallChain { .. },
                        ..
                    }
                )
            } else {
                matches!(
                    s,
                    ast::Stmt::BL {
                        e: ast::Expr::MethodCall { .. },
                        ..
                    } | ast::Stmt::BL {
                        e: ast::Expr::MethodCallChain { .. },
                        ..
                    }
                )
            };
            let matches = matches || is_cast_assignment(s);
            if !matches {
                continue;
            }
            let (lv, call) = match s {
                ast::Stmt::BL { lv, e } | ast::Stmt::NBL { lv, e } => (lv, e),
                _ => unreachable!("guard matched a class method assignment"),
            };
            if let ast::Expr::Call(name, arguments) = call {
                if name == "$cast" {
                    let target = self.elab_lvalue(lv, env)?;
                    return self.class_cast_assignment(target, arguments, nonblocking, env);
                }
            }
            if let ast::Expr::Ident(name) = lv {
                if let Some((method, base, width)) = env.method_return {
                    if method == name {
                        let value = self.elab_expr(call, env)?;
                        let value = self.resize_class_arg(value, width);
                        return Ok(StmtIR::Bl {
                            lv: LValue {
                                net: base,
                                decl_width: width,
                                range: full_range(width),
                            },
                            e: value,
                        });
                    }
                }
            }
            match call {
                ast::Expr::MethodCall {
                    object,
                    method,
                    arguments,
                } => {
                    if matches!(method.as_str(), "atoi" | "atohex" | "atobin") && !nonblocking {
                        if let Ok(entry) = env.net(object) {
                            if entry.type_name.as_deref() == Some("string") {
                                let target = self.elab_lvalue(lv, env)?;
                                let entry = entry.clone();
                                return self.string_radix_statement(
                                    &entry, target, method, arguments, env,
                                );
                            }
                        }
                    }
                    if self.classes.contains_key(object) {
                        return self.class_static_method_value_assignment(
                            self.elab_lvalue(lv, env)?,
                            object,
                            method,
                            arguments,
                            nonblocking,
                            env,
                        );
                    }
                    if object == "this"
                        || object == "super"
                        || env.class_handles.contains_key(object)
                    {
                        let (handle, class, owner_hint) = if object == "this" || object == "super" {
                            let (handle, class) = env.this_handle.ok_or_else(|| {
                                format!("'{object}' is valid only inside a class method")
                            })?;
                            let start = if object == "super" {
                                env.method_owner
                                    .and_then(|owner| self.classes.get(owner))
                                    .and_then(|definition| definition.extends.clone())
                                    .ok_or_else(|| "super call has no base class".to_string())?
                            } else {
                                class.to_string()
                            };
                            (handle.to_string(), class.to_string(), start)
                        } else {
                            let class = env
                                .class_handles
                                .get(object)
                                .cloned()
                                .ok_or_else(|| format!("'{object}' is not a class handle"))?;
                            (object.clone(), class.clone(), class)
                        };
                        return self.class_method_value_assignment(
                            lv,
                            handle.as_str(),
                            class.as_str(),
                            owner_hint.as_str(),
                            object == "super",
                            method,
                            arguments,
                            nonblocking,
                            env,
                        );
                    }
                }
                ast::Expr::MethodCallChain {
                    path,
                    method,
                    arguments,
                } => {
                    if path[0] == "this"
                        || path[0] == "super"
                        || env.class_handles.contains_key(&path[0])
                    {
                        match self.resolve_class_path(env, path)? {
                            ClassPathTarget::Handle { prefix, class, .. } => {
                                return self.class_method_value_assignment(
                                    lv,
                                    prefix.as_str(),
                                    class.as_str(),
                                    class.as_str(),
                                    false,
                                    method,
                                    arguments,
                                    nonblocking,
                                    env,
                                );
                            }
                            ClassPathTarget::Field(net) => {
                                if matches!(method.as_str(), "atoi" | "atohex" | "atobin")
                                    && !nonblocking
                                {
                                    if let Ok(entry) = env.net(&net) {
                                        if entry.type_name.as_deref() == Some("string") {
                                            let target = self.elab_lvalue(lv, env)?;
                                            let entry = entry.clone();
                                            return self.string_radix_statement(
                                                &entry, target, method, arguments, env,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => unreachable!("guard matched a class method assignment"),
            }
        }
        // Return-value assignment inside a statement-invoked method.
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e,
        } = s
        {
            if let Some((method, base, width)) = env.method_return {
                if method == name {
                    let value = self.elab_expr(e, env)?;
                    let value = self.resize_class_arg(value, width);
                    return Ok(StmtIR::Bl {
                        lv: LValue {
                            net: base,
                            decl_width: width,
                            range: full_range(width),
                        },
                        e: value,
                    });
                }
            }
        }
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            if object == "this" || object == "super" || env.class_handles.contains_key(object) {
                return self.class_method_statement(object, method, arguments, env);
            }
            if self.classes.contains_key(object) {
                return self.class_static_method_statement(object, method, arguments, env);
            }
            if method == "putc" {
                if let Ok(entry) = env.net(object) {
                    if entry.type_name.as_deref() == Some("string") {
                        let entry = entry.clone();
                        return self.string_putc_statement(&entry, arguments, env);
                    }
                }
            }
            if matches!(method.as_str(), "itoa" | "hextoa" | "bintoa") {
                if let Ok(entry) = env.net(object) {
                    if entry.type_name.as_deref() == Some("string") {
                        let entry = entry.clone();
                        return if method == "itoa" {
                            self.string_itoa_statement(&entry, arguments, env)
                        } else {
                            self.string_power_radix_statement(&entry, method, arguments, env)
                        };
                    }
                }
            }
        }
        if let ast::Stmt::BL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::New(size),
        } = s
        {
            return self.dynamic_array_new(name, size, false, env);
        }
        if let ast::Stmt::NBL {
            lv: ast::Expr::Ident(name),
            e: ast::Expr::New(size),
        } = s
        {
            return self.dynamic_array_new(name, size, true, env);
        }
        if let ast::Stmt::MethodCall {
            object,
            method,
            arguments,
        } = s
        {
            return self.dynamic_array_method(object, method, arguments, env);
        }
        if let ast::Stmt::BL {
            lv:
                ast::Expr::ArrayIndexedPart {
                    name,
                    indexes,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            return self.array_element_indexed_statement(
                name, indexes, start, width, *ascending, e, false, env,
            );
        }
        if let ast::Stmt::NBL {
            lv:
                ast::Expr::ArrayIndexedPart {
                    name,
                    indexes,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            return self.array_element_indexed_statement(
                name, indexes, start, width, *ascending, e, true, env,
            );
        }
        if let ast::Stmt::BL {
            lv:
                ast::Expr::IndexedPart {
                    name,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            if let Ok(entry) = env.net(name) {
                if entry.array.is_none() && entry.msb < entry.lsb {
                    return self
                        .dynamic_indexed_part_bl_reversed(entry, start, width, *ascending, e, env);
                }
            }
            if self.const_eval(start, env.params, env.module_name).is_err() {
                let expanded = self.expand_runtime_indexed_part_assignment(
                    name, start, width, *ascending, e, env,
                )?;
                return self.elab_stmt(&expanded, env);
            }
        }
        if let ast::Stmt::NBL {
            lv:
                ast::Expr::IndexedPart {
                    name,
                    start,
                    width,
                    ascending,
                },
            e,
        } = s
        {
            if let Ok(entry) = env.net(name) {
                if entry.array.is_none() && entry.msb < entry.lsb {
                    return self.dynamic_indexed_part_nbl(name, start, width, *ascending, e, env);
                }
            }
            if self.const_eval(start, env.params, env.module_name).is_err() {
                return self.dynamic_indexed_part_nbl(name, start, width, *ascending, e, env);
            }
        }
        Ok(match s {
            ast::Stmt::Block(v) => StmtIR::Block(
                v.iter()
                    .map(|x| self.elab_stmt(x, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Stmt::Local { .. } => {
                return Err(
                    "local variable declarations are supported only inside functions".into(),
                )
            }
            ast::Stmt::Assert { cond, message } => StmtIR::Assert {
                cond: self.elab_expr(cond, env)?,
                message: message.clone(),
            },
            ast::Stmt::MethodCall { .. } => {
                return Err("dynamic-array methods are supported only in module processes".into());
            }
            ast::Stmt::For { .. } => {
                return Err("for loops are supported only inside functions".to_string());
            }
            ast::Stmt::Repeat { .. } => {
                return Err("repeat loops are supported only inside functions".to_string());
            }
            ast::Stmt::While { .. } => {
                return Err("while loops are supported only inside functions".to_string());
            }
            ast::Stmt::Forever(b) => StmtIR::Forever(Box::new(self.elab_stmt(b, env)?)),
            ast::Stmt::NBL { lv, e } => StmtIR::NBl {
                lv: self.elab_lvalue(lv, env)?,
                e: self.elab_expr(e, env)?,
            },
            ast::Stmt::BL { lv, e } => StmtIR::Bl {
                lv: self.elab_lvalue(lv, env)?,
                e: self.elab_expr(e, env)?,
            },
            ast::Stmt::If { c, t, f } => StmtIR::If {
                c: self.elab_expr(c, env)?,
                t: Box::new(self.elab_stmt(t, env)?),
                f: match f {
                    Some(x) => Some(Box::new(self.elab_stmt(x, env)?)),
                    None => None,
                },
            },
            ast::Stmt::Case {
                e,
                arms,
                dflt,
                kind,
            } => {
                if *kind != ast::CaseKind::Exact {
                    let mut flattened_arms = Vec::new();
                    for (labels, body) in arms {
                        let body = self.elab_stmt(body, env)?;
                        for label in labels {
                            flattened_arms.push((self.elab_expr(label, env)?, body.clone()));
                        }
                    }
                    let default = dflt
                        .as_ref()
                        .map(|stmt| self.elab_stmt(stmt, env))
                        .transpose()?;
                    let selector = self.elab_expr(e, env)?;
                    return Ok(wildcard_case_as_if(
                        selector,
                        flattened_arms,
                        default,
                        *kind,
                    ));
                }
                let mut out = Vec::new();
                for (labels, body) in arms {
                    let b = self.elab_stmt(body, env)?;
                    for l in labels {
                        out.push((self.elab_expr(l, env)?, b.clone()));
                    }
                }
                StmtIR::Case {
                    e: self.elab_expr(e, env)?,
                    arms: out,
                    dflt: match dflt {
                        Some(x) => Some(Box::new(self.elab_stmt(x, env)?)),
                        None => None,
                    },
                }
            }
            ast::Stmt::Delay { ns, body } => StmtIR::Delay {
                ns: *ns,
                body: Box::new(self.elab_stmt(body, env)?),
            },
            ast::Stmt::Display { fmt, args, monitor } => {
                let mut parts = Vec::new();
                let mut args_ir = Vec::new();
                for part in fmt {
                    match part {
                        ast::DispPartSrc::Str(s) => parts.push(DispPartIR::Str(s.clone())),
                        ast::DispPartSrc::Time => parts.push(DispPartIR::Time),
                        ast::DispPartSrc::Arg(spec) => {
                            let argument = self.elab_expr(&args[args_ir.len()], env)?;
                            args_ir.push(argument);
                            parts.push(DispPartIR::Arg(args_ir.len() - 1, *spec));
                        }
                    }
                }
                StmtIR::Display {
                    parts,
                    args: args_ir,
                    monitor: *monitor,
                }
            }
            ast::Stmt::Finish => StmtIR::Finish,
        })
    }

    fn elab_lvalue(&self, e: &ast::Expr, env: &Env) -> Result<LValue, String> {
        match e {
            ast::Expr::Ident(n) => {
                let entry = env.net(n)?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                })
            }
            ast::Expr::Select(n, idx) => {
                let entry = env.net(n)?;
                let i = self.const_eval(idx, env.params, env.module_name)?;
                let off = if entry.array.is_some() {
                    entry.array_offset(&[i])?
                } else {
                    entry.offset(i)?
                };
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: if entry.array.is_some() {
                        Range::Span {
                            msb: off
                                + entry
                                    .array
                                    .as_ref()
                                    .map(|array| array.element_width)
                                    .unwrap_or(1)
                                - 1,
                            lsb: off,
                        }
                    } else {
                        Range::Bit(off)
                    },
                })
            }
            ast::Expr::ArraySelect(n, indexes) => {
                let entry = env.net(n)?;
                let mut resolved = Vec::with_capacity(indexes.len());
                for index in indexes {
                    resolved.push(self.const_eval(index, env.params, env.module_name)?);
                }
                let off = entry.array_offset(&resolved)?;
                let element_width = entry
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: off + element_width - 1,
                        lsb: off,
                    },
                })
            }
            ast::Expr::Member(object, field) => {
                let (net, decl_width, range) = self.member_reference(env, object, field, true)?;
                Ok(LValue {
                    net,
                    decl_width,
                    range,
                })
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    Ok(LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: full_range(entry.width),
                    })
                }
                ClassPathTarget::Handle { .. } => {
                    Err("class handle paths cannot be assigned directly; construct with new".into())
                }
            },
            ast::Expr::Part(n, a, b) => {
                let entry = env.net(n)?;
                let ia = self.const_eval(a, env.params, env.module_name)?;
                let ib = self.const_eval(b, env.params, env.module_name)?;
                if entry.array.is_some() {
                    let (lsb, width) = entry.array_slice(ia, ib)?;
                    return Ok(LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Span {
                            msb: lsb + width - 1,
                            lsb,
                        },
                    });
                }
                let (hi, lo) = if ia >= ib { (ia, ib) } else { (ib, ia) };
                let o1 = entry.offset(hi)?;
                let o2 = entry.offset(lo)?;
                Ok(LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: o1.max(o2),
                        lsb: o1.min(o2),
                    },
                })
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                let (msb, lsb) = self.indexed_part_range(name, start, width, *ascending, env)?;
                Ok(LValue {
                    net: env.net(name)?.base,
                    decl_width: env.net(name)?.width,
                    range: Range::Span { msb, lsb },
                })
            }
            other => Err(format!("invalid assignment target: {:?}", other)),
        }
    }

    fn const_eval(
        &self,
        expression: &ast::Expr,
        params: &HashMap<String, i64>,
        module_name: &str,
    ) -> Result<i64, String> {
        const_eval_packages(expression, params, module_name, self.packages)
    }

    fn ast_range_width(&self, range: &ast::RangeAst, env: &Env<'_>) -> Result<usize, String> {
        let width = match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                (hi - lo).unsigned_abs() as usize + 1
            }
            None => 1,
        };
        if width == 0 || width > 4096 {
            Err(format!("invalid declaration width {}", width))
        } else {
            Ok(width)
        }
    }

    fn ast_range_bounds(&self, range: &ast::RangeAst, env: &Env<'_>) -> Result<(i64, i64), String> {
        match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                Ok((hi, lo))
            }
            None => Ok((0, 0)),
        }
    }

    fn expand_module_instance_arrays(
        &self,
        items: &[ast::Item],
        env: &Env<'_>,
    ) -> Result<Vec<ast::Item>, String> {
        let mut expanded = Vec::new();
        for item in items {
            let ast::Item::Inst {
                module: sub_name,
                name: inst_name,
                array_ranges,
                params: overrides,
                conns,
            } = item
            else {
                expanded.push(item.clone());
                continue;
            };
            if array_ranges.is_empty() {
                expanded.push(item.clone());
                continue;
            }
            let Some(sub) = self.mods.get(sub_name.as_str()) else {
                expanded.push(item.clone());
                continue;
            };
            if sub.is_interface {
                expanded.push(item.clone());
                continue;
            }
            let mut dimensions = Vec::with_capacity(array_ranges.len());
            let mut length = 1_usize;
            for range in array_ranges {
                let (left, right) = range;
                let base = self.const_eval(left, env.params, env.module_name)?;
                let last = self.const_eval(right, env.params, env.module_name)?;
                let step = if base <= last { 1 } else { -1 };
                let dimension_length = (base - last).unsigned_abs() as usize + 1;
                if dimension_length == 0 || dimension_length > 256 {
                    return Err(format!(
                        "instance '{}': each module array length must be between 1 and 256",
                        inst_name
                    ));
                }
                length = length
                    .checked_mul(dimension_length)
                    .filter(|total| *total <= 4096)
                    .ok_or_else(|| {
                        format!(
                            "instance '{}': total module array elements exceed 4096",
                            inst_name
                        )
                    })?;
                dimensions.push(InterfaceArrayDimension {
                    base,
                    step,
                    length: dimension_length,
                });
            }
            let mut index_sets = vec![Vec::<i64>::new()];
            for dimension in &dimensions {
                let mut next_index_sets = Vec::new();
                for index_set in &index_sets {
                    for element in 0..dimension.length {
                        let mut next = index_set.clone();
                        next.push(dimension.base + dimension.step * element as i64);
                        next_index_sets.push(next);
                    }
                }
                index_sets = next_index_sets;
            }

            let mut child_params = HashMap::new();
            let mut declared_parameters = Vec::new();
            for child_item in &sub.items {
                if let ast::Item::Params(list) = child_item {
                    for (parameter, value) in list {
                        let value = self.const_eval(value, &child_params, &sub.name)?;
                        child_params.insert(parameter.clone(), value);
                        declared_parameters.push(parameter.clone());
                    }
                }
            }
            for (index, override_value) in overrides.iter().enumerate() {
                let value = self.const_eval(&override_value.1, env.params, env.module_name)?;
                match override_value.0.as_deref() {
                    Some(parameter) => {
                        child_params.insert(parameter.to_string(), value);
                    }
                    None => {
                        if let Some(parameter) = declared_parameters.get(index) {
                            child_params.insert(parameter.clone(), value);
                        }
                    }
                }
            }

            let mut ports = Vec::new();
            for child_item in &sub.items {
                if let ast::Item::Port {
                    dir,
                    interface_name,
                    type_name,
                    range,
                    array_ranges,
                    names,
                    ..
                } = child_item
                {
                    if interface_name.is_some() {
                        return Err(format!(
                            "instance '{}': module arrays with interface ports are not yet supported",
                            inst_name
                        ));
                    }
                    if type_name.is_some() {
                        return Err(format!(
                            "instance '{}': module arrays with custom-typed ports are not yet supported",
                            inst_name
                        ));
                    }
                    let width = match range {
                        Some((msb, lsb)) => {
                            let hi = self.const_eval(msb, &child_params, &sub.name)?;
                            let lo = self.const_eval(lsb, &child_params, &sub.name)?;
                            (hi - lo).unsigned_abs() as usize + 1
                        }
                        None => 1,
                    };
                    if width == 0 {
                        return Err(format!(
                            "instance '{}': module array child port width cannot be zero",
                            inst_name
                        ));
                    }
                    let array = self.resolve_unpacked_array(
                        array_ranges,
                        width,
                        &child_params,
                        &sub.name,
                        &names[0],
                        "port",
                    )?;
                    let total_width = width * array.as_ref().map_or(1, |resolved| resolved.length);
                    for port_name in names {
                        ports.push((port_name.clone(), *dir, total_width));
                    }
                }
            }

            for (element, index_set) in index_sets.into_iter().enumerate() {
                let index = index_set
                    .iter()
                    .map(|index| index.to_string())
                    .collect::<Vec<_>>()
                    .join("_");
                let bit_offset = length - 1 - element;
                let mut element_conns = Vec::new();
                let mut positional_port = 0_usize;
                for connection in conns {
                    let (port_name, direction, port_width) = match connection {
                        ast::Conn::Named(name, _) => ports
                            .iter()
                            .find(|(candidate, _, _)| candidate == name)
                            .map(|(name, direction, width)| (name.clone(), *direction, *width))
                            .ok_or_else(|| {
                                format!(
                                    "instance '{}': unknown module array port '{}'",
                                    inst_name, name
                                )
                            })?,
                        ast::Conn::Pos(_) => {
                            let port = ports
                                .get(positional_port)
                                .ok_or_else(|| {
                                    format!(
                                        "instance '{}': too many positional module array connections",
                                        inst_name
                                    )
                                })?
                                .clone();
                            positional_port += 1;
                            port
                        }
                    };
                    let connection = match connection {
                        ast::Conn::Named(name, expression) => {
                            ast::Conn::Named(name.clone(), expression.clone())
                        }
                        ast::Conn::Pos(expression) => ast::Conn::Pos(expression.clone()),
                    };
                    let expression = match &connection {
                        ast::Conn::Named(_, Some(expression))
                        | ast::Conn::Pos(Some(expression)) => expression,
                        _ => {
                            element_conns.push(connection);
                            continue;
                        }
                    };
                    let expression_width = self.elab_expr(expression, env)?.width();
                    let slice_offset = bit_offset * port_width;
                    let msb = slice_offset + port_width - 1;
                    let lsb = slice_offset;
                    let sliced = match direction {
                        ast::DirSrc::In => {
                            if expression_width == port_width {
                                expression.clone()
                            } else if expression_width >= length * port_width {
                                ast::Expr::Slice {
                                    value: Box::new(expression.clone()),
                                    msb: Box::new(loop_value(msb as i64)),
                                    lsb: Box::new(loop_value(lsb as i64)),
                                }
                            } else {
                                return Err(format!(
                                    "instance '{}': input port '{}' width {} cannot feed {} array elements of width {}",
                                    inst_name,
                                    port_name,
                                    expression_width,
                                    length,
                                    port_width
                                ));
                            }
                        }
                        ast::DirSrc::Out => {
                            let ast::Expr::Ident(target) = expression else {
                                return Err(format!(
                                    "instance '{}': module array output port '{}' must connect an identifier",
                                    inst_name, port_name
                                ));
                            };
                            if expression_width < length * port_width {
                                return Err(format!(
                                    "instance '{}': output port '{}' width {} cannot receive {} array elements of width {}",
                                    inst_name,
                                    port_name,
                                    expression_width,
                                    length,
                                    port_width
                                ));
                            }
                            ast::Expr::Part(
                                target.clone(),
                                Box::new(loop_value(msb as i64)),
                                Box::new(loop_value(lsb as i64)),
                            )
                        }
                        ast::DirSrc::InOut => {
                            return Err(format!(
                                "instance '{}': module arrays do not support inout port '{}'",
                                inst_name, port_name
                            ));
                        }
                    };
                    element_conns.push(match connection {
                        ast::Conn::Named(name, _) => ast::Conn::Named(name, Some(sliced)),
                        ast::Conn::Pos(_) => ast::Conn::Pos(Some(sliced)),
                    });
                }
                expanded.push(ast::Item::Inst {
                    module: sub_name.clone(),
                    name: format!("{inst_name}_{index}"),
                    array_ranges: Vec::new(),
                    params: overrides.clone(),
                    conns: element_conns,
                });
            }
        }
        Ok(expanded)
    }

    fn inline_function_call(
        &self,
        name: &str,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let inlined = self.build_function_expression(name, arguments, env, &Vec::new())?;
        self.elab_expr(&inlined, env)
    }

    fn build_function_expression(
        &self,
        name: &str,
        arguments: &[ast::Expr],
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        if call_stack.iter().any(|active| active == name) {
            return Err(format!(
                "recursive function call '{}' through {:?}",
                name, call_stack
            ));
        }
        let definition = env
            .functions
            .get(name)
            .ok_or_else(|| format!("unknown function '{}'", name))?;
        let arguments = self.expand_function_actuals(definition, arguments)?;
        let inlined_arguments = arguments
            .iter()
            .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
            .collect::<Result<Vec<_>, String>>()?;
        validate_function_body(definition)?;
        let mut argument_map = HashMap::new();
        for ((formal, range), actual) in definition.args.iter().zip(inlined_arguments) {
            let width = self.ast_range_width(range, env)?;
            let actual = ast::Expr::Resize {
                width,
                value: Box::new(actual),
            };
            argument_map.insert(formal.clone(), actual);
        }
        let mut nested_stack = call_stack.to_vec();
        nested_stack.push(name.to_string());
        self.evaluate_function_body(definition, &mut argument_map, env, &nested_stack)
    }

    fn evaluate_function_body(
        &self,
        definition: &ast::FunctionDef,
        values: &mut HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        let mut return_expression = None;
        let mut aggregate_types = FunctionAggregateState::default();
        for (argument, range) in &definition.args {
            let width = self.ast_range_width(range, env)?;
            aggregate_types
                .scalar_widths
                .insert(argument.clone(), width);
            aggregate_types
                .scalar_ranges
                .insert(argument.clone(), self.ast_range_bounds(range, env)?);
        }
        self.evaluate_function_statements(
            &definition.body,
            definition,
            values,
            env,
            call_stack,
            &mut aggregate_types,
            &mut return_expression,
        )?;
        return_expression.ok_or_else(|| {
            format!(
                "function '{}' must assign its return value exactly once",
                definition.name
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_function_statements(
        &self,
        statements: &[ast::Stmt],
        definition: &ast::FunctionDef,
        values: &mut HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
        aggregate_types: &mut FunctionAggregateState,
        return_expression: &mut Option<ast::Expr>,
    ) -> Result<(), String> {
        for statement in statements {
            match statement {
                ast::Stmt::Block(nested) => self.evaluate_function_statements(
                    nested,
                    definition,
                    values,
                    env,
                    call_stack,
                    aggregate_types,
                    return_expression,
                )?,
                ast::Stmt::Local {
                    type_name,
                    range,
                    array_ranges,
                    names,
                } => {
                    for (name, initializer) in names {
                        let initializer = if let Some(type_name) = type_name {
                            if range.is_some() {
                                return Err(format!(
                                    "function '{}': typed local '{}' does not support an additional packed range",
                                    definition.name, name
                                ));
                            }
                            let definition = env.types.get(type_name).ok_or_else(|| {
                                format!(
                                    "function local '{}' uses unknown type '{}'",
                                    name, type_name
                                )
                            })?;
                            aggregate_types
                                .types
                                .insert(name.clone(), type_name.clone());
                            aggregate_types
                                .scalar_widths
                                .insert(name.clone(), definition.width());
                            aggregate_types
                                .scalar_ranges
                                .insert(name.clone(), (definition.width() as i64 - 1, 0));
                            match initializer {
                                Some(expression) => self.expand_function_expression(
                                    expression,
                                    values,
                                    env,
                                    call_stack,
                                    aggregate_types,
                                )?,
                                None => ast::Expr::Num {
                                    width: Some(definition.width()),
                                    word: LogicWord::zero(definition.width()),
                                    orig: "0".to_string(),
                                },
                            }
                        } else {
                            aggregate_types.types.remove(name);
                            if !array_ranges.is_empty() {
                                let element_width = self.ast_range_width(range, env)?;
                                let (element_msb, element_lsb) =
                                    self.ast_range_bounds(range, env)?;
                                aggregate_types.scalar_widths.remove(name);
                                aggregate_types.scalar_ranges.remove(name);
                                let mut dimensions = Vec::with_capacity(array_ranges.len());
                                let mut length = 1_usize;
                                for array_range in array_ranges {
                                    let Some((left, right)) = array_range else {
                                        return Err(format!(
                                            "function array '{}' has an empty dimension",
                                            name
                                        ));
                                    };
                                    let base =
                                        self.const_eval(left, env.params, env.module_name)?;
                                    let last =
                                        self.const_eval(right, env.params, env.module_name)?;
                                    let step = if base <= last { 1 } else { -1 };
                                    let dimension_length =
                                        (base - last).unsigned_abs() as usize + 1;
                                    if dimension_length == 0 || dimension_length > 256 {
                                        return Err(format!(
                                            "function array '{}' has invalid dimensions",
                                            name
                                        ));
                                    }
                                    length = length
                                        .checked_mul(dimension_length)
                                        .filter(|total| *total <= 4096)
                                        .ok_or_else(|| {
                                            format!("function array '{}' is too large", name)
                                        })?;
                                    dimensions.push(FunctionArrayDimension {
                                        base,
                                        step,
                                        length: dimension_length,
                                    });
                                }
                                let total_width = element_width
                                    .checked_mul(length)
                                    .ok_or_else(|| "function array is too large".to_string())?;
                                if total_width > 4096 {
                                    return Err(format!(
                                        "function array '{}' exceeds the packed-width limit",
                                        name
                                    ));
                                }
                                aggregate_types.arrays.insert(
                                    name.clone(),
                                    FunctionArrayLocal {
                                        dimensions,
                                        element_width,
                                        element_msb,
                                        element_lsb,
                                        length,
                                    },
                                );
                                aggregate_types.array_values.insert(
                                    name.clone(),
                                    vec![
                                        ast::Expr::Num {
                                            width: Some(element_width),
                                            word: LogicWord::zero(element_width),
                                            orig: "0".to_string(),
                                        };
                                        length
                                    ],
                                );
                                continue;
                            } else {
                                aggregate_types.arrays.remove(name);
                                aggregate_types.array_values.remove(name);
                                aggregate_types
                                    .scalar_widths
                                    .insert(name.clone(), self.ast_range_width(range, env)?);
                                aggregate_types
                                    .scalar_ranges
                                    .insert(name.clone(), self.ast_range_bounds(range, env)?);
                                match initializer {
                                    Some(expression) => self.expand_function_expression(
                                        expression,
                                        values,
                                        env,
                                        call_stack,
                                        aggregate_types,
                                    )?,
                                    None => self.local_zero(range, env)?,
                                }
                            }
                        };
                        let initializer =
                            if let Some(width) = aggregate_types.scalar_widths.get(name).copied() {
                                ast::Expr::Resize {
                                    width,
                                    value: Box::new(initializer),
                                }
                            } else {
                                initializer
                            };
                        values.insert(name.clone(), initializer);
                    }
                }
                ast::Stmt::BL { lv, e } => {
                    if let ast::Expr::ArrayIndexedPart {
                        name,
                        indexes,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        if indexes.len() != array.dimensions.len() {
                            return Err(format!(
                                "array expects {} index(es), received {}",
                                array.dimensions.len(),
                                indexes.len()
                            ));
                        }
                        let mut resolved_indexes = Vec::with_capacity(indexes.len());
                        let mut runtime_indexes = Vec::with_capacity(indexes.len());
                        let mut all_indexes_constant = true;
                        for index in indexes {
                            let index = self.expand_function_expression(
                                index,
                                values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            runtime_indexes.push(index.clone());
                            if let Ok(constant) =
                                self.const_eval(&index, env.params, env.module_name)
                            {
                                resolved_indexes.push(constant);
                            } else {
                                all_indexes_constant = false;
                            }
                        }
                        let reversed = array.element_msb < array.element_lsb;
                        let logical_base = if reversed {
                            array.element_msb
                        } else {
                            array.element_lsb
                        };

                        let start = self.expand_function_expression(
                            start,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let width = self.expand_function_expression(
                            width,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let width = self.const_eval(&width, env.params, env.module_name)?;
                        if width <= 0 {
                            return Err(format!(
                                "indexed part-select width must be positive, got {width}"
                            ));
                        }
                        let width = width as usize;
                        if width > array.element_width {
                            return Err(format!(
                                "indexed part-select width {width} exceeds array element width {}",
                                array.element_width
                            ));
                        }
                        let logical_start = if *ascending {
                            start
                        } else {
                            ast::Expr::Bin(
                                ast::BinOpSrc::Sub,
                                Box::new(start),
                                Box::new(constant_expr(width as i64 - 1)),
                            )
                        };
                        let offset = ast::Expr::Bin(
                            ast::BinOpSrc::Sub,
                            Box::new(logical_start),
                            Box::new(constant_expr(logical_base)),
                        );
                        let value = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let update_element = |old_element: ast::Expr| {
                            let normalized_old = if reversed {
                                Self::reverse_ast_bits(old_element.clone(), array.element_width)
                            } else {
                                old_element.clone()
                            };
                            let normalized_updated = Self::function_array_indexed_update(
                                normalized_old,
                                array.element_width,
                                offset.clone(),
                                width,
                                value.clone(),
                            );
                            if reversed {
                                Self::reverse_ast_bits(normalized_updated, array.element_width)
                            } else {
                                normalized_updated
                            }
                        };
                        if all_indexes_constant {
                            let element_index = array.element_index(&resolved_indexes)?;
                            let old_element =
                                aggregate_types.array_values[name][element_index].clone();
                            let updated = update_element(old_element);
                            let elements = aggregate_types
                                .array_values
                                .get_mut(name)
                                .ok_or_else(|| format!("array local '{name}' state is missing"))?;
                            elements[element_index] = updated;
                            continue;
                        }

                        let old_elements = aggregate_types.array_values[name].clone();
                        let mut updated_elements = Vec::with_capacity(array.length);
                        for (physical_element, old_element) in old_elements.iter().enumerate() {
                            let mut physical_offset = physical_element;
                            let mut scale = array.length;
                            let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
                            for dimension in &array.dimensions {
                                scale /= dimension.length;
                                let dimension_offset = physical_offset / scale;
                                physical_offset %= scale;
                                logical_indexes.push(
                                    dimension.base + dimension.step * dimension_offset as i64,
                                );
                            }
                            let mut candidate_valid = ast::Expr::Num {
                                width: Some(1),
                                word: LogicWord::from_u64(1, 1),
                                orig: "function_runtime_array_write_true".to_string(),
                            };
                            for (runtime_index, logical_index) in
                                runtime_indexes.iter().zip(&logical_indexes)
                            {
                                let equals = ast::Expr::Bin(
                                    ast::BinOpSrc::Eq,
                                    Box::new(runtime_index.clone()),
                                    Box::new(constant_expr(*logical_index)),
                                );
                                candidate_valid = ast::Expr::Bin(
                                    ast::BinOpSrc::LogAnd,
                                    Box::new(candidate_valid),
                                    Box::new(equals),
                                );
                            }
                            let updated = update_element(old_element.clone());
                            updated_elements.push(ast::Expr::Cond(
                                Box::new(candidate_valid),
                                Box::new(updated),
                                Box::new(old_element.clone()),
                            ));
                        }
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{name}' state is missing"))?;
                        *elements = updated_elements;
                        continue;
                    }
                    if let ast::Expr::ArraySelect(name, indexes) = lv {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        let mut resolved_indexes = Vec::with_capacity(indexes.len());
                        for index in indexes {
                            let index = self.expand_function_expression(
                                index,
                                values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            resolved_indexes.push(self.const_eval(
                                &index,
                                env.params,
                                env.module_name,
                            )?);
                        }
                        let element_index = array.element_index(&resolved_indexes)?;
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{}' state is missing", name))?;
                        elements[element_index] = ast::Expr::Resize {
                            width: array.element_width,
                            value: Box::new(expanded),
                        };
                        continue;
                    }
                    if let ast::Expr::Select(name, index) = lv {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        let index = self.expand_function_expression(
                            index,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let index = self.const_eval(&index, env.params, env.module_name)?;
                        let element_index = array.element_index(&[index])?;
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{}' state is missing", name))?;
                        elements[element_index] = ast::Expr::Resize {
                            width: array.element_width,
                            value: Box::new(expanded),
                        };
                        continue;
                    }
                    if let ast::Expr::Member(name, field) = lv {
                        let Some(type_name) = aggregate_types.types.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not a typed local",
                                definition.name, name
                            ));
                        };
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let updated = self.update_aggregate_field(
                            values[name].clone(),
                            env,
                            &type_name,
                            field,
                            expanded,
                        )?;
                        values.insert(name.clone(), updated);
                        continue;
                    }
                    let ast::Expr::Ident(name) = lv else {
                        return Err(format!(
                            "function '{}': only scalar/function or packed aggregate field assignments are supported",
                            definition.name
                        ));
                    };
                    let mut expanded = self.expand_function_expression(
                        e,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    if name == &definition.name {
                        let width = self.ast_range_width(&definition.return_range, env)?;
                        expanded = ast::Expr::Resize {
                            width,
                            value: Box::new(expanded),
                        };
                        if return_expression.replace(expanded).is_some() {
                            return Err(format!(
                                "function '{}' assigns its return value more than once",
                                definition.name
                            ));
                        }
                    } else if values.contains_key(name) {
                        let expanded =
                            if let Some(width) = aggregate_types.scalar_widths.get(name).copied() {
                                ast::Expr::Resize {
                                    width,
                                    value: Box::new(expanded),
                                }
                            } else {
                                expanded
                            };
                        values.insert(name.clone(), expanded);
                    } else {
                        return Err(format!(
                            "function '{}': assignment to undeclared local '{}'",
                            definition.name, name
                        ));
                    }
                }
                ast::Stmt::Repeat { count, body } => {
                    let count = self.expand_function_expression(
                        count,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let count = self.const_eval(&count, env.params, env.module_name)?;
                    if !(0..=256).contains(&count) {
                        return Err(format!(
                            "function '{}': repeat count must be between 0 and 256, got {}",
                            definition.name, count
                        ));
                    }
                    for _ in 0..count {
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                    }
                }
                ast::Stmt::While { cond, body } => {
                    let mut iterations = 0_usize;
                    loop {
                        let condition = self.expand_function_expression(
                            cond,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let condition = self
                            .const_eval(&condition, env.params, env.module_name)
                            .map_err(|error| {
                                format!(
                                    "function '{}': while condition must be constant during elaboration: {error}",
                                    definition.name
                                )
                            })?;
                        if condition == 0 {
                            break;
                        }
                        if iterations == 256 {
                            return Err(format!(
                                "function '{}': while loop exceeds the 256-iteration elaboration limit",
                                definition.name
                            ));
                        }
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                        iterations += 1;
                    }
                }
                ast::Stmt::For {
                    var,
                    start,
                    cond,
                    step,
                    body,
                } => {
                    let start = self.expand_function_expression(
                        start,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let start = self.const_eval(&start, env.params, env.module_name)?;
                    if !(0..=i32::MAX as i64).contains(&start) {
                        return Err(format!(
                            "function '{}': for-loop start for '{var}' must be a non-negative integer",
                            definition.name
                        ));
                    }
                    values.insert(var.clone(), loop_value(start));
                    let mut iterations = 0_usize;
                    loop {
                        if iterations == 256 {
                            return Err(format!(
                                "function '{}': for-loop '{var}' exceeds the 256-iteration elaboration limit",
                                definition.name
                            ));
                        }
                        let condition = self.expand_function_expression(
                            cond,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let condition = self.const_eval(&condition, env.params, env.module_name)?;
                        if condition == 0 {
                            break;
                        }
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                        let step = self.expand_function_expression(
                            step,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let next = self.const_eval(&step, env.params, env.module_name)?;
                        if !(0..=i32::MAX as i64).contains(&next) {
                            return Err(format!(
                                "function '{}': for-loop '{var}' step must remain a non-negative integer",
                                definition.name
                            ));
                        }
                        let current = match &values[var] {
                            ast::Expr::Num { word, .. } => word.to_u64().unwrap_or(0) as i64,
                            _ => return Err(format!(
                                "function '{}': for-loop variable '{var}' must remain constant during elaboration",
                                definition.name
                            )),
                        };
                        if next == current {
                            return Err(format!(
                                "function '{}': for-loop '{var}' does not advance",
                                definition.name
                            ));
                        }
                        values.insert(var.clone(), loop_value(next));
                        iterations += 1;
                    }
                    values.remove(var);
                }
                ast::Stmt::If { c, t, f } => {
                    let condition = self.expand_function_expression(
                        c,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let mut then_values = values.clone();
                    let mut then_aggregate_types = aggregate_types.clone();
                    let mut then_return = return_expression.clone();
                    self.evaluate_function_statements(
                        std::slice::from_ref(t.as_ref()),
                        definition,
                        &mut then_values,
                        env,
                        call_stack,
                        &mut then_aggregate_types,
                        &mut then_return,
                    )?;
                    let mut else_values = values.clone();
                    let mut else_aggregate_types = aggregate_types.clone();
                    let mut else_return = return_expression.clone();
                    if let Some(false_branch) = f {
                        self.evaluate_function_statements(
                            std::slice::from_ref(false_branch.as_ref()),
                            definition,
                            &mut else_values,
                            env,
                            call_stack,
                            &mut else_aggregate_types,
                            &mut else_return,
                        )?;
                    }
                    if then_return.is_some() != else_return.is_some() {
                        return Err(format!(
                            "function '{}': if branches must both assign or both preserve the return value",
                            definition.name
                        ));
                    }
                    for key in then_values.keys().cloned().collect::<Vec<_>>() {
                        let then_value = then_values.remove(&key).expect("known then value");
                        let else_value = else_values.remove(&key).expect("known else value");
                        let merged = if then_value == else_value {
                            then_value
                        } else {
                            ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(then_value),
                                Box::new(else_value),
                            )
                        };
                        values.insert(key, merged);
                    }
                    Self::merge_array_states(
                        aggregate_types,
                        then_aggregate_types.array_values,
                        else_aggregate_types.array_values,
                        &condition,
                    );
                    *return_expression = match (then_return, else_return) {
                        (Some(then_value), Some(else_value)) => Some(if then_value == else_value {
                            then_value
                        } else {
                            ast::Expr::Cond(
                                Box::new(condition),
                                Box::new(then_value),
                                Box::new(else_value),
                            )
                        }),
                        (None, None) => None,
                        _ => unreachable!("branch return states were checked"),
                    };
                }
                ast::Stmt::Case {
                    e,
                    arms,
                    dflt,
                    kind,
                } => {
                    let selector = self.expand_function_expression(
                        e,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let entry_values = values.clone();
                    let entry_return = return_expression.clone();
                    let mut branches = Vec::new();
                    for (labels, body) in arms {
                        let mut condition = None;
                        for label in labels {
                            let label = self.expand_function_expression(
                                label,
                                &entry_values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            let comparison = if *kind == ast::CaseKind::Exact {
                                ast::Expr::Bin(
                                    ast::BinOpSrc::Eq,
                                    Box::new(selector.clone()),
                                    Box::new(label),
                                )
                            } else {
                                ast::Expr::WildcardMatch {
                                    kind: *kind,
                                    selector: Box::new(selector.clone()),
                                    label: Box::new(label),
                                }
                            };
                            condition = Some(match condition {
                                Some(existing) => ast::Expr::Bin(
                                    ast::BinOpSrc::LogOr,
                                    Box::new(existing),
                                    Box::new(comparison),
                                ),
                                None => comparison,
                            });
                        }
                        let Some(condition) = condition else {
                            return Err(format!(
                                "function '{}': case arm has no labels",
                                definition.name
                            ));
                        };
                        let mut branch_values = entry_values.clone();
                        let mut branch_aggregate_types = aggregate_types.clone();
                        let mut branch_return = entry_return.clone();
                        self.evaluate_function_statements(
                            std::slice::from_ref(body),
                            definition,
                            &mut branch_values,
                            env,
                            call_stack,
                            &mut branch_aggregate_types,
                            &mut branch_return,
                        )?;
                        branches.push((
                            condition,
                            branch_values,
                            branch_return,
                            branch_aggregate_types.array_values,
                        ));
                    }
                    let mut unmatched_values = entry_values.clone();
                    let mut unmatched_aggregate_types = aggregate_types.clone();
                    let mut unmatched_return = entry_return.clone();
                    if let Some(default_body) = dflt {
                        self.evaluate_function_statements(
                            std::slice::from_ref(default_body),
                            definition,
                            &mut unmatched_values,
                            env,
                            call_stack,
                            &mut unmatched_aggregate_types,
                            &mut unmatched_return,
                        )?;
                    }
                    for key in unmatched_values.keys().cloned().collect::<Vec<_>>() {
                        let mut merged = unmatched_values
                            .remove(&key)
                            .expect("known unmatched case value");
                        for (condition, branch_values, _, _) in branches.iter().rev() {
                            let branch_value = branch_values
                                .get(&key)
                                .cloned()
                                .expect("known case branch value");
                            merged = ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(branch_value),
                                Box::new(merged),
                            );
                        }
                        values.insert(key, merged);
                    }
                    let mut return_states = Vec::with_capacity(branches.len() + 1);
                    return_states.push(unmatched_return.as_ref());
                    for (_, _, branch_return, _) in &branches {
                        return_states.push(branch_return.as_ref());
                    }
                    if return_states.iter().any(Option::is_some)
                        && return_states.iter().any(Option::is_none)
                    {
                        return Err(format!(
                            "function '{}': every case path must assign or preserve the return value",
                            definition.name
                        ));
                    }
                    let merged_return = if return_states.iter().all(Option::is_some) {
                        let mut merged = unmatched_return.clone().expect("checked case return");
                        for (condition, _, branch_return, _) in branches.iter().rev() {
                            let branch_value = branch_return.clone().expect("checked return");
                            merged = ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(branch_value),
                                Box::new(merged),
                            );
                        }
                        Some(merged)
                    } else {
                        None
                    };
                    aggregate_types.array_values = unmatched_aggregate_types.array_values;
                    for (condition, _, _, branch_arrays) in branches.into_iter().rev() {
                        let unmatched_arrays = aggregate_types.array_values.clone();
                        Self::merge_array_states(
                            aggregate_types,
                            branch_arrays,
                            unmatched_arrays,
                            &condition,
                        );
                    }
                    *return_expression = merged_return;
                }
                other => {
                    return Err(format!(
                        "function '{}': unsupported statement {:?} in straight-line function body",
                        definition.name, other
                    ));
                }
            }
        }
        Ok(())
    }

    fn expand_function_expression(
        &self,
        expression: &ast::Expr,
        values: &HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
        aggregate_types: &FunctionAggregateState,
    ) -> Result<ast::Expr, String> {
        let substituted = substitute_function_arguments(expression, values);
        let substituted =
            self.resolve_aggregate_references(&substituted, values, aggregate_types, env)?;
        self.inline_nested_function_calls(&substituted, env, call_stack)
    }

    fn resolve_aggregate_references(
        &self,
        expression: &ast::Expr,
        values: &HashMap<String, ast::Expr>,
        aggregate_types: &FunctionAggregateState,
        env: &Env<'_>,
    ) -> Result<ast::Expr, String> {
        Ok(match expression {
            ast::Expr::Member(object, field)
                if values.contains_key(object) && aggregate_types.types.contains_key(object) =>
            {
                let type_name = &aggregate_types.types[object];
                let definition = env.types.get(type_name).ok_or_else(|| {
                    format!(
                        "function local '{}' uses unknown type '{}'",
                        object, type_name
                    )
                })?;
                let definition = match definition {
                    ResolvedType::Struct(definition) | ResolvedType::Union(definition) => {
                        definition
                    }
                    ResolvedType::Enum(_) => {
                        return Err(format!(
                            "function local '{}' has enum type and no field '{}'",
                            object, field
                        ));
                    }
                };
                let &(offset, width) = definition
                    .fields
                    .get(field)
                    .ok_or_else(|| format!("type '{}' has no field '{}'", type_name, field))?;
                ast::Expr::Slice {
                    value: Box::new(values[object].clone()),
                    msb: Box::new(constant_expr((offset + width - 1) as i64)),
                    lsb: Box::new(constant_expr(offset as i64)),
                }
            }
            ast::Expr::Ident(name) if aggregate_types.arrays.contains_key(name) => {
                ast::Expr::Concat(aggregate_types.array_values[name].clone())
            }
            ast::Expr::Ident(_)
            | ast::Expr::ScopedIdent { .. }
            | ast::Expr::Num { .. }
            | ast::Expr::Str(_)
            | ast::Expr::Member(_, _)
            | ast::Expr::MemberChain(_) => expression.clone(),
            ast::Expr::Select(name, index) if aggregate_types.arrays.contains_key(name) => {
                let array = &aggregate_types.arrays[name];
                let index =
                    self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                let index = self.const_eval(&index, env.params, env.module_name)?;
                let element_index = array.element_index(&[index])?;
                aggregate_types.array_values[name][element_index].clone()
            }
            ast::Expr::ArraySelect(name, indexes) if aggregate_types.arrays.contains_key(name) => {
                let mut resolved_indexes = Vec::with_capacity(indexes.len());
                for index in indexes {
                    let index =
                        self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                    resolved_indexes.push(self.const_eval(&index, env.params, env.module_name)?);
                }
                let array = &aggregate_types.arrays[name];
                let element_index = array.element_index(&resolved_indexes)?;
                aggregate_types.array_values[name][element_index].clone()
            }
            ast::Expr::Select(name, index) => ast::Expr::Select(
                name.clone(),
                Box::new(self.resolve_aggregate_references(index, values, aggregate_types, env)?),
            ),
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => ast::Expr::MethodCall {
                object: object.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => ast::Expr::MethodCallChain {
                path: path.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::New(size) => ast::Expr::New(Box::new(self.resolve_aggregate_references(
                size,
                values,
                aggregate_types,
                env,
            )?)),
            ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
                arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
                name.clone(),
                indexes
                    .iter()
                    .map(|index| {
                        self.resolve_aggregate_references(index, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => {
                let Some(array) = aggregate_types.arrays.get(name) else {
                    return Err(format!(
                        "indexed part-select uses unknown function array '{}'",
                        name
                    ));
                };
                let mut resolved_indexes = Vec::with_capacity(indexes.len());
                let mut runtime_indexes = Vec::with_capacity(indexes.len());
                let mut all_indexes_constant = true;
                for index in indexes {
                    let index =
                        self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                    runtime_indexes.push(index.clone());
                    if let Ok(constant) = self.const_eval(&index, env.params, env.module_name) {
                        resolved_indexes.push(constant);
                    } else {
                        all_indexes_constant = false;
                    }
                }
                let mut array_index_valid = None;
                let element = if all_indexes_constant {
                    let element_index = array.element_index(&resolved_indexes)?;
                    aggregate_types.array_values[name][element_index].clone()
                } else {
                    let mut selected_element = ast::Expr::Num {
                        width: Some(array.element_width),
                        word: LogicWord::xes(array.element_width),
                        orig: "function_runtime_array_x".to_string(),
                    };
                    let mut any_index_valid = ast::Expr::Num {
                        width: Some(1),
                        word: LogicWord::from_u64(1, 0),
                        orig: "function_runtime_array_false".to_string(),
                    };
                    for physical_element in 0..array.length {
                        let mut physical_offset = physical_element;
                        let mut scale = array.length;
                        let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
                        for dimension in &array.dimensions {
                            scale /= dimension.length;
                            let dimension_offset = physical_offset / scale;
                            physical_offset %= scale;
                            logical_indexes
                                .push(dimension.base + dimension.step * dimension_offset as i64);
                        }

                        let mut candidate_valid = ast::Expr::Num {
                            width: Some(1),
                            word: LogicWord::from_u64(1, 1),
                            orig: "function_runtime_array_true".to_string(),
                        };
                        for (runtime_index, logical_index) in
                            runtime_indexes.iter().zip(&logical_indexes)
                        {
                            let equals = ast::Expr::Bin(
                                ast::BinOpSrc::Eq,
                                Box::new(runtime_index.clone()),
                                Box::new(constant_expr(*logical_index)),
                            );
                            candidate_valid = ast::Expr::Bin(
                                ast::BinOpSrc::LogAnd,
                                Box::new(candidate_valid),
                                Box::new(equals),
                            );
                        }
                        any_index_valid = ast::Expr::Bin(
                            ast::BinOpSrc::LogOr,
                            Box::new(any_index_valid),
                            Box::new(candidate_valid.clone()),
                        );
                        selected_element = ast::Expr::Cond(
                            Box::new(candidate_valid),
                            Box::new(aggregate_types.array_values[name][physical_element].clone()),
                            Box::new(selected_element),
                        );
                    }
                    array_index_valid = Some(any_index_valid);
                    selected_element
                };
                let element_width = array.element_width;
                let reversed = array.element_msb < array.element_lsb;
                let logical_base = if reversed {
                    array.element_msb
                } else {
                    array.element_lsb
                };
                let normalized_element = if reversed {
                    Self::reverse_ast_bits(element, element_width)
                } else {
                    element
                };

                let start =
                    self.resolve_aggregate_references(start, values, aggregate_types, env)?;
                let width =
                    self.resolve_aggregate_references(width, values, aggregate_types, env)?;
                let width = self.const_eval(&width, env.params, env.module_name)?;
                if width <= 0 {
                    return Err(format!(
                        "indexed part-select width must be positive, got {width}"
                    ));
                }
                let width = width as usize;
                if width > element_width {
                    return Err(format!(
                        "indexed part-select width {width} exceeds array element width {element_width}"
                    ));
                }
                let logical_start = if *ascending {
                    start
                } else {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(start),
                        Box::new(constant_expr(width as i64 - 1)),
                    )
                };
                let offset = ast::Expr::Bin(
                    ast::BinOpSrc::Sub,
                    Box::new(logical_start),
                    Box::new(constant_expr(logical_base)),
                );
                if let Ok(logical_start) = self.const_eval(&offset, env.params, env.module_name) {
                    if logical_start < 0 || logical_start + width as i64 > element_width as i64 {
                        return Err(format!(
                            "indexed part-select [{logical_start}+:{}] is out of array-element range",
                            width
                        ));
                    }
                    let selected = ast::Expr::Slice {
                        value: Box::new(normalized_element),
                        msb: Box::new(constant_expr(logical_start + width as i64 - 1)),
                        lsb: Box::new(constant_expr(logical_start)),
                    };
                    return Ok(if let Some(valid) = array_index_valid {
                        ast::Expr::Cond(
                            Box::new(valid),
                            Box::new(selected),
                            Box::new(ast::Expr::Num {
                                width: Some(width),
                                word: LogicWord::xes(width),
                                orig: "function_runtime_array_index_x".to_string(),
                            }),
                        )
                    } else {
                        selected
                    });
                }

                let max_offset = element_width - width;
                let in_range = ast::Expr::Bin(
                    ast::BinOpSrc::LogAnd,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Ge,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(0)),
                    )),
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Le,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(max_offset as i64)),
                    )),
                );
                let selected = ast::Expr::Bin(
                    ast::BinOpSrc::And,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Shr,
                        Box::new(normalized_element),
                        Box::new(offset),
                    )),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::ones(width),
                        orig: "function_array_runtime_mask".to_string(),
                    }),
                );
                let packed_result = ast::Expr::Cond(
                    Box::new(in_range),
                    Box::new(selected),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::xes(width),
                        orig: "function_array_runtime_x".to_string(),
                    }),
                );
                return Ok(if let Some(valid) = array_index_valid {
                    ast::Expr::Cond(
                        Box::new(valid),
                        Box::new(packed_result),
                        Box::new(ast::Expr::Num {
                            width: Some(width),
                            word: LogicWord::xes(width),
                            orig: "function_runtime_array_index_x".to_string(),
                        }),
                    )
                } else {
                    packed_result
                });
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                if aggregate_types.arrays.contains_key(name) {
                    return Err(format!(
                        "indexed part-select is not supported on function array '{}'",
                        name
                    ));
                }
                let start =
                    self.resolve_aggregate_references(start, values, aggregate_types, env)?;
                let width =
                    self.resolve_aggregate_references(width, values, aggregate_types, env)?;
                let width = self.const_eval(&width, env.params, env.module_name)?;
                if width <= 0 {
                    return Err(format!(
                        "indexed part-select width must be positive, got {width}"
                    ));
                }
                let width = width as usize;
                let value = values
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("indexed part-select uses unknown local '{}'", name))?;
                let value_width = *aggregate_types.scalar_widths.get(name).ok_or_else(|| {
                    format!("indexed part-select local '{}' has unknown width", name)
                })?;
                if width > value_width {
                    return Err(format!(
                        "indexed part-select width {width} exceeds local width {value_width}"
                    ));
                }
                let (logical_msb, logical_lsb) = aggregate_types
                    .scalar_ranges
                    .get(name)
                    .copied()
                    .unwrap_or((value_width as i64 - 1, 0));
                let reversed = logical_msb < logical_lsb;
                let normalized_value = if reversed {
                    let mut physical_parts = Vec::with_capacity(value_width);
                    for physical_bit in 0..value_width {
                        physical_parts.push(ast::Expr::Slice {
                            value: Box::new(value.clone()),
                            msb: Box::new(constant_expr(physical_bit as i64)),
                            lsb: Box::new(constant_expr(physical_bit as i64)),
                        });
                    }
                    ast::Expr::Concat(physical_parts)
                } else {
                    value
                };
                if let Ok(start) = self.const_eval(&start, env.params, env.module_name) {
                    let logical_start = if *ascending {
                        start
                    } else {
                        start - width as i64 + 1
                    };
                    let normalized_start = if reversed {
                        logical_start - logical_msb
                    } else {
                        logical_start
                    };
                    return Ok(ast::Expr::Slice {
                        value: Box::new(normalized_value),
                        msb: Box::new(constant_expr(normalized_start + width as i64 - 1)),
                        lsb: Box::new(constant_expr(normalized_start)),
                    });
                }

                let offset = if *ascending {
                    start
                } else {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(start),
                        Box::new(constant_expr(width as i64 - 1)),
                    )
                };
                let offset = if reversed {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(offset),
                        Box::new(constant_expr(logical_msb)),
                    )
                } else {
                    offset
                };
                let max_offset = value_width - width;
                let in_range = ast::Expr::Bin(
                    ast::BinOpSrc::LogAnd,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Ge,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(0)),
                    )),
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Le,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(max_offset as i64)),
                    )),
                );
                let selected = ast::Expr::Bin(
                    ast::BinOpSrc::And,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Shr,
                        Box::new(normalized_value),
                        Box::new(offset),
                    )),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::ones(width),
                        orig: "runtime_function_indexed_mask".to_string(),
                    }),
                );
                ast::Expr::Cond(
                    Box::new(in_range),
                    Box::new(selected),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::xes(width),
                        orig: "runtime_function_indexed_x".to_string(),
                    }),
                )
            }
            ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
                name.clone(),
                Box::new(self.resolve_aggregate_references(msb, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(lsb, values, aggregate_types, env)?),
            ),
            ast::Expr::Un(op, value) => ast::Expr::Un(
                *op,
                Box::new(self.resolve_aggregate_references(value, values, aggregate_types, env)?),
            ),
            ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
                *op,
                Box::new(self.resolve_aggregate_references(left, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(right, values, aggregate_types, env)?),
            ),
            ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
                Box::new(self.resolve_aggregate_references(
                    condition,
                    values,
                    aggregate_types,
                    env,
                )?),
                Box::new(self.resolve_aggregate_references(
                    true_value,
                    values,
                    aggregate_types,
                    env,
                )?),
                Box::new(self.resolve_aggregate_references(
                    false_value,
                    values,
                    aggregate_types,
                    env,
                )?),
            ),
            ast::Expr::Concat(parts) => ast::Expr::Concat(
                parts
                    .iter()
                    .map(|part| {
                        self.resolve_aggregate_references(part, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
                Box::new(self.resolve_aggregate_references(count, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(value, values, aggregate_types, env)?),
            ),
            ast::Expr::Call(name, args) => ast::Expr::Call(
                name.clone(),
                args.iter()
                    .map(|arg| self.resolve_aggregate_references(arg, values, aggregate_types, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
                value: Box::new(self.resolve_aggregate_references(
                    value,
                    values,
                    aggregate_types,
                    env,
                )?),
                msb: Box::new(self.resolve_aggregate_references(
                    msb,
                    values,
                    aggregate_types,
                    env,
                )?),
                lsb: Box::new(self.resolve_aggregate_references(
                    lsb,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
            ast::Expr::Resize { width, value } => ast::Expr::Resize {
                width: *width,
                value: Box::new(self.resolve_aggregate_references(
                    value,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => ast::Expr::WildcardMatch {
                kind: *kind,
                selector: Box::new(self.resolve_aggregate_references(
                    selector,
                    values,
                    aggregate_types,
                    env,
                )?),
                label: Box::new(self.resolve_aggregate_references(
                    label,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
        })
    }

    fn update_aggregate_field(
        &self,
        current: ast::Expr,
        env: &Env<'_>,
        type_name: &str,
        field: &str,
        value: ast::Expr,
    ) -> Result<ast::Expr, String> {
        let definition = env
            .types
            .get(type_name)
            .ok_or_else(|| format!("unknown aggregate type '{}'", type_name))?;
        let definition = match definition {
            ResolvedType::Struct(definition) | ResolvedType::Union(definition) => definition,
            ResolvedType::Enum(_) => {
                return Err(format!("type '{}' is not a packed struct/union", type_name));
            }
        };
        let &(offset, width) = definition
            .fields
            .get(field)
            .ok_or_else(|| format!("type '{}' has no field '{}'", type_name, field))?;
        let total_width = definition.width;
        let mut parts = Vec::new();
        if offset + width < total_width {
            parts.push(ast::Expr::Slice {
                value: Box::new(current.clone()),
                msb: Box::new(constant_expr((total_width - 1) as i64)),
                lsb: Box::new(constant_expr((offset + width) as i64)),
            });
        }
        parts.push(ast::Expr::Resize {
            width,
            value: Box::new(value),
        });
        if offset > 0 {
            parts.push(ast::Expr::Slice {
                value: Box::new(current),
                msb: Box::new(constant_expr((offset - 1) as i64)),
                lsb: Box::new(constant_expr(0)),
            });
        }
        Ok(ast::Expr::Concat(parts))
    }

    fn merge_array_states(
        target: &mut FunctionAggregateState,
        mut then_state: HashMap<String, Vec<ast::Expr>>,
        else_state: HashMap<String, Vec<ast::Expr>>,
        condition: &ast::Expr,
    ) {
        let names = then_state
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        for name in names {
            let Some(mut then_values) = then_state.remove(&name) else {
                continue;
            };
            let Some(else_values) = else_state.get(&name) else {
                continue;
            };
            for (then_value, else_value) in then_values.iter_mut().zip(else_values) {
                if *then_value != *else_value {
                    *then_value = ast::Expr::Cond(
                        Box::new(condition.clone()),
                        Box::new(then_value.clone()),
                        Box::new(else_value.clone()),
                    );
                }
            }
            target.array_values.insert(name, then_values);
        }
    }

    fn local_zero(&self, range: &ast::RangeAst, env: &Env<'_>) -> Result<ast::Expr, String> {
        let width = match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                (hi - lo).unsigned_abs() as usize + 1
            }
            None => 1,
        };
        if width == 0 || width > 64 {
            return Err(format!(
                "function local variable width must be between 1 and 64, got {}",
                width
            ));
        }
        Ok(ast::Expr::Num {
            width: Some(width),
            word: LogicWord::from_u64(width, 0),
            orig: "0".to_string(),
        })
    }

    fn inline_nested_function_calls(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        Ok(match expression {
            ast::Expr::Call(name, arguments) if name != "$signed" && name != "$unsigned" => {
                self.build_function_expression(name, arguments, env, call_stack)?
            }
            ast::Expr::Call(name, arguments) => ast::Expr::Call(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Ident(_)
            | ast::Expr::ScopedIdent { .. }
            | ast::Expr::Num { .. }
            | ast::Expr::Str(_)
            | ast::Expr::Member(_, _)
            | ast::Expr::MemberChain(_) => expression.clone(),
            ast::Expr::Select(name, index) => ast::Expr::Select(
                name.clone(),
                Box::new(self.inline_nested_function_calls(index, env, call_stack)?),
            ),
            ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
                name.clone(),
                indexes
                    .iter()
                    .map(|index| self.inline_nested_function_calls(index, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => ast::Expr::ArrayIndexedPart {
                name: name.clone(),
                indexes: indexes
                    .iter()
                    .map(|index| self.inline_nested_function_calls(index, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
                start: Box::new(self.inline_nested_function_calls(start, env, call_stack)?),
                width: Box::new(self.inline_nested_function_calls(width, env, call_stack)?),
                ascending: *ascending,
            },
            ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
                name.clone(),
                Box::new(self.inline_nested_function_calls(msb, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(lsb, env, call_stack)?),
            ),
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => ast::Expr::IndexedPart {
                name: name.clone(),
                start: Box::new(self.inline_nested_function_calls(start, env, call_stack)?),
                width: Box::new(self.inline_nested_function_calls(width, env, call_stack)?),
                ascending: *ascending,
            },
            ast::Expr::Un(op, value) => ast::Expr::Un(
                *op,
                Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            ),
            ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
                *op,
                Box::new(self.inline_nested_function_calls(left, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(right, env, call_stack)?),
            ),
            ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
                Box::new(self.inline_nested_function_calls(condition, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(true_value, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(false_value, env, call_stack)?),
            ),
            ast::Expr::Concat(parts) => ast::Expr::Concat(
                parts
                    .iter()
                    .map(|part| self.inline_nested_function_calls(part, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
                Box::new(self.inline_nested_function_calls(count, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            ),
            ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
                value: Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
                msb: Box::new(self.inline_nested_function_calls(msb, env, call_stack)?),
                lsb: Box::new(self.inline_nested_function_calls(lsb, env, call_stack)?),
            },
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => ast::Expr::MethodCall {
                object: object.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => ast::Expr::MethodCallChain {
                path: path.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::New(size) => ast::Expr::New(Box::new(
                self.inline_nested_function_calls(size, env, call_stack)?,
            )),
            ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
                arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Resize { width, value } => ast::Expr::Resize {
                width: *width,
                value: Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            },
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => ast::Expr::WildcardMatch {
                kind: *kind,
                selector: Box::new(self.inline_nested_function_calls(selector, env, call_stack)?),
                label: Box::new(self.inline_nested_function_calls(label, env, call_stack)?),
            },
        })
    }

    /// `$sformatf(fmt, args...)`：本子集支持全常量参数的编译期格式化，
    /// 结果按 512 位打包字符串（≤64 字节、NUL 填充）返回。
    fn elab_sformatf(&self, arguments: &[ast::Expr], env: &Env<'_>) -> Result<ExprIR, String> {
        let Some((fmt_expr, value_args)) = arguments.split_first() else {
            return Err("$sformatf requires a format string".into());
        };
        let ast::Expr::Str(fmt) = fmt_expr else {
            return Err("$sformatf format must be a string literal".into());
        };
        let mut rendered = String::new();
        let mut arg_index = 0usize;
        let mut chars = fmt.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '%' {
                rendered.push(c);
                continue;
            }
            if chars.peek() == Some(&'%') {
                chars.next();
                rendered.push('%');
                continue;
            }
            let mut spec = chars.next().ok_or("$sformatf format ends after '%'")?;
            if spec == '0' {
                spec = chars.next().ok_or("$sformatf format ends after '%0'")?;
            }
            let Some(argument) = value_args.get(arg_index) else {
                return Err(format!(
                    "$sformatf has {} format specifiers but {} arguments",
                    arg_index + 1,
                    value_args.len()
                ));
            };
            arg_index += 1;
            let value = self.elab_expr(argument, env)?;
            let ExprIR::Const(word) = &value else {
                return Err("$sformatf in this subset requires constant arguments".to_string());
            };
            match spec {
                'd' | 'D' => {
                    let Some(v) = word.to_u64() else {
                        return Err("$sformatf %d argument contains x/z".into());
                    };
                    rendered.push_str(&v.to_string());
                }
                'h' | 'H' => {
                    let Some(v) = word.to_u64() else {
                        return Err("$sformatf %h argument contains x/z".into());
                    };
                    rendered.push_str(&format!("{v:x}"));
                }
                'b' | 'B' => rendered.push_str(&word.to_bin_str()),
                's' | 'S' => rendered.push_str(&word.to_ascii_string_nul_terminated()),
                other => {
                    return Err(format!(
                        "$sformatf does not support '%{other}' in this subset"
                    ))
                }
            }
        }
        if arg_index != value_args.len() {
            return Err(format!(
                "$sformatf has {} format specifiers but {} arguments",
                arg_index,
                value_args.len()
            ));
        }
        if rendered.len() > ast::STRING_MAX_BYTES {
            return Err(format!(
                "$sformatf result '{}' exceeds the {}-byte string subset",
                rendered,
                ast::STRING_MAX_BYTES
            ));
        }
        Ok(ExprIR::Const(LogicWord::from_bytes_le(
            ast::STRING_WIDTH,
            rendered.as_bytes(),
        )))
    }

    fn string_method_expression(
        &self,
        entry: &NetEntry,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<Option<ExprIR>, String> {
        if entry.type_name.as_deref() != Some("string") {
            return Ok(None);
        }
        match method {
            "len" => {
                if !arguments.is_empty() {
                    return Err("string.len() expects no arguments".to_string());
                }
                let byte_count = entry.width / 8;
                let mut length = ExprIR::Const(LogicWord::from_u64(32, byte_count as u64));
                for byte in (0..byte_count).rev() {
                    length = ExprIR::Cond {
                        c: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(self.string_byte_ref(entry, byte)),
                            b: Box::new(ExprIR::Const(LogicWord::zero(8))),
                        }),
                        t: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                        f: Box::new(length),
                    };
                }
                Ok(Some(length))
            }
            "getc" => {
                let Some(index) = arguments.first() else {
                    return Err("string.getc() expects one index argument".to_string());
                };
                if arguments.len() != 1 {
                    return Err("string.getc() expects one index argument".to_string());
                }
                let index = self.elab_expr(index, env)?;
                let byte_count = entry.width / 8;
                let mut selected = ExprIR::Const(LogicWord::zero(8));
                for byte in (0..byte_count).rev() {
                    selected = ExprIR::Cond {
                        c: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(index.clone()),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                        }),
                        t: Box::new(self.string_byte_ref(entry, byte)),
                        f: Box::new(selected),
                    };
                }
                Ok(Some(selected))
            }
            "toupper" | "tolower" => {
                if !arguments.is_empty() {
                    return Err(format!("string.{method}() expects no arguments"));
                }
                let upper = method == "toupper";
                let byte_count = entry.width / 8;
                let transformed = (0..byte_count)
                    .rev()
                    .map(|byte| self.string_case_byte(self.string_byte_ref(entry, byte), upper))
                    .collect();
                Ok(Some(ExprIR::Concat(transformed)))
            }
            "compare" | "icompare" => {
                let Some(argument) = arguments.first() else {
                    return Err(format!("string.{method}() expects one string argument"));
                };
                if arguments.len() != 1 {
                    return Err(format!("string.{method}() expects one string argument"));
                }
                let other = self.elab_expr(argument, env)?;
                Ok(Some(self.string_compare_expression(
                    entry,
                    other,
                    method == "icompare",
                )))
            }
            "substr" => {
                if arguments.len() != 2 {
                    return Err("string.substr() expects left and right indices".to_string());
                }
                let left = self.elab_expr(&arguments[0], env)?;
                let right = self.elab_expr(&arguments[1], env)?;
                Ok(Some(self.string_substr_expression(entry, left, right)))
            }
            _ => Ok(None),
        }
    }

    fn string_substr_expression(&self, entry: &NetEntry, left: ExprIR, right: ExprIR) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut bytes = Vec::with_capacity(byte_count);
        for output in 0..byte_count {
            let source_index = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(left.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(32, output as u64))),
            };
            let in_range = ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(left.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::zero(32))),
                }),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Le,
                    a: Box::new(source_index.clone()),
                    b: Box::new(right.clone()),
                }),
            };
            bytes.push(ExprIR::Cond {
                c: Box::new(in_range),
                t: Box::new(self.string_indexed_byte(entry, source_index)),
                f: Box::new(ExprIR::Const(LogicWord::zero(8))),
            });
        }
        bytes.reverse();
        ExprIR::Concat(bytes)
    }

    fn string_indexed_byte(&self, entry: &NetEntry, index: ExprIR) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut selected = ExprIR::Const(LogicWord::zero(8));
        for byte in (0..byte_count).rev() {
            selected = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                }),
                t: Box::new(self.string_byte_ref(entry, byte)),
                f: Box::new(selected),
            };
        }
        selected
    }

    fn string_compare_expression(
        &self,
        entry: &NetEntry,
        other: ExprIR,
        ignore_case: bool,
    ) -> ExprIR {
        let byte_count = entry.width / 8;
        let mut result = ExprIR::Const(LogicWord::zero(32));
        for byte in (0..byte_count).rev() {
            let mut left = self.string_byte_ref(entry, byte);
            let mut right = Self::string_expr_byte(other.clone(), byte);
            if ignore_case {
                left = self.string_case_byte(left, true);
                right = self.string_case_byte(right, true);
            }
            let ordering = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Lt,
                    a: Box::new(left.clone()),
                    b: Box::new(right.clone()),
                }),
                t: Box::new(ExprIR::Const(LogicWord::from_u64(32, (-1_i64) as u64))),
                f: Box::new(ExprIR::Const(LogicWord::from_u64(32, 1))),
            };
            result = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ne,
                    a: Box::new(left),
                    b: Box::new(right),
                }),
                t: Box::new(ordering),
                f: Box::new(result),
            };
        }
        result
    }

    fn string_expr_byte(value: ExprIR, byte: usize) -> ExprIR {
        let lsb = byte * 8;
        ExprIR::Index {
            e: Box::new(value),
            msb: lsb + 7,
            lsb,
        }
    }

    fn string_case_byte(&self, byte: ExprIR, upper: bool) -> ExprIR {
        let (low, delta, op) = if upper {
            (b'a', 32_u64, BinOp::Sub)
        } else {
            (b'A', 32_u64, BinOp::Add)
        };
        let high = low + 25;
        let in_range = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(ExprIR::Bin {
                op: BinOp::Ge,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
            }),
            b: Box::new(ExprIR::Bin {
                op: BinOp::Le,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, high as u64))),
            }),
        };
        ExprIR::Cond {
            c: Box::new(in_range),
            t: Box::new(ExprIR::Bin {
                op,
                a: Box::new(byte.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, delta))),
            }),
            f: Box::new(byte),
        }
    }

    fn string_putc_statement(
        &self,
        entry: &NetEntry,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        if arguments.len() != 2 {
            return Err("string.putc() expects index and byte arguments".to_string());
        }
        let index = self.elab_expr(&arguments[0], env)?;
        let value = self.elab_expr(&arguments[1], env)?;
        let byte_count = entry.width / 8;
        let mut statements = Vec::with_capacity(byte_count);
        for byte in 0..byte_count {
            let lsb = byte * 8;
            statements.push(StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(index.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, byte as u64))),
                },
                t: Box::new(StmtIR::Bl {
                    lv: LValue {
                        net: entry.base,
                        decl_width: entry.width,
                        range: Range::Span { msb: lsb + 7, lsb },
                    },
                    e: value.clone(),
                }),
                f: None,
            });
        }
        Ok(StmtIR::Block(statements))
    }

    fn string_itoa_statement(
        &mut self,
        entry: &NetEntry,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        let Some(argument) = arguments.first() else {
            return Err("string.itoa() expects one integer argument".to_string());
        };
        if arguments.len() != 1 {
            return Err("string.itoa() expects one integer argument".to_string());
        }
        let input = self.elab_expr(argument, env)?;
        let input = ExprIR::Signed {
            e: Box::new(input),
            signed: false,
        };

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let sign = declare_state(self, "__string_itoa_sign", 1)?;
        let magnitude = declare_state(self, "__string_itoa_magnitude", 32)?;
        let count = declare_state(self, "__string_itoa_count", 32)?;
        let mut digits = Vec::with_capacity(10);
        for digit in 0..10 {
            digits.push(declare_state(
                self,
                &format!("__string_itoa_digit_{digit}"),
                8,
            )?);
        }

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };
        let const32 = |value: u64| ExprIR::Const(LogicWord::from_u64(32, value));

        let input_bit31 = ExprIR::Index {
            e: Box::new(input.clone()),
            msb: 31,
            lsb: 31,
        };
        let negated = ExprIR::Bin {
            op: BinOp::Sub,
            a: Box::new(ExprIR::Const(LogicWord::zero(32))),
            b: Box::new(input.clone()),
        };
        let mut statements = vec![
            assign_state(&sign, input_bit31),
            StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&sign)),
                    b: Box::new(ExprIR::Const(LogicWord::ones(1))),
                },
                t: Box::new(assign_state(&magnitude, negated)),
                f: Some(Box::new(assign_state(&magnitude, input.clone()))),
            },
            assign_state(&count, const32(1)),
        ];

        let mut threshold = 10_u64;
        for _ in 1..10 {
            let more_digits = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(state_ref(&magnitude)),
                    b: Box::new(const32(threshold)),
                }),
                t: Box::new(const32(1)),
                f: Box::new(const32(0)),
            };
            let next_count = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(state_ref(&count)),
                b: Box::new(more_digits),
            };
            statements.push(assign_state(&count, next_count));
            threshold = threshold.saturating_mul(10);
        }

        for digit_entry in &digits {
            let digit_value = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Mod,
                    a: Box::new(state_ref(&magnitude)),
                    b: Box::new(const32(10)),
                }),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
            };
            let quotient = ExprIR::Bin {
                op: BinOp::Div,
                a: Box::new(state_ref(&magnitude)),
                b: Box::new(const32(10)),
            };
            statements.push(assign_state(digit_entry, digit_value));
            statements.push(assign_state(&magnitude, quotient));
        }

        for output in 0..entry.width / 8 {
            let mut byte_expr = ExprIR::Const(LogicWord::zero(8));
            for digit_count in 1..=10_usize {
                for signed in [false, true] {
                    let offset = usize::from(signed);
                    let digit_position = if signed && output == 0 {
                        Some(0)
                    } else {
                        output.checked_sub(offset)
                    };
                    let Some(digit_index) = digit_position else {
                        continue;
                    };
                    let Some(digit_index) = digit_count
                        .checked_sub(1)
                        .and_then(|highest| highest.checked_sub(digit_index))
                    else {
                        continue;
                    };
                    let character = if signed && output == 0 {
                        ExprIR::Const(LogicWord::from_u64(8, b'-' as u64))
                    } else {
                        state_ref(&digits[digit_index])
                    };
                    let matches = ExprIR::Bin {
                        op: BinOp::And,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(state_ref(&count)),
                            b: Box::new(const32(digit_count as u64)),
                        }),
                        b: Box::new(ExprIR::Bin {
                            op: BinOp::Eq,
                            a: Box::new(state_ref(&sign)),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(1, u64::from(signed)))),
                        }),
                    };
                    byte_expr = ExprIR::Cond {
                        c: Box::new(matches),
                        t: Box::new(character),
                        f: Box::new(byte_expr),
                    };
                }
            }
            let lsb = output * 8;
            statements.push(StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span { msb: lsb + 7, lsb },
                },
                e: byte_expr,
            });
        }

        Ok(StmtIR::Block(statements))
    }

    fn string_power_radix_statement(
        &mut self,
        entry: &NetEntry,
        method: &str,
        arguments: &[ast::Expr],
        env: &Env,
    ) -> Result<StmtIR, String> {
        let Some(argument) = arguments.first() else {
            return Err(format!("string.{method}() expects one integer argument"));
        };
        if arguments.len() != 1 {
            return Err(format!("string.{method}() expects one integer argument"));
        }
        let (base_bits, digit_count) = match method {
            "hextoa" => (4_usize, 8_usize),
            "bintoa" => (1, 32),
            _ => return Err(format!("unsupported string format method '{method}'")),
        };
        let input = self.elab_expr(argument, env)?;
        let input = ExprIR::Signed {
            e: Box::new(input),
            signed: false,
        };

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let value = declare_state(self, "__string_radix_format_value", 32)?;
        let count = declare_state(self, "__string_radix_format_count", 32)?;
        let mut digits = Vec::with_capacity(digit_count);
        for digit in 0..digit_count {
            digits.push(declare_state(
                self,
                &format!("__string_radix_format_digit_{digit}"),
                8,
            )?);
        }

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };
        let const32 = |value: u64| ExprIR::Const(LogicWord::from_u64(32, value));

        let mut statements = vec![
            assign_state(&value, input.clone()),
            assign_state(&count, const32(1)),
        ];
        let base = 1_u64 << base_bits;
        let mut threshold = base;
        for _ in 1..digit_count {
            let more_digits = ExprIR::Cond {
                c: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(state_ref(&value)),
                    b: Box::new(const32(threshold)),
                }),
                t: Box::new(const32(1)),
                f: Box::new(const32(0)),
            };
            let next_count = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(state_ref(&count)),
                b: Box::new(more_digits),
            };
            statements.push(assign_state(&count, next_count));
            threshold = threshold.saturating_mul(base);
        }

        for digit_entry in &digits {
            let raw_digit = ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(state_ref(&value)),
                b: Box::new(const32(base - 1)),
            };
            let ascii_digit = if method == "bintoa" {
                ExprIR::Bin {
                    op: BinOp::Add,
                    a: Box::new(raw_digit),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                }
            } else {
                ExprIR::Cond {
                    c: Box::new(ExprIR::Bin {
                        op: BinOp::Lt,
                        a: Box::new(raw_digit.clone()),
                        b: Box::new(const32(10)),
                    }),
                    t: Box::new(ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(raw_digit.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    }),
                    f: Box::new(ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(raw_digit),
                            b: Box::new(const32(10)),
                        }),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'a' as u64))),
                    }),
                }
            };
            let shifted = ExprIR::Bin {
                op: BinOp::Shr,
                a: Box::new(state_ref(&value)),
                b: Box::new(const32(base_bits as u64)),
            };
            statements.push(assign_state(digit_entry, ascii_digit));
            statements.push(assign_state(&value, shifted));
        }

        for output in 0..entry.width / 8 {
            let mut byte_expr = ExprIR::Const(LogicWord::zero(8));
            for possible_count in 1..=digit_count {
                let Some(digit_index) = possible_count
                    .checked_sub(1)
                    .and_then(|highest| highest.checked_sub(output))
                else {
                    continue;
                };
                let matches = ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&count)),
                    b: Box::new(const32(possible_count as u64)),
                };
                byte_expr = ExprIR::Cond {
                    c: Box::new(matches),
                    t: Box::new(state_ref(&digits[digit_index])),
                    f: Box::new(byte_expr),
                };
            }
            let lsb = output * 8;
            statements.push(StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span { msb: lsb + 7, lsb },
                },
                e: byte_expr,
            });
        }

        Ok(StmtIR::Block(statements))
    }

    fn string_radix_statement(
        &mut self,
        entry: &NetEntry,
        target: LValue,
        method: &str,
        arguments: &[ast::Expr],
        _env: &Env,
    ) -> Result<StmtIR, String> {
        let base = match method {
            "atoi" => 10_u64,
            "atohex" => 16,
            "atobin" => 2,
            _ => return Err(format!("unsupported string radix method '{method}'")),
        };
        if !arguments.is_empty() {
            return Err(format!("string.{method}() expects no arguments"));
        }

        let declare_state = |self_: &mut Self, name: &str, width: usize| {
            self_.declare_net(
                name,
                "",
                &Some((
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::from_u64(32, width as i64 as u64),
                        orig: (width - 1).to_string(),
                    }),
                    Box::new(ast::Expr::Num {
                        width: Some(32),
                        word: LogicWord::zero(32),
                        orig: "0".to_string(),
                    }),
                )),
                &HashMap::new(),
                true,
                false,
                None,
                None,
            )
        };
        let value = declare_state(self, "__string_radix_value", 32)?;
        let sign = declare_state(self, "__string_radix_sign", 32)?;
        let started = declare_state(self, "__string_radix_started", 1)?;
        let sign_seen = declare_state(self, "__string_radix_sign_seen", 1)?;
        let stopped = declare_state(self, "__string_radix_stopped", 1)?;

        let state_ref = |entry: &NetEntry| -> ExprIR {
            ExprIR::Ref {
                net: entry.base,
                decl_width: entry.width,
                range: full_range(entry.width),
                signed: false,
            }
        };
        let assign_state = |entry: &NetEntry, value: ExprIR| -> StmtIR {
            StmtIR::Bl {
                lv: LValue {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                },
                e: value,
            }
        };

        let mut statements = vec![
            assign_state(&value, ExprIR::Const(LogicWord::zero(32))),
            assign_state(&sign, ExprIR::Const(LogicWord::from_u64(32, 1))),
            assign_state(&started, ExprIR::Const(LogicWord::zero(1))),
            assign_state(&sign_seen, ExprIR::Const(LogicWord::zero(1))),
            assign_state(&stopped, ExprIR::Const(LogicWord::zero(1))),
        ];

        for byte in 0..entry.width / 8 {
            let b = self.string_byte_ref(entry, byte);
            let eq = |value: u8| ExprIR::Bin {
                op: BinOp::Eq,
                a: Box::new(b.clone()),
                b: Box::new(ExprIR::Const(LogicWord::from_u64(8, value as u64))),
            };
            let in_range = |low: u8, high: u8| ExprIR::Bin {
                op: BinOp::And,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Ge,
                    a: Box::new(b.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
                }),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Le,
                    a: Box::new(b.clone()),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(8, high as u64))),
                }),
            };
            let (is_digit, digit) = match method {
                "atoi" => {
                    let is_digit = in_range(b'0', b'9');
                    let digit = ExprIR::Bin {
                        op: BinOp::Sub,
                        a: Box::new(b.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    };
                    (is_digit, digit)
                }
                "atobin" => {
                    let zero = eq(b'0');
                    let one = eq(b'1');
                    let is_digit = ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(zero),
                        b: Box::new(one.clone()),
                    };
                    let digit = ExprIR::Cond {
                        c: Box::new(one),
                        t: Box::new(ExprIR::Const(LogicWord::from_u64(8, 1))),
                        f: Box::new(ExprIR::Const(LogicWord::zero(8))),
                    };
                    (is_digit, digit)
                }
                _ => {
                    let decimal = in_range(b'0', b'9');
                    let lower = in_range(b'a', b'f');
                    let upper = in_range(b'A', b'F');
                    let is_digit = ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(decimal.clone()),
                        b: Box::new(ExprIR::Bin {
                            op: BinOp::Or,
                            a: Box::new(lower.clone()),
                            b: Box::new(upper.clone()),
                        }),
                    };
                    let decimal_digit = ExprIR::Bin {
                        op: BinOp::Sub,
                        a: Box::new(b.clone()),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, b'0' as u64))),
                    };
                    let add_offset = |low: u8| ExprIR::Bin {
                        op: BinOp::Add,
                        a: Box::new(ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(b.clone()),
                            b: Box::new(ExprIR::Const(LogicWord::from_u64(8, low as u64))),
                        }),
                        b: Box::new(ExprIR::Const(LogicWord::from_u64(8, 10))),
                    };
                    let digit = ExprIR::Cond {
                        c: Box::new(decimal),
                        t: Box::new(decimal_digit),
                        f: Box::new(ExprIR::Cond {
                            c: Box::new(lower),
                            t: Box::new(add_offset(b'a')),
                            f: Box::new(ExprIR::Cond {
                                c: Box::new(upper),
                                t: Box::new(add_offset(b'A')),
                                f: Box::new(ExprIR::Const(LogicWord::zero(8))),
                            }),
                        }),
                    };
                    (is_digit, digit)
                }
            };

            let whitespace = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(eq(b' ')),
                b: Box::new(ExprIR::Bin {
                    op: BinOp::Or,
                    a: Box::new(eq(b'\t')),
                    b: Box::new(ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(eq(b'\n')),
                        b: Box::new(eq(b'\r')),
                    }),
                }),
            };
            let plus = eq(b'+');
            let minus = eq(b'-');
            let continued = ExprIR::Bin {
                op: BinOp::Add,
                a: Box::new(ExprIR::Bin {
                    op: BinOp::Mul,
                    a: Box::new(state_ref(&value)),
                    b: Box::new(ExprIR::Const(LogicWord::from_u64(32, base))),
                }),
                b: Box::new(digit.clone()),
            };
            let digit_statements = StmtIR::Block(vec![
                StmtIR::If {
                    c: ExprIR::Bin {
                        op: BinOp::Eq,
                        a: Box::new(state_ref(&started)),
                        b: Box::new(ExprIR::Const(LogicWord::zero(1))),
                    },
                    t: Box::new(assign_state(&value, digit)),
                    f: Some(Box::new(assign_state(&value, continued))),
                },
                assign_state(&started, ExprIR::Const(LogicWord::ones(1))),
                assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1))),
            ]);
            let prefix_consumed = ExprIR::Bin {
                op: BinOp::Or,
                a: Box::new(state_ref(&started)),
                b: Box::new(state_ref(&sign_seen)),
            };
            let non_digit = StmtIR::If {
                c: prefix_consumed,
                t: Box::new(assign_state(&stopped, ExprIR::Const(LogicWord::ones(1)))),
                f: Some(Box::new(StmtIR::If {
                    c: whitespace,
                    t: Box::new(StmtIR::Block(Vec::new())),
                    f: Some(Box::new(StmtIR::If {
                        c: plus,
                        t: Box::new(assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1)))),
                        f: Some(Box::new(StmtIR::If {
                            c: minus,
                            t: Box::new(StmtIR::Block(vec![
                                assign_state(
                                    &sign,
                                    ExprIR::Const(LogicWord::from_u64(32, (-1_i64) as u64)),
                                ),
                                assign_state(&sign_seen, ExprIR::Const(LogicWord::ones(1))),
                            ])),
                            f: Some(Box::new(assign_state(
                                &stopped,
                                ExprIR::Const(LogicWord::ones(1)),
                            ))),
                        })),
                    })),
                })),
            };
            statements.push(StmtIR::If {
                c: ExprIR::Bin {
                    op: BinOp::Eq,
                    a: Box::new(state_ref(&stopped)),
                    b: Box::new(ExprIR::Const(LogicWord::ones(1))),
                },
                t: Box::new(StmtIR::Block(Vec::new())),
                f: Some(Box::new(StmtIR::If {
                    c: is_digit,
                    t: Box::new(digit_statements),
                    f: Some(Box::new(non_digit)),
                })),
            });
        }

        statements.push(StmtIR::Bl {
            lv: target,
            e: ExprIR::Bin {
                op: BinOp::Mul,
                a: Box::new(state_ref(&sign)),
                b: Box::new(state_ref(&value)),
            },
        });
        Ok(StmtIR::Block(statements))
    }

    fn string_byte_ref(&self, entry: &NetEntry, byte: usize) -> ExprIR {
        let lsb = byte * 8;
        ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: Range::Span { msb: lsb + 7, lsb },
            signed: false,
        }
    }

    fn class_handle_null_compare(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let ast::Expr::Bin(op, a, b) = expression else {
            return Ok(None);
        };
        let equality = match op {
            ast::BinOpSrc::Eq => true,
            ast::BinOpSrc::Ne => false,
            _ => return Ok(None),
        };
        let is_null =
            |expression: &ast::Expr| matches!(expression, ast::Expr::Ident(name) if name == "null");
        if is_null(a) && is_null(b) {
            return Ok(Some(ExprIR::Const(LogicWord::from_u64(
                1,
                u64::from(equality),
            ))));
        }
        let handle = if is_null(a) {
            b
        } else if is_null(b) {
            a
        } else {
            return Ok(None);
        };
        let flag = self
            .class_handle_flag_expression(handle, env)?
            .ok_or_else(|| "null comparisons require a class handle".to_string())?;
        let comparison = ExprIR::Bin {
            op: if equality { BinOp::Eq } else { BinOp::Ne },
            a: Box::new(flag),
            b: Box::new(ExprIR::Const(LogicWord::zero(1))),
        };
        Ok(Some(comparison))
    }

    fn class_handle_equality_compare(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let ast::Expr::Bin(op, a, b) = expression else {
            return Ok(None);
        };
        let equality = match op {
            ast::BinOpSrc::Eq => true,
            ast::BinOpSrc::Ne => false,
            _ => return Ok(None),
        };
        let Some(left_flag) = self.class_handle_flag_expression(a, env)? else {
            return Ok(None);
        };
        let Some(right_flag) = self.class_handle_flag_expression(b, env)? else {
            return Ok(None);
        };
        let Some(left_oid) = self.class_handle_object_id_expression(a, env)? else {
            return Ok(None);
        };
        let Some(right_oid) = self.class_handle_object_id_expression(b, env)? else {
            return Ok(None);
        };
        let object_ids_equal = ExprIR::Bin {
            op: BinOp::Eq,
            a: Box::new(left_oid),
            b: Box::new(right_oid),
        };
        let both_allocated = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(left_flag),
            b: Box::new(right_flag),
        };
        let equal = ExprIR::Bin {
            op: BinOp::And,
            a: Box::new(both_allocated),
            b: Box::new(object_ids_equal),
        };
        Ok(Some(if equality {
            equal
        } else {
            ExprIR::Un {
                op: UnOp::Not,
                a: Box::new(equal),
            }
        }))
    }

    fn class_handle_flag_expression(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let flag_net = match expression {
            ast::Expr::Ident(name) => {
                if !env.class_handles.contains_key(name)
                    && !env.class_aliases.borrow().contains_key(name)
                {
                    return Ok(None);
                }
                env.canon(name)
            }
            ast::Expr::Member(object, field) => {
                let path = vec![object.clone(), field.clone()];
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { flag_net, .. } => flag_net,
                    ClassPathTarget::Field(_) => return Ok(None),
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { flag_net, .. } => flag_net,
                ClassPathTarget::Field(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        let entry = env.net(&flag_net)?;
        Ok(Some(ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        }))
    }

    fn class_handle_object_id_expression(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
    ) -> Result<Option<ExprIR>, String> {
        let handle = match expression {
            ast::Expr::Ident(name) => {
                if !env.class_handles.contains_key(name)
                    && !env.class_aliases.borrow().contains_key(name)
                {
                    return Ok(None);
                }
                env.canon(name)
            }
            ast::Expr::Member(object, field) => {
                let path = vec![object.clone(), field.clone()];
                match self.resolve_class_path(env, &path)? {
                    ClassPathTarget::Handle { prefix, .. } => prefix,
                    ClassPathTarget::Field(_) => return Ok(None),
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { prefix, .. } => prefix,
                ClassPathTarget::Field(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        let entry = env.net(&object_id_net(&handle))?;
        Ok(Some(ExprIR::Ref {
            net: entry.base,
            decl_width: entry.width,
            range: full_range(entry.width),
            signed: false,
        }))
    }

    fn elab_expr(&self, e: &ast::Expr, env: &Env) -> Result<ExprIR, String> {
        if let Some(comparison) = self.class_handle_null_compare(e, env)? {
            return Ok(comparison);
        }
        if let Some(comparison) = self.class_handle_equality_compare(e, env)? {
            return Ok(comparison);
        }
        Ok(match e {
            ast::Expr::Num { width, word, .. } => {
                ExprIR::Const(word.clone().sized(width.unwrap_or(32)))
            }
            ast::Expr::Str(s) => {
                // 字符串字面量：≤64 字符，首字符在最低字节，NUL 填充到
                // 512 位，与 `string` 类型变量布局一致。
                const STR_MAX: usize = ast::STRING_MAX_BYTES;
                if s.len() > STR_MAX {
                    return Err(format!(
                        "string literals are limited to {STR_MAX} bytes in this subset, got {} bytes",
                        s.len()
                    ));
                }
                ExprIR::Const(LogicWord::from_bytes_le(ast::STRING_WIDTH, s.as_bytes()))
            }
            ast::Expr::Ident(n) => {
                if let Some(value) = env.method_args.get(n) {
                    return Ok(value.clone());
                }
                if let Some(pv) = env.params.get(n) {
                    return Ok(ExprIR::Const(LogicWord::from_u64(32, *pv as u64)));
                }
                let entry = env.net(n)?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: full_range(entry.width),
                    signed: entry.is_signed,
                }
            }
            ast::Expr::ScopedIdent { package, name } => {
                let value = self
                    .packages
                    .constants
                    .get(package)
                    .and_then(|values| values.get(name))
                    .copied()
                    .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name))?;
                ExprIR::Const(LogicWord::from_u64(32, value as u64))
            }
            ast::Expr::Select(n, idx) => {
                let entry = env.net(n)?;
                let i = self.const_eval(idx, env.params, env.module_name)?;
                let off = if entry.array.is_some() {
                    entry.array_offset(&[i])?
                } else {
                    entry.offset(i)?
                };
                let range = if let Some(array) = entry.array.as_ref() {
                    Range::Span {
                        msb: off + array.element_width - 1,
                        lsb: off,
                    }
                } else {
                    Range::Bit(off)
                };
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range,
                    signed: false,
                }
            }
            ast::Expr::ArraySelect(..) => {
                let ast::Expr::ArraySelect(n, indexes) = e else {
                    unreachable!("pattern was already checked");
                };
                let entry = env.net(n)?;
                let mut resolved = Vec::with_capacity(indexes.len());
                for index in indexes {
                    resolved.push(self.const_eval(index, env.params, env.module_name)?);
                }
                let off = entry.array_offset(&resolved)?;
                let element_width = entry
                    .array
                    .as_ref()
                    .map(|array| array.element_width)
                    .ok_or_else(|| format!("'{n}' is not an unpacked array"))?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    range: Range::Span {
                        msb: off + element_width - 1,
                        lsb: off,
                    },
                    signed: false,
                }
            }
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => self.array_element_indexed_expr(name, indexes, start, width, *ascending, env)?,
            ast::Expr::Member(object, field) => {
                let (net, decl_width, range) = self.member_reference(env, object, field, false)?;
                ExprIR::Ref {
                    net,
                    decl_width,
                    range,
                    signed: false,
                }
            }
            ast::Expr::MemberChain(path) => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        range: full_range(entry.width),
                        signed: entry.is_signed,
                    }
                }
                ClassPathTarget::Handle { .. } => {
                    return Err("class handle paths cannot be read as values".into());
                }
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => match self.resolve_class_path(env, path)? {
                ClassPathTarget::Handle { prefix, class, .. } => {
                    return self.class_method_expression_on(
                        prefix.as_str(),
                        class.as_str(),
                        class.as_str(),
                        method,
                        arguments,
                        env,
                    );
                }
                ClassPathTarget::Field(net) => {
                    let entry = env.net(&net)?;
                    if let Some(value) =
                        self.string_method_expression(entry, method, arguments, env)?
                    {
                        return Ok(value);
                    }
                    return Err(format!(
                        "method '{}' called on a non-object field path",
                        method
                    ));
                }
            },
            ast::Expr::Part(n, a, b) => {
                let entry = env.net(n)?;
                let ia = self.const_eval(a, env.params, env.module_name)?;
                let ib = self.const_eval(b, env.params, env.module_name)?;
                if entry.array.is_some() {
                    let (lsb, width) = entry.array_slice(ia, ib)?;
                    return Ok(ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        signed: false,
                        range: Range::Span {
                            msb: lsb + width - 1,
                            lsb,
                        },
                    });
                }
                let (hi, lo) = if ia >= ib { (ia, ib) } else { (ib, ia) };
                let o1 = entry.offset(hi)?;
                let o2 = entry.offset(lo)?;
                ExprIR::Ref {
                    net: entry.base,
                    decl_width: entry.width,
                    signed: false,
                    range: Range::Span {
                        msb: o1.max(o2),
                        lsb: o1.min(o2),
                    },
                }
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                let entry = env.net(name)?;
                if entry.msb < entry.lsb {
                    return self.reversed_indexed_part_expr(entry, start, width, *ascending, env);
                }
                if self.const_eval(start, env.params, env.module_name).is_ok() {
                    let (msb, lsb) =
                        self.indexed_part_range(name, start, width, *ascending, env)?;
                    ExprIR::Ref {
                        net: entry.base,
                        decl_width: entry.width,
                        signed: false,
                        range: Range::Span { msb, lsb },
                    }
                } else {
                    self.dynamic_indexed_part_expr(name, start, width, *ascending, env)?
                }
            }
            ast::Expr::Un(op, a) => {
                let a = self.elab_expr(a, env)?;
                match op {
                    ast::UnOpSrc::Not => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(a),
                    },
                    ast::UnOpSrc::AndR | ast::UnOpSrc::OrR | ast::UnOpSrc::XorR => ExprIR::Un {
                        op: match op {
                            ast::UnOpSrc::AndR => UnOp::AndR,
                            ast::UnOpSrc::OrR => UnOp::OrR,
                            _ => UnOp::XorR,
                        },
                        a: Box::new(a),
                    },
                    ast::UnOpSrc::NandR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::AndR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::NorR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::XnorR => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::XorR,
                            a: Box::new(a),
                        }),
                    },
                    ast::UnOpSrc::Plus => a,
                    ast::UnOpSrc::Minus => {
                        let w = a.width().max(1);
                        ExprIR::Bin {
                            op: BinOp::Sub,
                            a: Box::new(ExprIR::Const(LogicWord::zero(w))),
                            b: Box::new(a),
                        }
                    }
                    ast::UnOpSrc::LogNot => ExprIR::Un {
                        op: UnOp::Not,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                    },
                }
            }
            ast::Expr::Bin(op, a, b) => {
                let a = self.elab_expr(a, env)?;
                let b = self.elab_expr(b, env)?;
                let ir_op = match op {
                    ast::BinOpSrc::Add => BinOp::Add,
                    ast::BinOpSrc::Sub => BinOp::Sub,
                    ast::BinOpSrc::Mul => BinOp::Mul,
                    ast::BinOpSrc::Div => BinOp::Div,
                    ast::BinOpSrc::Mod => BinOp::Mod,
                    ast::BinOpSrc::And => BinOp::And,
                    ast::BinOpSrc::Or => BinOp::Or,
                    ast::BinOpSrc::Xor => BinOp::Xor,
                    ast::BinOpSrc::Shl => BinOp::Shl,
                    ast::BinOpSrc::Shr => BinOp::Shr,
                    ast::BinOpSrc::Eq => BinOp::Eq,
                    ast::BinOpSrc::Ne => BinOp::Ne,
                    ast::BinOpSrc::Lt => BinOp::Lt,
                    ast::BinOpSrc::Le => BinOp::Le,
                    ast::BinOpSrc::Gt => BinOp::Gt,
                    ast::BinOpSrc::Ge => BinOp::Ge,
                    ast::BinOpSrc::LogAnd => BinOp::And,
                    ast::BinOpSrc::LogOr => BinOp::Or,
                    ast::BinOpSrc::CaseEq => BinOp::CaseEq,
                    ast::BinOpSrc::CaseNe => BinOp::CaseNe,
                };
                match op {
                    ast::BinOpSrc::LogAnd => ExprIR::Bin {
                        op: BinOp::And,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                        b: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(b),
                        }),
                    },
                    ast::BinOpSrc::LogOr => ExprIR::Bin {
                        op: BinOp::Or,
                        a: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(a),
                        }),
                        b: Box::new(ExprIR::Un {
                            op: UnOp::OrR,
                            a: Box::new(b),
                        }),
                    },
                    _ => ExprIR::Bin {
                        op: ir_op,
                        a: Box::new(a),
                        b: Box::new(b),
                    },
                }
            }
            ast::Expr::Cond(c, t, f) => ExprIR::Cond {
                c: Box::new(self.elab_expr(c, env)?),
                t: Box::new(self.elab_expr(t, env)?),
                f: Box::new(self.elab_expr(f, env)?),
            },
            ast::Expr::Concat(parts) => ExprIR::Concat(
                parts
                    .iter()
                    .map(|p| self.elab_expr(p, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, inner) => {
                let n = self.const_eval(count, env.params, env.module_name)?;
                if !(0..=4096).contains(&n) {
                    return Err(format!("replication count {} out of range", n));
                }
                let inner = self.elab_expr(inner, env)?;
                ExprIR::Repeat {
                    times: n as usize,
                    e: Box::new(inner),
                }
            }
            ast::Expr::Slice { value, msb, lsb } => {
                let inner = self.elab_expr(value, env)?;
                let msb = self.const_eval(msb, env.params, env.module_name)?;
                let lsb = self.const_eval(lsb, env.params, env.module_name)?;
                if msb < lsb || lsb < 0 || msb as usize >= inner.width() {
                    return Err(format!(
                        "invalid part-select [{}:{}] for expression width {}",
                        msb,
                        lsb,
                        inner.width()
                    ));
                }
                ExprIR::Index {
                    e: Box::new(inner),
                    msb: msb as usize,
                    lsb: lsb as usize,
                }
            }
            ast::Expr::Resize { width, value } => {
                if *width == 0 || *width > 4096 {
                    return Err(format!("invalid assignment width {}", width));
                }
                let inner = self.elab_expr(value, env)?;
                if inner.width() > *width {
                    ExprIR::Index {
                        e: Box::new(inner),
                        msb: width - 1,
                        lsb: 0,
                    }
                } else if inner.width() < *width {
                    ExprIR::Concat(vec![
                        ExprIR::Const(LogicWord::zero(width - inner.width())),
                        inner,
                    ])
                } else {
                    inner
                }
            }
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => {
                let selector = self.elab_expr(selector, env)?;
                let label = self.elab_expr(label, env)?;
                wildcard_label_condition(selector, &label, *kind)
            }
            ast::Expr::Call(name, arguments) if name == "$signed" || name == "$unsigned" => {
                if arguments.len() != 1 {
                    return Err(format!("'{}' expects exactly one argument", name));
                }
                let inner = self.elab_expr(&arguments[0], env)?;
                ExprIR::Signed {
                    e: Box::new(inner),
                    signed: name == "$signed",
                }
            }
            ast::Expr::Call(name, arguments) if name == "$sformatf" => {
                self.elab_sformatf(arguments, env)?
            }
            ast::Expr::Call(name, _) => {
                let arguments = match e {
                    ast::Expr::Call(_, arguments) => arguments,
                    _ => unreachable!(),
                };
                return self.inline_function_call(name, arguments, env);
            }
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => {
                if object == "this" || object == "super" || env.class_handles.contains_key(object) {
                    return self.class_method_expression(object, method, arguments, env);
                }
                if let Ok(entry) = env.net(object) {
                    if let Some(value) =
                        self.string_method_expression(entry, method, arguments, env)?
                    {
                        return Ok(value);
                    }
                }
                if method != "size" || !arguments.is_empty() {
                    return Err(format!(
                        "unsupported dynamic-array method '{object}.{method}'"
                    ));
                }
                let entry = env.net(object)?;
                let array = entry
                    .array
                    .as_ref()
                    .ok_or_else(|| format!("'{object}' is not a dynamic array"))?;
                if !array.dynamic {
                    return Err(format!("'{object}' is not a dynamic array"));
                }
                ExprIR::Ref {
                    net: array.dynamic_size_base,
                    decl_width: 9,
                    range: full_range(9),
                    signed: false,
                }
            }
            ast::Expr::New(_) => {
                return Err("dynamic-array allocation is supported only as an assignment".into());
            }
            ast::Expr::NewClass(_) => {
                return Err(
                    "class construction is supported only as an assignment to a handle".into(),
                );
            }
        })
    }
}

fn reduce(items: &[ExprIR], op: BinOp) -> ExprIR {
    items
        .iter()
        .cloned()
        .reduce(|a, b| ExprIR::Bin {
            op,
            a: Box::new(a),
            b: Box::new(b),
        })
        .unwrap_or(ExprIR::Const(LogicWord::zero(1)))
}

fn full_range(width: usize) -> Range {
    if width == 1 {
        Range::Bit(0)
    } else {
        Range::Span {
            msb: width - 1,
            lsb: 0,
        }
    }
}

fn range_bits(net: &usize, range: &Range) -> Vec<usize> {
    match range {
        Range::Bit(offset) => vec![net + offset],
        Range::Span { msb, lsb } => {
            if msb >= lsb {
                (*lsb..=*msb).map(|offset| net + offset).collect()
            } else {
                (*msb..=*lsb).rev().map(|offset| net + offset).collect()
            }
        }
    }
}

fn find_conn<'a>(conns: &'a [ast::Conn], name: &str, index: usize) -> Option<&'a ast::Expr> {
    let mut pos = 0;
    for c in conns {
        match c {
            ast::Conn::Named(n, e) => {
                if n == name {
                    return e.as_ref();
                }
            }
            ast::Conn::Pos(e) => {
                if pos == index {
                    return e.as_ref();
                }
                pos += 1;
            }
        }
    }
    None
}

fn const_eval_packages(
    expression: &ast::Expr,
    params: &HashMap<String, i64>,
    module_name: &str,
    packages: &PackageTables,
) -> Result<i64, String> {
    let resolved = resolve_package_expressions(expression, packages)?;
    const_eval(&resolved, params, module_name)
}

fn resolve_package_expressions(
    expression: &ast::Expr,
    packages: &PackageTables,
) -> Result<ast::Expr, String> {
    Ok(match expression {
        ast::Expr::ScopedIdent { package, name } => {
            let value = packages
                .constants
                .get(package)
                .and_then(|values| values.get(name))
                .copied()
                .ok_or_else(|| format!("unknown package constant '{}::{}'", package, name))?;
            ast::Expr::Num {
                width: Some(32),
                word: LogicWord::from_u64(32, value as u64),
                orig: value.to_string(),
            }
        }
        ast::Expr::Ident(_)
        | ast::Expr::Member(_, _)
        | ast::Expr::MemberChain(_)
        | ast::Expr::Num { .. }
        | ast::Expr::Str(_) => expression.clone(),
        ast::Expr::Select(name, index) => ast::Expr::Select(
            name.clone(),
            Box::new(resolve_package_expressions(index, packages)?),
        ),
        ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
            name.clone(),
            indexes
                .iter()
                .map(|index| resolve_package_expressions(index, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::ArrayIndexedPart {
            name,
            indexes,
            start,
            width,
            ascending,
        } => ast::Expr::ArrayIndexedPart {
            name: name.clone(),
            indexes: indexes
                .iter()
                .map(|index| resolve_package_expressions(index, packages))
                .collect::<Result<Vec<_>, _>>()?,
            start: Box::new(resolve_package_expressions(start, packages)?),
            width: Box::new(resolve_package_expressions(width, packages)?),
            ascending: *ascending,
        },
        ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
            name.clone(),
            Box::new(resolve_package_expressions(msb, packages)?),
            Box::new(resolve_package_expressions(lsb, packages)?),
        ),
        ast::Expr::MethodCall {
            object,
            method,
            arguments,
        } => ast::Expr::MethodCall {
            object: object.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ast::Expr::MethodCallChain {
            path,
            method,
            arguments,
        } => ast::Expr::MethodCallChain {
            path: path.clone(),
            method: method.clone(),
            arguments: arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ast::Expr::New(size) => {
            ast::Expr::New(Box::new(resolve_package_expressions(size, packages)?))
        }
        ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
            arguments
                .iter()
                .map(|argument| resolve_package_expressions(argument, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::IndexedPart {
            name,
            start,
            width,
            ascending,
        } => ast::Expr::IndexedPart {
            name: name.clone(),
            start: Box::new(resolve_package_expressions(start, packages)?),
            width: Box::new(resolve_package_expressions(width, packages)?),
            ascending: *ascending,
        },
        ast::Expr::Un(op, a) => {
            ast::Expr::Un(*op, Box::new(resolve_package_expressions(a, packages)?))
        }
        ast::Expr::Bin(op, a, b) => ast::Expr::Bin(
            *op,
            Box::new(resolve_package_expressions(a, packages)?),
            Box::new(resolve_package_expressions(b, packages)?),
        ),
        ast::Expr::Cond(c, t, f) => ast::Expr::Cond(
            Box::new(resolve_package_expressions(c, packages)?),
            Box::new(resolve_package_expressions(t, packages)?),
            Box::new(resolve_package_expressions(f, packages)?),
        ),
        ast::Expr::Concat(parts) => ast::Expr::Concat(
            parts
                .iter()
                .map(|part| resolve_package_expressions(part, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
            Box::new(resolve_package_expressions(count, packages)?),
            Box::new(resolve_package_expressions(value, packages)?),
        ),
        ast::Expr::Call(name, args) => ast::Expr::Call(
            name.clone(),
            args.iter()
                .map(|arg| resolve_package_expressions(arg, packages))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
            value: Box::new(resolve_package_expressions(value, packages)?),
            msb: Box::new(resolve_package_expressions(msb, packages)?),
            lsb: Box::new(resolve_package_expressions(lsb, packages)?),
        },
        ast::Expr::Resize { width, value } => ast::Expr::Resize {
            width: *width,
            value: Box::new(resolve_package_expressions(value, packages)?),
        },
        ast::Expr::WildcardMatch {
            kind,
            selector,
            label,
        } => ast::Expr::WildcardMatch {
            kind: *kind,
            selector: Box::new(resolve_package_expressions(selector, packages)?),
            label: Box::new(resolve_package_expressions(label, packages)?),
        },
    })
}

fn const_eval(
    e: &ast::Expr,
    params: &HashMap<String, i64>,
    module_name: &str,
) -> Result<i64, String> {
    match e {
        ast::Expr::Num { word, .. } => word
            .to_u64()
            .map(|v| v as i64)
            .ok_or_else(|| "constant contains x/z".to_string()),
        ast::Expr::Ident(n) => params
            .get(n)
            .copied()
            .ok_or_else(|| format!("'{}' is not a parameter (in module '{}')", n, module_name)),
        ast::Expr::Call(name, args) if name == "$signed" || name == "$unsigned" => {
            if args.len() != 1 {
                return Err(format!("'{}' constant expects one argument", name));
            }
            let value = const_eval(&args[0], params, module_name)?;
            if name == "$unsigned" {
                return Ok(value);
            }
            match &args[0] {
                ast::Expr::Num {
                    width: Some(width), ..
                } if *width < 64 => {
                    let bit = (value as u64 >> (*width - 1)) & 1;
                    if bit == 1 {
                        Ok(value - (1_i64 << *width))
                    } else {
                        Ok(value)
                    }
                }
                _ => Ok(value),
            }
        }
        ast::Expr::Resize { width, value } => {
            let value = const_eval(value, params, module_name)?;
            if *width < 64 {
                let modulo = 1_i64 << *width;
                Ok(value.rem_euclid(modulo))
            } else {
                Ok(value)
            }
        }
        ast::Expr::Un(op, a) => {
            let v = const_eval(a, params, module_name)?;
            Ok(match op {
                ast::UnOpSrc::Minus => -v,
                ast::UnOpSrc::Plus => v,
                ast::UnOpSrc::LogNot => (v == 0) as i64,
                _ => return Err("unsupported constant unary operator".into()),
            })
        }
        ast::Expr::Bin(op, a, b) => {
            let x = const_eval(a, params, module_name)?;
            let y = const_eval(b, params, module_name)?;
            Ok(match op {
                ast::BinOpSrc::Add => x.wrapping_add(y),
                ast::BinOpSrc::Sub => x.wrapping_sub(y),
                ast::BinOpSrc::Mul => x.wrapping_mul(y),
                ast::BinOpSrc::Div => {
                    if y == 0 {
                        return Err("division by zero in constant".into());
                    }
                    x / y
                }
                ast::BinOpSrc::Shl => x.wrapping_shl(y as u32),
                ast::BinOpSrc::Shr => x.wrapping_shr(y as u32),
                ast::BinOpSrc::Eq => (x == y) as i64,
                ast::BinOpSrc::Ne => (x != y) as i64,
                ast::BinOpSrc::Lt => (x < y) as i64,
                ast::BinOpSrc::Le => (x <= y) as i64,
                ast::BinOpSrc::Gt => (x > y) as i64,
                ast::BinOpSrc::Ge => (x >= y) as i64,
                _ => return Err("unsupported constant binary operator".into()),
            })
        }
        _ => Err("expression is not constant (parameters/ranges/indices must be constant)".into()),
    }
}

struct Env<'a> {
    nets: &'a HashMap<String, NetEntry>,
    interface_ports: &'a HashMap<String, HashMap<String, NetEntry>>,
    interface_directions: &'a HashMap<String, HashMap<String, ast::DirSrc>>,
    params: &'a HashMap<String, i64>,
    types: &'a HashMap<String, ResolvedType>,
    functions: &'a HashMap<String, ast::FunctionDef>,
    module_name: &'a str,
    prefix: &'a str,
    /// Handle variable name -> class name for class-typed nets.
    class_handles: &'a HashMap<String, String>,
    /// 引用别名：句柄 -> 组代表句柄。`b = d` 后写 b 可经 d 观察。
    /// Rc 共享：方法体内建立的别名对后续语句可见（connect 阶段语义）。
    class_aliases: std::rc::Rc<std::cell::RefCell<HashMap<String, String>>>,
    /// While elaborating a class method: (handle, dynamic class of the
    /// allocated object).
    this_handle: Option<(&'a str, &'a str)>,
    /// Class in which the currently elaborated method was found; used to
    /// resolve `super` dispatch.
    method_owner: Option<&'a str>,
    /// Elaborated actuals bound to the current method's formal arguments.
    method_args: &'a HashMap<String, ExprIR>,
    /// Scratch return-value net for a value-returning method invoked as a
    /// statement: (method name, net base, width).
    method_return: Option<(&'a str, usize, usize)>,
}

impl<'a> Env<'a> {
    /// 句柄的组代表（无别名时为自身）。
    fn canon(&self, handle: &str) -> String {
        self.class_aliases
            .borrow()
            .get(handle)
            .cloned()
            .unwrap_or_else(|| handle.to_string())
    }

    fn net(&self, name: &str) -> Result<&NetEntry, String> {
        self.nets.get(name).ok_or_else(|| {
            format!(
                "module '{}': undeclared identifier '{}'",
                self.module_name, name
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::ir::{BinOp, Expr, Range, Stmt};

    fn compile_ok(src: &str) -> Design {
        compile(src, None).unwrap_or_else(|e| panic!("compile failed: {}", e))
    }

    #[test]
    fn simple_assign_flattens() {
        let d = compile_ok("module m(input a, b, output y); assign y = a & b; endmodule");
        assert_eq!(d.name, "m");
        assert_eq!(d.ports.len(), 3);
        assert_eq!(d.n_nets, 3);
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(d.assigns[0].e, Expr::Bin { op: BinOp::And, .. }));
    }

    #[test]
    fn vectors_and_parts() {
        let d = compile_ok("module m(input [3:0] a, output [1:0] y); assign y = a[3:2]; endmodule");
        assert_eq!(d.n_nets, 6);
        assert_eq!(d.assigns[0].lv.range, Range::Span { msb: 1, lsb: 0 });
        match &d.assigns[0].e {
            Expr::Ref { range, .. } => assert_eq!(*range, Range::Span { msb: 3, lsb: 2 }),
            other => panic!("expected ref, got {:?}", other),
        }
    }

    #[test]
    fn reversed_range_decl() {
        let d = compile_ok("module m(input [0:3] a, output y); assign y = a[2]; endmodule");
        // [0:3]: source index 2 -> decl offset 3 - 2 = 1
        match &d.assigns[0].e {
            Expr::Ref { range, .. } => assert_eq!(*range, Range::Bit(1)),
            other => panic!("expected ref, got {:?}", other),
        }
    }

    #[test]
    fn out_of_range_index_is_error() {
        assert!(compile(
            "module m(input [3:0] a, output y); assign y = a[7]; endmodule",
            None
        )
        .is_err());
    }

    #[test]
    fn undeclared_ident_is_error() {
        assert!(compile("module m(output y); assign y = q; endmodule", None).is_err());
    }

    #[test]
    fn parameterized_width() {
        let d = compile_ok(
            "module m #(parameter W = 4) (input [W-1:0] a, output [W-1:0] y); assign y = a + 1; endmodule",
        );
        assert_eq!(d.n_nets, 8);
    }

    #[test]
    fn systemverilog_variable_types_have_default_widths() {
        let d = compile_ok(
            "module m(input int a, output byte b, output tri z, output bit d, output logic [7:0] q); \
             shortint s; longint l; integer i; logic x; \
             endmodule",
        );
        for (name, width, is_reg) in [
            ("a", 32, true),
            ("b", 8, true),
            ("z", 1, false),
            ("d", 1, true),
            ("q", 8, true),
            ("s", 16, true),
            ("l", 64, true),
            ("i", 32, true),
            ("x", 1, true),
        ] {
            let declaration = d
                .net_decls
                .iter()
                .find(|declaration| declaration.name == name)
                .unwrap_or_else(|| panic!("missing declaration {name}"));
            assert_eq!(declaration.width, width, "{name} width");
            assert_eq!(declaration.is_reg, is_reg, "{name} variable flag");
        }
    }

    #[test]
    fn package_constants_import_and_scoped_references_elaborate() {
        let d = compile_ok(
            r#"
package base;
    parameter WIDTH = 8;
endpackage

package bus;
    parameter WIDTH = base::WIDTH + 1;
    localparam DEPTH = 4;
endpackage

import bus::*;

module top(
    input  [bus::WIDTH-1:0] a,
    output [bus::DEPTH-1:0] d
);
wire [WIDTH-1:0] wide;
wire [bus::DEPTH-1:0] generated_wire;
generate
    if (bus::WIDTH > 4) begin : wide_bus
        assign generated_wire = d;
    end else begin : narrow_bus
        assign generated_wire = d;
    end
endgenerate
assign d = wide;
endmodule
"#,
        );
        assert_eq!(d.port("a").unwrap().width, 9);
        assert_eq!(d.port("d").unwrap().width, 4);
        assert_eq!(
            d.net_decls.iter().find(|n| n.name == "wide").unwrap().width,
            9
        );
        assert_eq!(
            d.net_decls
                .iter()
                .find(|n| n.name == "generated_wire")
                .unwrap()
                .width,
            4
        );
    }

    #[test]
    fn enum_typedefs_define_width_and_constants() {
        let local = compile_ok(
            r#"
module top(output reg [1:0] y);
typedef enum logic [1:0] { IDLE = 2, RUN, DONE } state_t;
state_t state;
initial begin
    state = IDLE;
    y = state;
end
endmodule
"#,
        );
        assert_eq!(
            local
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "state")
                .unwrap()
                .width,
            2
        );

        let package = compile_ok(
            r#"
package states;
parameter BASE = 3;
typedef enum [2:0] { X = BASE, Y, Z } state_t;
endpackage

import states::*;

module top(output state_t q);
assign q = X;
endmodule
"#,
        );
        assert_eq!(package.port("q").unwrap().width, 3);
        assert!(matches!(
            package
                .assigns
                .iter()
                .find(|assignment| assignment.lv.net == package.port("q").unwrap().base)
                .map(|assignment| &assignment.e),
            Some(ExprIR::Const(value)) if value.to_u64() == Some(3)
        ));
    }

    #[test]
    fn packed_struct_typedefs_elaborate_fields_and_members() {
        let local = compile_ok(
            r#"
module top(input logic [7:0] value, output logic [3:0] high, output logic low);
typedef struct packed {
    logic [3:0] first;
    logic second;
} pair_t;
pair_t pair;
assign pair = value;
assign high = pair.first;
assign low = pair.second;
endmodule
"#,
        );
        assert_eq!(
            local
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "pair")
                .unwrap()
                .width,
            5
        );
        assert_eq!(local.port("high").unwrap().width, 4);
        assert_eq!(local.port("low").unwrap().width, 1);

        let package = compile_ok(
            r#"
package bus;
typedef struct packed {
    logic [7:0] address;
    logic [1:0] byte_enable;
} request_t;
endpackage

import bus::*;

module top(input request_t request, output [7:0] address, output [1:0] enable);
assign address = request.address;
assign enable = request.byte_enable;
endmodule
"#,
        );
        assert_eq!(package.port("request").unwrap().width, 10);
        let request = package.port("request").unwrap();
        let address = package
            .assigns
            .iter()
            .find(|assignment| assignment.lv.net == package.port("address").unwrap().base)
            .unwrap();
        let enable = package
            .assigns
            .iter()
            .find(|assignment| assignment.lv.net == package.port("enable").unwrap().base)
            .unwrap();
        assert!(matches!(
            &address.e,
            ExprIR::Ref { net, range, .. }
                if *net == request.base && *range == Range::Span { msb: 9, lsb: 2 }
        ));
        assert!(matches!(
            &enable.e,
            ExprIR::Ref { net, range, .. }
                if *net == request.base && *range == Range::Span { msb: 1, lsb: 0 }
        ));
    }

    #[test]
    fn packed_union_typedefs_overlap_members() {
        let local = compile_ok(
            r#"
module top(input logic [7:0] value, output logic [7:0] bytes, output logic [3:0] nibble);
typedef union packed {
    logic [7:0] bytes;
    logic [3:0] nibble;
} view_t;
view_t view;
assign view = value;
assign bytes = view.bytes;
assign nibble = view.nibble;
endmodule
"#,
        );
        assert_eq!(
            local
                .net_decls
                .iter()
                .find(|declaration| declaration.name == "view")
                .unwrap()
                .width,
            8
        );

        let package = compile_ok(
            r#"
package views;
typedef union packed {
    logic [7:0] wide;
    logic [1:0] narrow;
} view_t;
endpackage

import views::*;

module top(input view_t view, output [7:0] wide, output [1:0] narrow);
assign wide = view.wide;
assign narrow = view.narrow;
endmodule
"#,
        );
        assert_eq!(package.port("view").unwrap().width, 8);
        let view = package.port("view").unwrap();
        let wide = package
            .assigns
            .iter()
            .find(|assignment| assignment.lv.net == package.port("wide").unwrap().base)
            .unwrap();
        let narrow = package
            .assigns
            .iter()
            .find(|assignment| assignment.lv.net == package.port("narrow").unwrap().base)
            .unwrap();
        assert!(matches!(
            &wide.e,
            ExprIR::Ref { net, range, .. }
                if *net == view.base && *range == Range::Span { msb: 7, lsb: 0 }
        ));
        assert!(matches!(
            &narrow.e,
            ExprIR::Ref { net, range, .. }
                if *net == view.base && *range == Range::Span { msb: 1, lsb: 0 }
        ));
    }

    #[test]
    fn generate_for_loop_expands_assigns() {
        let d = compile_ok(
            "module m(input [3:0] a, output [3:0] y); generate for (genvar i=0; i<4; i=i+1) begin : bit_assign assign y[i] = a[i]; end endgenerate endmodule",
        );
        assert_eq!(d.assigns.len(), 4);
        for (expected, assign) in d.assigns.iter().enumerate() {
            assert_eq!(assign.lv.range, eda_core::ir::Range::Bit(expected));
        }
    }

    #[test]
    fn generate_if_selects_parameterized_branch() {
        let invert = compile_ok(
            "module m #(parameter INVERT = 1) (input a, output y); generate if (INVERT) begin : inv assign y = ~a; end else begin : pass assign y = a; end endgenerate endmodule",
        );
        assert_eq!(invert.assigns.len(), 1);
        assert!(matches!(
            invert.assigns[0].e,
            eda_core::ir::Expr::Un { op: UnOp::Not, .. }
        ));

        let identity = compile_ok(
            "module m #(parameter INVERT = 0) (input a, output y); generate if (INVERT) begin : inv assign y = ~a; end else begin : pass assign y = a; end endgenerate endmodule",
        );
        assert_eq!(identity.assigns.len(), 1);
        assert!(matches!(
            identity.assigns[0].e,
            eda_core::ir::Expr::Ref { .. }
        ));
    }

    #[test]
    fn generate_case_selects_constant_item() {
        let source = |mode: usize| {
            format!(
                "module m #(parameter MODE = {mode}) (output [7:0] y); \
                 generate case (MODE) \
                 0: begin : zero assign y = 8'd10; end \
                 1, 2: begin : one_or_two assign y = 8'd20; end \
                 default: begin : other assign y = 8'd30; end \
                 endcase endgenerate endmodule"
            )
        };
        for (mode, expected) in [(0, 10), (1, 20), (2, 20), (3, 30)] {
            let design = compile_ok(&source(mode));
            assert_eq!(design.assigns.len(), 1);
            match &design.assigns[0].e {
                ExprIR::Const(word) => assert_eq!(word.to_u64(), Some(expected)),
                other => panic!("expected constant, got {:?}", other),
            }
        }

        assert!(compile(
            "module m #(parameter MODE = 4) (output y); \
             generate case (MODE) 0: begin : zero assign y = 1'b0; end endcase endgenerate endmodule",
            None
        )
        .is_err());
    }

    #[test]
    fn instance_flattening() {
        let d = compile_ok(
            "module inv(input a, output y); assign y = ~a; endmodule
             module top(input x, output z); wire w; inv u1(.a(x), .y(w)); inv u2(.a(w), .y(z)); endmodule",
        );
        assert_eq!(d.name, "top");
        assert_eq!(d.n_nets, 7); // x, z, w + 2 per instance
        assert!(d.net_decls.iter().any(|n| n.name == "u1.a"));
        assert!(d.net_decls.iter().any(|n| n.name == "u2.y"));
    }

    #[test]
    fn instance_positional_and_params() {
        let d = compile_ok(
            "module add #(parameter W = 2) (input [W-1:0] a, b, output [W-1:0] s);
                assign s = a + b;
             endmodule
             module top(input [3:0] x, y, output [3:0] z);
                add #(4) u1(x, y, z);
             endmodule",
        );
        assert_eq!(d.name, "top");
        assert_eq!(d.n_nets, 24); // top 12 + child 12
    }

    #[test]
    fn hierarchy_metadata_is_preserved() {
        let d = compile_ok(
            "module leaf(input a, output y); assign y = ~a; endmodule
             module sub(input a, output y); wire n; leaf u_leaf(.a(a), .y(n)); assign y = n; endmodule
             module top(input a, output y); sub u1(.a(a), .y(y)); endmodule",
        );
        assert_eq!(d.hierarchy.len(), 3);
        assert_eq!(d.hierarchy[0].path, "top");
        assert_eq!(d.hierarchy[0].parent, None);
        assert_eq!(d.hierarchy[0].depth, 0);
        assert_eq!(d.hierarchy[1].path, "top.u1");
        assert_eq!(d.hierarchy[1].module, "sub");
        assert_eq!(d.hierarchy[1].parent.as_deref(), Some("top"));
        assert_eq!(d.hierarchy[2].path, "top.u1.u_leaf");
        assert_eq!(d.hierarchy[2].module, "leaf");
        assert_eq!(d.hierarchy[2].parent.as_deref(), Some("top.u1"));
        assert_eq!(d.hierarchy[2].depth, 2);
        assert!(d.hierarchy[2].net_start >= d.hierarchy[1].net_start);
        assert!(d.hierarchy[2].net_end <= d.hierarchy[1].net_end);
        assert_eq!(
            d.hierarchy[2]
                .ports
                .iter()
                .map(|port| (port.name.as_str(), port.direction, port.width))
                .collect::<Vec<_>>(),
            vec![("a", Dir::In, 1), ("y", Dir::Out, 1)]
        );
        assert_eq!(d.instance("top.u1").unwrap().module, "sub");
        assert_eq!(d.instances_of_module("leaf").len(), 1);
        let leaf_a = d.hierarchy[2].ports[0].base;
        assert_eq!(
            d.owning_instance(leaf_a)
                .map(|instance| instance.path.as_str()),
            Some("top.u1.u_leaf")
        );
        assert!(d.hierarchy_report().contains("top.u1.u_leaf"));
    }

    #[test]
    fn gate_primitives_become_assigns() {
        let d = compile_ok(
            "module m(input a, b, output y, n); and g1(y, a, b); not g2(n, y); endmodule",
        );
        assert_eq!(d.assigns.len(), 2);
    }

    #[test]
    fn always_blocks_and_sensitivity() {
        let d = compile_ok(
            "module m(input clk, rst, d, output reg q);
                always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
             endmodule",
        );
        assert_eq!(d.processes.len(), 1);
        assert!(matches!(&d.processes[0].body, Stmt::If { .. }));
    }

    #[test]
    fn case_becomes_case() {
        let d = compile_ok(
            "module m(input [1:0] sel, output reg y);
                always @(*) case (sel) 2'b00: y = 1'b1; 2'b01: y = 1'b0; default: y = 1'b1; endcase
             endmodule",
        );
        match &d.processes[0].body {
            Stmt::Case { arms, dflt, .. } => {
                assert_eq!(arms.len(), 2);
                assert!(dflt.is_some());
            }
            other => panic!("expected case, got {:?}", other),
        }
    }

    #[test]
    fn wildcard_case_lowers_to_ordered_conditionals() {
        let d = compile_ok(
            "module m(input [3:0] sel, output reg [1:0] y); always @(*) casez (sel) 4'b1???: y = 2'd3; default: y = 2'd0; endcase endmodule",
        );
        match &d.processes[0].body {
            Stmt::If { f: Some(_), .. } => {}
            other => panic!("expected ordered wildcard conditional, got {:?}", other),
        }
    }

    #[test]
    fn simple_functions_inline_into_expressions() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [7:0] sum, product);
function logic [7:0] add(input logic [7:0] left, right);
    add = left + right;
endfunction
function logic [7:0] multiply(input logic [7:0] left, right);
    multiply = left * right;
endfunction
assign sum = add(a, b);
assign product = multiply(a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 2);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));
        assert!(matches!(&d.assigns[1].e, Expr::Bin { op: BinOp::Mul, .. }));

        assert!(compile(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] add(input logic [7:0] left, right);
    add = left + right;
endfunction
assign y = add(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn package_functions_import_and_inline() {
        let d = compile_ok(
            r#"
package math;
function logic [7:0] add(input logic [7:0] left, right);
    add = left + right;
endfunction

function logic [7:0] bitwise_or(input logic [7:0] left, right);
    bitwise_or = left | right;
endfunction
endpackage

import math::add, math::bitwise_or;

module top(input [7:0] a, b, output [7:0] sum, union_result);
assign sum = add(a, b);
assign union_result = bitwise_or(a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));
        assert!(matches!(&d.assigns[1].e, Expr::Bin { op: BinOp::Or, .. }));

        assert!(compile(
            r#"
package math;
function logic [7:0] add(input logic [7:0] left, right);
    add = left + right;
endfunction
endpackage

module top(input [7:0] a, output [7:0] y);
assign y = add(a, a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn nested_function_calls_inline_and_recursion_is_rejected() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] add(input logic [7:0] left, right);
    add = left + right;
endfunction
function logic [7:0] increment(input logic [7:0] value);
    increment = add(value, 8'd1);
endfunction
function logic [7:0] pipeline(input logic [7:0] left, right);
    pipeline = increment(add(left, right));
endfunction
assign y = pipeline(a, increment(b));
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        assert!(compile(
            r#"
module recursive(input [7:0] a, output [7:0] y);
function logic [7:0] left(input logic [7:0] value);
    left = right(value);
endfunction
function logic [7:0] right(input logic [7:0] value);
    right = left(value);
endfunction
assign y = left(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_local_variables_inline_in_assignment_order() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, c, output [7:0] y);
function logic [7:0] sum3(
    input logic [7:0] left, middle, right);
    logic [7:0] first;
    logic [7:0] second = 8'd2;
    first = left + middle;
    second = first + right;
    sum3 = second + 8'd1;
endfunction
assign y = sum3(a, b, c);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
    unknown = value;
    f = unknown;
endfunction
assign y = f(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_if_else_assignments_merge_to_conditionals() {
        let d = compile_ok(
            r#"
module top(input sel, [7:0] a, b, output [7:0] y);
function logic [7:0] select(input logic selector,
    input logic [7:0] left, right);
    logic [7:0] result;
    if (selector)
        result = left;
    else
        result = right;
    select = result;
endfunction
assign y = select(sel, a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

        let d = compile_ok(
            r#"
module top(input s0, s1, [7:0] a, b, output [7:0] y);
function logic [7:0] priority(input logic high, low,
    input logic [7:0] left, right);
    logic [7:0] result;
    if (high)
        result = left;
    else if (low)
        result = right;
    else
        result = 8'd0;
    priority = result;
endfunction
assign y = priority(s0, s1, a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

        assert!(compile(
            r#"
module bad(input sel, [7:0] a, output [7:0] y);
function logic [7:0] f(input logic selector,
    input logic [7:0] value);
    if (selector)
        f = value;
    f = 8'd0;
endfunction
assign y = f(sel, a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_exact_case_assignments_merge_in_priority_order() {
        let d = compile_ok(
            r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
    input logic [7:0] left, right);
    logic [7:0] result;
    case (selector)
        2'd0: result = left + 8'd1;
        2'd1, 2'd2: result = right ^ 8'd2;
        default: result = left & right;
    endcase
    decode = result;
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

        let d = compile_ok(
            r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
    input logic [7:0] left, right);
    case (selector)
        2'd0: decode = left;
        2'd1: decode = right;
        default: decode = left | right;
    endcase
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

        assert!(compile(
            r#"
module bad(input [1:0] sel, [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [1:0] selector,
    input logic [7:0] value);
    casez (selector)
        2'b?0: f = value;
        default: f = 8'd0;
    endcase
endfunction
assign y = f(sel, a);
endmodule
"#,
            None
        )
        .is_ok());
    }

    #[test]
    fn function_wildcard_cases_preserve_dont_care_bits() {
        let d = compile_ok(
            r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
    input logic [7:0] left, right);
    logic [7:0] result;
    casez (selector)
        2'b?0: result = left;
        2'b11: result = right;
        default: result = left & right;
    endcase
    decode = result;
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));

        let d = compile_ok(
            r#"
module top(input [1:0] sel, [7:0] a, b, output [7:0] y);
function logic [7:0] decode(input logic [1:0] selector,
    input logic [7:0] left, right);
    casex (selector)
        2'b0?: decode = left;
        2'b1?: decode = right;
        default: decode = left | right;
    endcase
endfunction
assign y = decode(sel, a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Cond { .. }));
    }

    #[test]
    fn function_bounded_for_loops_unroll_during_inlining() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
    logic [7:0] total = value;
    for (int i = 0; i < 4; i = i + 1)
        total = total + 8'd1;
    bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
    logic [7:0] total = value;
    for (int i = 0; i < 2; i = i + 1)
        for (int j = 0; j < 2; j = j + 1)
            total = total + 8'd1;
    grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] stuck(input logic [7:0] value);
    for (int i = 0; i < 4; i = i)
        value = value + 8'd1;
    stuck = value;
endfunction
assign y = stuck(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_constant_repeat_loops_unroll_during_inlining() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
    logic [7:0] total = value;
    repeat (4)
        total = total + 8'd1;
    bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
    logic [7:0] total = value;
    repeat (2)
        repeat (2)
            total = total + 8'd1;
    grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] too_many(input logic [7:0] value);
    repeat (257)
        value = value + 8'd1;
    too_many = value;
endfunction
assign y = too_many(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_bounded_while_loops_unroll_during_inlining() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] bump(input logic [7:0] value);
    logic [7:0] total = value;
    int counter = 0;
    while (counter < 4) begin
        total = total + 8'd1;
        counter = counter + 1;
    end
    bump = total;
endfunction
assign y = bump(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        let d = compile_ok(
            r#"
module top(input [7:0] a, output [7:0] y);
function logic [7:0] grid(input logic [7:0] value);
    logic [7:0] total = value;
    int outer = 0;
    int inner = 0;
    while (outer < 2) begin
        inner = 0;
        while (inner < 2) begin
            total = total + 8'd1;
            inner = inner + 1;
        end
        outer = outer + 1;
    end
    grid = total;
endfunction
assign y = grid(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] runtime(input logic [7:0] value);
    while (value != 8'd0)
        value = value - 8'd1;
    runtime = value;
endfunction
assign y = runtime(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_packed_struct_locals_update_and_read_fields() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [11:0] y);
typedef struct packed {
    logic [7:0] high;
    logic [3:0] low;
} packet_t;
function logic [11:0] combine(input logic [7:0] left, right);
    packet_t packet;
    packet.high = left + 8'd1;
    packet.low = right;
    combine = packet;
endfunction
assign y = combine(a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Concat { .. }));

        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [7:0] y);
typedef struct packed {
    logic [7:0] high;
    logic [3:0] low;
} packet_t;
function logic [7:0] sum(input logic [7:0] left, right);
    packet_t packet;
    packet.high = left;
    packet.low = right;
    sum = packet.high + packet.low;
endfunction
assign y = sum(a, b);
endmodule
"#,
        );
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        let d = compile_ok(
            r#"
module top(input [3:0] a, output [7:0] y);
typedef union packed {
    logic [7:0] wide;
    logic [3:0] narrow;
} view_t;
function logic [7:0] view_value(input logic [3:0] value);
    view_t view;
    view.narrow = value;
    view_value = view.narrow;
endfunction
assign y = view_value(a);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
    packet_t packet;
    packet.high = value;
    f = packet.high;
endfunction
assign y = f(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_unpacked_array_locals_update_and_read_elements() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] sum4(input logic [7:0] left, right);
    logic [7:0] table [0:3];
    for (int i = 0; i < 4; i = i + 1)
        table[i] = left + right + i;
    sum4 = table[0] + table[1] + table[2] + table[3];
endfunction
assign y = sum4(a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
    logic [7:0] table [0:3];
    table[4] = value;
    f = table[4];
endfunction
assign y = f(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn function_multidimensional_unpacked_array_locals_select_elements() {
        let d = compile_ok(
            r#"
module top(input [7:0] a, b, output [7:0] y);
function logic [7:0] sum4(input logic [7:0] left, right);
    logic [7:0] table [0:1][1:0];
    for (int i = 0; i < 2; i = i + 1)
        for (int j = 0; j < 2; j = j + 1)
            table[i][j] = left + right + i * 2 + j;
    sum4 = table[0][0] + table[0][1] + table[1][0] + table[1][1];
endfunction
assign y = sum4(a, b);
endmodule
"#,
        );
        assert_eq!(d.assigns.len(), 1);
        assert!(matches!(&d.assigns[0].e, Expr::Bin { op: BinOp::Add, .. }));

        assert!(compile(
            r#"
module bad(input [7:0] a, output [7:0] y);
function logic [7:0] f(input logic [7:0] value);
    logic [7:0] table [0:1][1:0];
    table[0] = value;
    f = table[0][1];
endfunction
assign y = f(a);
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn interface_signals_connect_producer_and_consumer() {
        let d = compile_ok(
            r#"
interface bus;
    logic [7:0] data;
    logic valid;
    modport master(output valid, data);
    modport slave(input valid, data);
endinterface

module producer(interface bus.master b, input logic [7:0] value);
    assign b.valid = 1'b1;
    assign b.data = value;
endmodule

module consumer(interface bus.slave b, output logic [7:0] result);
    assign result = b.valid ? b.data : 8'd0;
endmodule

module top(input logic [7:0] value, output logic [7:0] result);
    bus shared();
    producer p(.b(shared), .value(value));
    consumer c(.b(shared), .result(result));
endmodule
"#,
        );
        assert_eq!(d.ports.len(), 2);
        assert!(d.assigns.len() >= 3);
        assert!(d.hierarchy.iter().any(|instance| instance.module == "bus"));

        assert!(compile(
            r#"
interface bus;
    logic [7:0] data;
    modport master(output data);
endinterface
module bad(interface bus.master b, output logic [7:0] y);
    assign y = b.data;
endmodule
module top(output logic [7:0] y);
    bus shared();
    bad b(.b(shared), .y(y));
endmodule
"#,
            None
        )
        .is_err());

        assert!(compile(
            r#"
interface bus;
    logic [7:0] data;
    modport slave(input data);
endinterface
module bad(interface bus.slave b, input logic [7:0] value);
    assign b.data = value;
endmodule
module top(input logic [7:0] value);
    bus shared();
    bad b(.b(shared), .value(value));
endmodule
"#,
            None
        )
        .is_err());
    }

    #[test]
    fn immediate_assertions_lower_to_simulation_statements() {
        let d = compile_ok(
            r#"
module checks(input logic [7:0] a, b);
initial begin
    #1;
    assert (a < b) else $error("a must be less than b");
end
endmodule
"#,
        );
        assert_eq!(d.initials.len(), 1);
        assert!(matches!(
            &d.initials[0],
            StmtIR::Block(statements) if matches!(statements[1], StmtIR::Assert { .. })
        ));

        assert!(compile("module m(input a); assert property (a); endmodule", None).is_err());
    }

    #[test]
    fn top_selection_prefers_tb() {
        let d = compile_ok(
            "module dut(input a, output y); assign y = a; endmodule
             module tb; reg a; wire y; dut u1(a, y); initial begin #1 a = 0; end endmodule",
        );
        assert_eq!(d.name, "tb");
    }

    #[test]
    fn explicit_top_overrides() {
        let d = compile(
            "module dut(input a, output y); assign y = a; endmodule
             module tb; reg a; wire y; dut u1(a, y); endmodule",
            Some("dut"),
        )
        .unwrap();
        assert_eq!(d.name, "dut");
        assert_eq!(d.ports.len(), 2);
    }

    #[test]
    fn concat_and_replicate() {
        let d = compile_ok(
            "module m(input [3:0] a, b, output [7:0] y, [7:0] z);
                assign y = {a, b};
                assign z = {2{a}};
             endmodule",
        );
        assert_eq!(d.n_nets, 24); // 4+4+8+8
        assert!(matches!(d.assigns[0].e, Expr::Concat(_)));
        assert!(matches!(d.assigns[1].e, Expr::Repeat { times: 2, .. }));
    }

    #[test]
    fn reg_ports_supported() {
        let d = compile_ok(
            "module cnt(input clk, output reg [3:0] q);
                always @(posedge clk) q <= q + 1;
             endmodule",
        );
        assert_eq!(d.ports.len(), 2);
        assert_eq!(d.processes.len(), 1);
    }

    #[test]
    fn recursive_instance_detected() {
        let e = compile(
            "module a(input x, output y); a u1(x, y); assign y = x; endmodule",
            None,
        );
        assert!(e.is_err());
    }

    #[test]
    fn unknown_instance_module_is_error() {
        assert!(compile(
            "module top(input x, output y); nosuch u1(x, y); endmodule",
            None
        )
        .is_err());
    }

    #[test]
    fn monitor_flag_propagates() {
        let d = compile_ok("module tb; reg a; initial $monitor(\"a=%b\", a); endmodule");
        match &d.initials[0] {
            Stmt::Display { monitor, .. } => assert!(*monitor),
            other => panic!("expected display, got {:?}", other),
        }
    }

    #[test]
    fn net_init_becomes_assign() {
        let d = compile_ok("module m(output y); wire w = 1'b1; assign y = w; endmodule");
        assert_eq!(d.assigns.len(), 2);
    }

    #[test]
    fn logical_ops_lowered_via_reduction() {
        let d = compile_ok(
            "module m(input [1:0] a, b, output y); assign y = (a == 2'd3) && (b != 0); endmodule",
        );
        // `&&` lowers to And of OrR reductions
        match &d.assigns[0].e {
            Expr::Bin { op: BinOp::And, .. } => {}
            other => panic!("expected and, got {:?}", other),
        }
    }

    #[test]
    fn delayed_runtime_continuous_lvalue_is_rejected() {
        let error = compile(
            "module m(input [1:0] selector, input [3:0] a, output reg [15:0] y); \
             assign #1 y[selector +: 4] = a; \
             endmodule",
            None,
        )
        .unwrap_err();
        assert!(
            error.contains("delayed runtime indexed continuous-assignment lvalue is not supported")
        );
    }

    #[test]
    fn sequence_composition_with_repetition_is_rejected() {
        let error = compile(
            "module m(input clk, req, first, second); \
             assert property (@(posedge clk) req |-> first and second[*2]); \
             endmodule",
            None,
        )
        .unwrap_err();
        assert!(error.contains("sequence composition does not yet compose with repetition"));
    }
}
