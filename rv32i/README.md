# rv32i — RV32I CPU 设计、仿真与验证（foundry signoff 目标载体）

从 RTL → class/UVM 风格 testbench → 回归 → （后续）综合/布局布线/STA/
DFT/签核的完整交付链路。

## 目录

- `rtl/gen_rv32i_core.py` — 核生成器（寄存器 + case 多路器风格）
- `rtl/rv32i_core.v` — 生成产物（请勿手改）
- `tb/tb_class.v` — class 组合式 testbench（Monitor/Scoreboard/Env）
- `programs/`、`verify/` — 预留
- 回归：`crates/eda-tests/src/rv32i_cases.rs`（Rust 侧指令编码器 + 程序驱动）

## 微架构

单周期核；IMEM/DMEM 各 16 词（512 位程序映像一拍装载）；32 项寄存器堆
（x0 恒零）；EBREAK/ECALL 停机、FENCE 当 nop；`dbg_pc/dbg_x10/dbg_x11`
调试端口。存储与寄存器堆用独立寄存器 + case 多路器实现 —— 即综合后
寄存器堆的真实门级结构，且完全在前端已验证的可综合子集内。

指令覆盖（RV32I 基础整型的全部访存 + 其余大部分）：

- OP-IMM：ADDI/SLTI/SLTIU/XORI/ORI/ANDI/SLLI/SRLI/SRAI
- OP：ADD/SUB/SLL/SLT/SLTU/XOR/SRL/SRA/OR/AND
- LUI/AUIPC、JAL/JALR
- 分支：BEQ/BNE/BLT/BGE/BLTU/BGEU
- 访存（全宽 + 子字）：LW/LH/LHU/LB/LBU、SW/SH/SB
  （子字装载含符号/零扩展与字节/半字选择；子字存储为读-改-写合并）
- SYSTEM：ECALL（cause=1，mcause=11）/EBREAK（cause=2，mcause=3）记录
  mepc/mcause 后停机并输出 `trap_cause`；
  Zicsr 子集：CSRRW/CSRRS/CSRRC + CSRRWI/CSRRSI/CSRRCI 立即数形式
  （mstatus/mie/mtvec/mscratch/mepc/mcause，旧值返回、rs1/zimm=0
  只读），MRET（返回 mepc、恢复 MIE），MIE=1 时 ECALL/EBREAK 跳
  mtvec（mepc=pc+4、MPIE←MIE、MIE←0），MIE=0 时记录后停机；未知
  CSR 标记 unsupported（cause=3）；
  FENCE/FENCE.I 显式 nop（单核顺序模型语义）

SRA 用全字算术恒等式实现（不依赖 `>>>`）；有符号比较用
「符号位 + 无符号比较」合成。

## 回归内容（15 个命名用例）

算术溢出对、逻辑运算、逻辑/算术移位、有/无符号比较、LUI/AUIPC、
计数循环求和（n=3/5/10）、JAL/JALR 调用返回、LW/SW 往返、
Fibonacci 混合程序、class 组合式 testbench（Monitor 采样 →
Scoreboard 比对 → Env 编排）。

## 签核进展（本轮）

- `rv32i_dft_scan_insertion_and_audit`：对 `rv32i_top` 插入并审计一条
  扫描链（链非空、无未扫描时序单元、扫描前后时序单元计数保持）。
- MMMC PDK 物理链路通过：fast/slow 双 corner、模型相关性、
  timing（slow 关键路径 201.125 ns）、DRC（0 error）、LEF/DEF/SDF
  物理库全生成；`layer ... tracks N` 驱动布线容量。
- **完整 `rv32i_pdk_signoff_mmmc_pass` 已取消门控并通过**：模型相关、
  sequential LEC、双角 timing、DRC、物理数据库全部 PASS。
- LEC 参考 Kepler Formal 的可扩展路径重写关键执行层：
  - 与 Kepler LEC 一样共享边界变量、构建去重 Boolean/CNF miter，并默认
    使用工业级 Kissat CDCL；Kissat 源码按 Kepler 绑定的
    `8af8e56f174b778aef3aa45af9f739b2a5f492c2` 提交 vendored 到
    `eda-formal`，由 build script 构建并通过 C API 取出 SAT/UNSAT 与
    SAT model，CEGAR refinement 不再把求解器容量误报为设计不等价。
  - 删除从未被求解的全网表死 CNF；`enc_cone_with_cuts` 改为谓词式割点，
    gate-side accepted 集合在归纳过程中维护，局部候选不再复制数千个
    cut；dense NetId identity hash 消除候选队列的 SipHash 开销。
  - 已由结构/局部/向量 CEGAR 证明的内部点不再在最终阶段逐点重放；
    最终只保留触发器次态与输出 obligations。触发器调度改为一次性
    计算的拓扑深度，不再为排序重复遍历 2051 个深锥。
  - 保留自研 incremental CDCL 作为 Kissat 初始化失败时的后备；Kissat
    为默认后端，保留 phase/model 语义与 fail-closed cancellation。
- 当前专用 profile 结论：`equivalent=true`、无 counterexample；
  3128 个直接命名点中证明 362 个，归纳接受 8660 个 correspondence
  points，最终 2406 个局部 miter queries 全部 UNSAT。专用 profile
  本机耗时约 3 分 55 秒。
- 验证入口：
  - `SUITE_FILTER=rv32i_pdk_signoff_mmmc_pass cargo test -p eda-tests
    --test suite full_suite_all_green -- --nocapture`：PASS
    （1 test，758.53s）。
  - `SUITE_FILTER=rv32i_ cargo test -p eda-tests --test suite
    full_suite_all_green -- --nocapture`：PASS（1 test，1530.13s）。
- 由此发现并保留两个真实工具 bug 的回归：组合 case 缺 default 曾推断
  锁存；synthesis If lowering 曾丢 else written set，导致 2049 位状态
  塌缩。二者均已修复。

## 签核工件

- `verify/signoff_report.txt`：分段签核总报告，含七段：
  1. 物理签核（模型相关/双角 timing/0 DRC/LEF-DEF-SDF）PASS；
  2. 功耗与静态 IR（2670.9 µW / 2.67 mA / 最差行 43）；
  3. 单层电源网格 Gauss-Seidel IR（128 节点 / 464 线段 / 43 次迭代
     收敛至 9.1e-10 V / 最差线段电流 55 µA）；
  4. 多层电源网格 M8/M7 + Black EM（256 节点 / 928 线段 / 43 次迭代 /
     最差 EM 寿命 3.32e10 h）；
  5. UPF 电源域 IR（TOP 域 18311 单元 / 3.74 mA / 域多层网格
     256 节点 928 线段 43 迭代；空 DEFAULT 域如实为零）；
  6. 时钟树综合（1 棵树、1245 个缓冲）；
  7. sequential LEC：PASS，无 counterexample。
- 套件常开用例：`rv32i_pdk_physical_signoff_mmmc_pass` 与完整
  `rv32i_pdk_signoff_mmmc_pass`（含 equivalence gate）。

## 功耗 / IR / CTS（套件常开）

- `rv32i_power_and_ir_analysis`：宽矢量活动格式——512 位 prog 端口
  直接以 `512'h...` 字面量驱动（功耗/IR 管线已支持任意位宽输入）+
  clk/rst/prog_ld 活动 → 全网翻转率 → 正总功耗/总电流、最差 IR 行、
  映射单元计数。
- `rv32i_cts_inserts_and_audits_clock_tree`：综合后网表时钟树综合
  （非空树、缓冲插入、时序单元保持）。
- `rv32i_power_mesh_ir_and_em`：布局后设计的电源网格 Gauss-Seidel IR
  求解（正总电流、有限最差压降、节点/线段非空、迭代收敛、网格报告）。
- `rv32i_multilayer_mesh_em_lifetime`：M8/M7 双层网格 + Black 方程
  电迁移寿命（正有限最差 EM 寿命、求解器收敛）。
- `rv32i_upf_domain_ir_and_mesh`：UPF 电源域视图（set_design_top /
  create_power_domain TOP / VDD-VSS 供电端口）→ 域感知 IR →
  每个有物理单元的域产出多层网格。

## 已修复：赋值按左值宽度补齐（表达式方法比较/算术形式恢复）

根因最终定位在仿真器而非类机制：`Stmt::Bl/NBl` 写入时逐位取
`v.bit(i)`，当 RHS 宽度（如 1 位比较结果）小于左值宽度（32 位
`int`）时，高位取到 X。修复：赋值前 `v.sized(lvalue_width)`
零扩展/截断。该修复同时恢复了：

- 表达式形式类方法中的比较/算术体（顶层与嵌套路径均正常）；
- 任何窄表达式赋宽目标的场景（`output int y3; y3 = (r == 5);`）。

回归用例 `sv_class_expression_method_comparisons` 覆盖顶层/嵌套的
get/is_zero/plus_two。

## 后续

- 类方法副作用表达式：`y = h.m(...)` 直接赋值现在会完整执行方法体，
  再读取返回 scratch；静态方法与经基类句柄的 virtual override 均覆盖；
  内置 `uvm_tlm_fifo.try_get()` 已改用该语义回归
- string 子集从 8 字节扩展为 64 字节固定宽度（512 位 NUL 填充）；
  `$sformatf`、字面量比较与 `%s` 显示均按新宽度编解码
- string 内建 `len()` 与 `getc()`：支持运行时索引、越界返回 0、
  空字符串以及类字符串属性路径（如 `h.name.len()`）
- string 内建 `putc()`、`toupper()`、`tolower()`：支持运行时索引自定义
  byte 写入、越界不改写、整串 ASCII 大小写转换与类属性路径
- string 内建 `compare()` / `icompare()`：按首个不同 byte 返回
  -1/0/1，`icompare` 先做 ASCII 大小写归一；支持字面量实参与类属性
- string 内建 `substr(left,right)`：闭区间子串拷贝，左右边界均可为
  运行时值；负左边界或 left>right 生成空串，支持类属性路径
- string 内建 `atoi()`：按直接赋值 lowering 为顺序 scratch 状态机，
  支持十进制数字、前导空白、`+`/`-` 符号、前缀截断与类属性路径
- 同一 radix 状态机支持 `atohex()` / `atobin()`：接受大小写十六进制
  digit、二进制 digit、前导空白与符号，并可解析类字符串属性
- string 内建 `itoa(integer)`：运行时 32 位有符号整数转十进制
  ASCII，支持 0、正数、负数、大负数与类属性路径
- string 内建 `hextoa(integer)` / `bintoa(integer)`：按 32 位无符号
  位型输出小写十六进制 / 二进制 ASCII，支持 0、正数、负数位型与类属性
- 类句柄引用别名已在 HDL 前端落地（`q = p` 共享存储、`new` 分裂）；
  uvm_component 内置 build/connect/run 三个虚 phase 钩子 + run_phases()
  驱动（经基类句柄与嵌套组合路径均生效）；run_phases 在 virtual
  run_phase 前后自动 raise/drop framework objection 并记录 drained；
  uvm_component 支持 name/set_name/get_name、parent 反向引用与
  child0..child3 四个显式子组件槽（add_child0..add_child3 绑定）；
  uvm_subscriber 虚 write
  支持分析风格事务扇出（多观察者各自 override）；内置
  uvm_analysis_port：connect(s) 经类类型形参引用绑定订阅者（实参可
  为顶层句柄或 env.mon 嵌套路径），write(d) 虚分派并计数；
  connect/connect1/connect2/connect3 分别绑定四个订阅者槽，
  write4(d) 对四个槽虚扇出，reject() 记录满连接拒绝
  uvm_tlm_fifo 深度 4 邮箱：try_put/try_get/peek + 满拒、空读与
  FIFO 顺序；sequencer 与 driver 经构造器句柄共享；uvm_objection：
  raise/drop 配对计数 + drained 终止判定
- 内置 UVM-lite 同步原语：uvm_event 提供 trigger/wait_trigger/reset/
  is_on 与触发/消费计数（顺序 pending 语义，不挂起进程）；
  uvm_barrier 提供阈值等待、计数饱和、reached 状态与 reset
- 内置 UVM-lite 深度 4 int 队列：push_front/push_back/pop_front/
  pop_back/size/delete，覆盖两端顺序、满拒、空读与成功计数
- 内置 UVM-lite 深度 4 int->int pool：唯一键 add/find/exists/remove/
  num/delete，已有键更新不增加数量，删除后按插入顺序左移
- 内置静态 uvm_config_db 子集：深度 4 string->int，类作用域
  set/get/exists/remove，已有键更新、满拒与删除左移
- 内置静态 uvm_factory 子集：深度 4 类型名注册表 +
  set_type_override/get_override/has_override/is_registered；
  重复与满槽注册 fail-closed，override 未设置时解析回原类型名
- uvm_factory 实例 override 子集：深度 4 `(instance, original,
  replacement)` 三元组；同实例同类型可更新，实例 override 优先于全局
  type override，删除后回退全局策略
- 内置 UVM-lite sequence/item/sequencer 流：uvm_sequence_item 携带 int
  payload；uvm_sequence 的 start 调 virtual body，start_item/finish_item
  写入共享 sequencer；uvm_sequencer 用内部 uvm_queue 缓存 item，
  send_item/get_next_item/peek_next_item 驱动 driver 侧取数
- 内置 uvm_seq_item_port + uvm_driver：driver.connect(sqr) 绑定
  sequencer，port 的 get_next_item/item_done 完成 peek→commit 握手并
  维护 gets/dones/transfers 计数
- 内置 UVM-lite callback：uvm_callback 提供 virtual callback(event,data)
  与共享 event/data/scale/hit 观察字段；uvm_callback_pool 用四个显式槽
  add0..add3 绑定回调，call 对全部槽虚分派并统计调用次数
  enable0..3/disable0..3 可过滤槽，call_filtered 只执行启用槽并记录
  skipped_callbacks，禁用不解除绑定
- 类作用域 static function 支持：`Class.method(...)` 可不经对象句柄调用
  void / 值返回方法，并共享访问 `Class.static_property`
- function/task 形参支持尾部默认值：类构造器、类方法与 module 内联
  function 均可省略带默认值的尾部实参，也可显式覆盖
- class 句柄支持 `null`：可显式清空顶层/嵌套句柄，与 `== null` /
  `!= null` 比较；清空别名组代表时复制并提升另一成员，保持旧对象可用
- class 对象身份相等：每次构造分配唯一 object id；`h == q` / `h != q`
  同时检查双方非 null 与对象 id，别名组共享同一身份，值相同的不同对象不等
- class 句柄 `$cast`：支持 `status = $cast(dst, src)` 与 void 调用；
  按当前别名代表的已知继承链判断上/下行兼容，成功时句柄引用绑定，
  失败返回 0 且目标保持未绑定
- class 浅拷贝构造：`dst = new source;`（无括号）创建新的对象身份并
  复制顶层值字段；嵌套 class 句柄保持引用共享，可独立修改顶层字段
- 参数化 class 默认值子集：`class C #(parameter W = 8, ...)` 会在收集期
  展开默认 specialization；属性范围、构造器/方法范围、默认实参与方法体
  均支持参数常量替换，派生类可继承基类默认 specialization
- 参数化 class 显式覆盖：支持 `C #(.W(4), ...)` 命名实参与 `C #(4)`
  位置整数实参；未覆盖参数继续使用默认值，并生成独立对象布局
- 参数化继承：`class D extends B #(...)` 可显式选择基类
  specialization；派生构造器经 `super.new` 传递宽度匹配实参
- 派生类参数可转发到基类：`class D #(W=3) extends B #(W)` 会按
  D 的默认值或显式覆盖生成对应 B specialization，并沿继承链展开
  属性/方法宽度与构造器实参
- string 类型 + `uvm_info` 风格消息；基类句柄多态
- 综合 → PnR → STA → 功耗 → DFT → LVS/DRC → signoff 报告
