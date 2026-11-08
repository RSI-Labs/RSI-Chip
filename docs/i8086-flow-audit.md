# i8086 流程完成度审计

审计对象：`platform/rtl/i8086_full_core.sv` + `i8086_prefix_decoder.sv` +
`i8086_opcode_decoder.sv` + `i8086_modrm_decoder.sv`（以及 `cpu8086/rtl` 子集核）。
审计目标：设计、仿真、网表、LEC、DFT、物理实现、foundry signoff 全流程是否完成。

## 结论

| 阶段 | 状态 | 说明 |
|---|---|---|
| RTL 设计（real-mode 子集） | **部分完成** | 覆盖面广，但不是完整 8086 ISA/总线/异常实现；源码头注释仍写 “under active construction” |
| RTL 仿真 | **通过（已覆盖范围）** | 29 个顶层差分矩阵 + 32 随机种子 + 8 个程序 fixture（i8086 COM/MZ 与 RV32I ELF） |
| 门级 + SDF 仿真 | **通过（已覆盖范围）** | 同一批 29 个程序在 SDF 反标后的 `netlist_to_design_with_timing` 上跑通 |
| 网表生成 | **通过** | 93,773 mapped cells / 544 flops；LEF/DEF/SDF/GDSII/OASIS 均生成 |
| LEC | **通过（自研 + 双前端）** | RTL-vs-netlist `equivalent=true`；独立 slang vs eda-hdl `equivalent=true` |
| DFT / ATPG | **部分完成** | 全核 544-flop 扫描链；ATPG 只在 opcode decoder 上，覆盖率 65.28% |
| 可移植 PDK gate | **通过（有警告）** | `i8086_pdk_signoff_mmmc_pass`，fast/slow 两 corner，0 DRC error / 72,912 warnings per corner |
| Foundry signoff | **未完成** | 无认证 PDK / 标准单元库 / 规则 deck / 版图提取 LVS / SPEF 后仿 STA；GDS 为符号化几何 |
| 商业工具对照 | **未执行** | 本机无 Formality/Conformal/Tempus/Calibre/StarRC |

**总体判定：已实现 real-mode 子集范围内的设计、仿真、网表、LEC 和仓库内可移植
signoff gate 已完成并通过；完整 8086 ISA/异常/总线覆盖和真正的 foundry signoff
（可交付流片的 signoff）没有完成。**

## 1. RTL 设计

已实现（有测试证据）的 real-mode 指令族：

- 段寄存器/段覆盖、默认 DS/SS 选择
- 主 ALU：ADD/OR/ADC/SBB/AND/SUB/XOR/CMP，byte/word，r/m 双向
- ModRM：mod 00/01/10/11、全部 base/index 组合、直接寻址、8/16 位位移
- 立即数组 0x80/0x81/0x83
- MOV 全部寄存器/内存/累加器形式，MOV sreg
- 变量/立即移位旋转（含 count 0/1/7/8/9/15/16/17/30/31）
- 一元组 TEST/NOT/NEG/MUL/IMUL/DIV/IDIV
- 标志控制 CMC/CLC/STC/CLI/STI/CLD/STD/LAHF/SAHF
- 栈与控制流：PUSH/POP（GPR/sreg）、PUSHF/POPF、CALL/RET、间接 far CALL、RETF imm16、全部短条件跳转、JCXZ、LOOP
- 字符串与 REP/REPNE：MOVS/CMPS/STOS/LODS/SCAS，DF=0/1，CX=0
- BCD/ASCII：DAA/DAS/AAA/AAS/AAM/AAD，AAM 任意非零基数
- I/O：立即数/DX 索引 byte/word IN/OUT
- 中断：INT/INT3/INTO、异步 IRQ、NMI、IF 屏蔽、IRET
- 20 位物理地址、1 MiB 空间
- MZ/COM loader 在 host golden 与 SV 核上都可运行

证据：

```text
platform/README.md: 8086 架构说明
docs/i8086-independent-golden-validation.md: 覆盖矩阵与差分方法
crates/eda-cpu-platform/tests/i8086_opcode_matrix.rs: 29 个矩阵测试
```

未完成/未声明完成的部分（源码与文档原话）：

- `platform/rtl/i8086_full_core.sv:1` — `// Full real-mode 8086 core under active construction.`
- `platform/README.md` Remaining acceptance gates 1–6：
  完整 8086 opcode/ModRM/prefix/exception 组合、更多随机流、独立 RTL/golden 对、
  编译程序语料、ELF relocation/DOS 文件 I/O/异常。
- `docs/i8086-independent-golden-validation.md`：明确 “does not claim a complete
  8086 instruction set, interrupt controller, peripheral, DOS/BIOS, or ELF/MZ
  loader oracle”，并且“current tests compare retired instruction count, not cycles”。
- `#DE`/非法指令只置 `trap` 输出位，没有像真实 8086 那样压栈 FLAGS/CS/IP 并跳转中断向量；
  矩阵没有正向的 divide-error / illegal-opcode 测试。
- 抽象总线接口（`mem_rdata/mem_addr/mem_wdata/mem_we/mem_width`），不是 8086 引脚级时序；
  没有 BIU 预取队列、ALE/RD/WR/READY/HOLD、最小/最大模式、8087 协处理。
- 无周期级时序模型或周期断言。

### 1.1 已知设计简化/偏差（影响“完整 8086”判定）

| 项 | 当前实现 | 真实 8086 行为 |
|---|---|---|
| `#DE`/非法指令 | 置 `trap` 输出位后停止（`trap_r <= 1`） | 压栈 FLAGS/CS/IP，跳转中断向量 0/6 |
| NMI | 电平采样（`if (nmi || (irq && flags[9]))`） | 边沿触发并锁存 |
| LOCK 前缀 | 仅译码，`fetch_lock` 未参与执行 | 总线锁定语义 |
| ESC/协处理器 | `0xD8..0xDF` 视为空操作（`next_ax = ax`） | 8087 协处理器总线周期 |
| 总线接口 | 抽象 `mem_*`/`io_*` 接口，无引脚时序 | ALE/RD/WR/READY/HOLD/QS 等 |
| BIU | 单周期取指状态机 | 6 字节预取队列 + EU/BIU 并行 |
| 时序 | 只比较 retired 指令数 | 周期级时序 |
| 异步输入 | IRQ/NMI 直接进时钟域，无同步器 | 需 CDC/同步与建立保持约束 |

这些点不影响仓库内差分矩阵的通过，但意味着该核是**功能级 real-mode 子集**，
不是可 pin/cycle 兼容或完整异常语义的 8086。

## 2. 仿真

### 2.1 RTL 差分矩阵

`cargo test --release -p eda-cpu-platform --test i8086_opcode_matrix -- --test-threads=1`

```text
running 29 tests
...（29 个矩阵/边界测试）
test result: ok. 29 passed; 0 failed; 0 ignored; finished in 1914.08s
```

每个矩阵用例比较：

- 仓库内 golden `I8086Machine`
- 独立 `ReferenceI8086`（不同内存表示/译码/ALU/移位实现）
- SystemVerilog RTL
- SDF 反标后的门级 Design

比较内容：8 个通用寄存器、4 个段寄存器、IP、FLAGS、retired 指令数、
1 MiB 全地址空间字节（独立 reference 逐字节比较），以及门级 setup/hold slack。

### 2.2 随机差分 fuzz

32 个确定性种子，每个 40 条混合指令；比较寄存器、IP、FLAGS、halt/trap 和 768 字节内存窗口。
证据：`deterministic_i8086_randomized_differential_fuzz_seeds_000_007` … `024_031` 全部通过。

### 2.3 程序 fixture

`cargo test --release -p eda-cpu-platform --test program_fixtures`

```text
test result: ok. 8 passed; 0 failed
```

包括 `hello.com`、`high_memory.com`、最大 64 KiB COM、可重定位 MZ、RV32I ELF 等，
全部同时跑 golden 与 SystemVerilog 核。

### 2.4 cpu8086 子集核

`cpu8086/rtl` 是 16 位定长指令的教学子集核，24 个 `cpu8086_*` 用例覆盖
ALU 全标志、4 个整机程序、VCD 波形导出，以及默认流程（综合/布线/LEC/DRC）。

### 2.5 未覆盖

- 没有对 i8086 核的 SVA/断言、BMC、k-induction 属性证明。
- 事件驱动门级仿真在 9 万 cell 上不可用于全矩阵（单程序数分钟到超时）；
  全矩阵使用 levelized Design 求值器。
- 没有并发/多线程、总线竞争、DMA、外设模型。
- 无周期级 timing 对比。

## 3. 网表与 LEC

### 3.1 网表规模

```text
RTL-vs-netlist LEC probe:
equivalent=true gold_cells=251734 mapped_cells=93773
drc_errors=0 delay_ns=389.65000000000174
```

- generic/RTL reference：251,734 cells
- standard-cell mapped：93,773 cells
- 时序单元：544 flops

### 3.2 LEC

- RTL-vs-mapped：`equivalent=true`（上面的 probe）
- 独立前端：slang vs eda-hdl full-core LEC
  `cargo test --release -p eda-tests --test i8086_independent_lec`
  → `1 passed`，251.70s
- 仓库内 LEC 引擎支持：RTL-vs-mapped、双源码 RTL、RTL vs 外部 netlist、
  Design-to-Design、反例输出、CLI `eda lec`。

### 3.3 物理数据库

每个 signoff corner 都生成：

```text
LEF + DEF + GDSII + OASIS + SDF
fast: gds=57,179,836 bytes oas=10,607,733 bytes
slow: gds=57,179,836 bytes oas=10,607,733 bytes
```

**限制**：GDS/OASIS 由 DEF 的元件矩形和布线矩形生成（`def_shapes` 使用固定
`SITE_DBU*4 × ROW_DBU` 的元件框），不是真实标准单元版图；LEF 也是合成的
`SIZE width BY 10.00`。manifest 只有单层 `met1` 布线、没有电源网格/UPF，
因此这些数据库是可移植交换格式，不是可交付 foundry 的 tapeout 数据。

## 4. DFT / ATPG

`cargo test --release -p eda-tests --test i8086_dft_atpg -- --nocapture`

```text
i8086 scan: chain=544 flops, sequential=544 scan_cells=544 chains=1
i8086 opcode-decoder ATPG: faults=1950 detected=1273 coverage=65.28%
test result: ok. 1 passed
```

已完成：全核 544 flop 扫描链插入、结构检查、0 unscanned flop、STIL 往返。

未完成：

- ATPG 只跑在 `i8086_opcode_decoder` 上，不是 full core；65.28% 覆盖率不满足量产目标。
- 没有 full-core transition delay / path delay ATPG。
- 没有 scan compression、MBIST、boundary scan、JTAG。
- 没有 tester 程序/向量格式交付。

## 5. Foundry signoff

仓库内的 “PDK signoff” gate 全部通过：

```text
PDK signoff: pdk=i8086-signoff-pdk version=1.0 design=i8086_full_core
Layout rule deck: not-supplied
Model correlation nmos.VTO: error=0.004444% tolerance=0.010000% PASS
fast: process=FF V=1.900 T=-40.0 scale=0.800 library=lib1-x0.800
      timing=311.720000 ns equivalent=true DRC=0 errors/72912 warnings
      physical-db=PASS gds=57179836 oas=10607733
slow: process=SS V=1.600 T=125.0 scale=1.250 library=lib1-x1.250
      timing=487.062500 ns equivalent=true DRC=0 errors/72912 warnings
      physical-db=PASS gds=57179836 oas=10607733
Check model-correlation: PASS (1/1)
Check equivalence: PASS
Check timing: PASS (2/2 corner timing results finite; worst 487.062500 ns)
Check drc: PASS (0 DRC errors, 145824 warnings)
Check physical-database: PASS
Overall signoff: PASS
```

但这是**可移植的仓库内 gate**，不是 foundry signoff。逐项差距：

| Foundry signoff 要求 | 当前状态 | 证据/缺口 |
|---|---|---|
| 认证 PDK（工艺厂发布） | **缺失** | 仓库只有 inline manifest `i8086-signoff-pdk`；`find` 无任何 `.lib/.lef/.gds/.tf/.itf/.cdl` |
| 标准单元库 | **缺失** | 使用 `CellLibrary::std()`（`lib1`，合成的 AND/OR/DFF 等），corner 用 `delay_scale` 缩放；报告 `imported-liberty=false` |
| 工艺规则 deck（DRC/LVS/ERC/DFM） | **缺失** | 报告 `Layout rule deck: not-supplied`；`eda capabilities` 明确 “a complete foundry rule deck is missing” |
| 版图提取 LVS | **未执行** | `eda-lvs` 只做 netlist-vs-netlist 结构比较，无 GDS 提取；signoff gate 不含 LVS |
| 寄生参数提取 + 后仿 STA | **未执行** | signoff 用 `eda_sta::analyze` 在布局前网表上跑，无 SDC clock/IO/uncertainty/OCV/derate，无 SPEF/RC |
| Antenna 签核 | **未纳入 gate** | 有 `repair_rtl_antennas`/`DrcOptions`，但 i8086 signoff manifest 没有 rule deck/antenna ratio |
| EM/IR/电源签核 | **未执行** | `eda-ir`/`eda-power` 工具存在；i8086 没有 power grid/IR/EM gate |
| UPF/低功耗 | **未执行** | 无 i8086 UPF/多电源域 |
| 版图 DRC（foundry deck） | **未执行** | 只有 router DRC，0 error / 72,912 warnings（每 corner） |
| 密度/DFM/OPC/掩模数据 | **缺失** | 无 density/DFM/OPC/fracture |
| Tapeout GDS 审核 | **缺失** | GDS 为符号化元件/布线矩形 |
| 封装/可靠性（ESD/HTOL/EM lifetime） | **缺失** | 无封装模型、无 IC 级可靠性签核 |
| Silicon bring-up/ATE | **缺失** | 无硅后测试 |
| 商业工具对照 | **未执行** | 本机无 Formality/Conformal/Tempus/PrimeTime/Calibre/StarRC license |

仓库自身的能力报告也给出同样判定：

```text
$ eda capabilities
signoff foundry_calibrated_pdk_signoff partial
        manifest is portable but not certified by an external foundry;
        Liberty/rule-deck SHA-256 detects accidental corruption but does not
        establish foundry provenance, and advanced compact-model correlation
        is not vendor-calibrated
physical placement_routing_drc_def_lef implemented
        ... a complete foundry rule deck is missing
summary: implemented=24, partial=5, missing=0
```

## 6. 结论与外部阻塞

**已完成并可复现**：

1. RTL 功能设计（real-mode 子集）与 29 组差分矩阵仿真
2. 32 种子随机差分、8 个程序 fixture（含 5 个 i8086 COM/MZ）、24 个 cpu8086 子集用例
3. SDF 反标门级矩阵（同一 29 个程序，全架构状态 + setup/hold）
4. 综合/映射网表（93,773 cells / 544 flops）
5. RTL-vs-netlist LEC、独立 slang-vs-hdl LEC
6. 全核 544-flop 扫描链 + opcode decoder 65.28% ATPG
7. 可移植双 corner PDK gate（LEC/timing/DRC/LEF/DEF/GDSII/OASIS/SDF）

**未完成，且不能仅靠本仓库代码完成**：

1. 完整 8086 ISA/异常/总线时序覆盖（#DE、非法指令向量、周期级 timing、BIU/总线引脚）
2. 认证 foundry PDK、真实标准单元库（Liberty/LEF/GDS）、完整 DRC/LVS/ERC/DFM deck
3. GDS 提取 LVS、SPEF/RC 提取 + 后仿 STA（OCV/derate/SI）
4. EM/IR、antenna、UPF、DFT full-core ATPG、封装、可靠性、硅后 bring-up
5. 商业 LEC/STA/DRC/LVS 工具对照

这些需要外部输入：foundry PDK/库/规则 deck、封装与测试资源、商业 EDA license。
在拿到这些输入之前，不能声称 8086 已完成全部 foundry signoff 流程。

## 7. 复现命令与结果（本次审计执行）

```sh
# 1. RTL / 独立 reference / 门级 + SDF 全矩阵（29 项）
cargo test --release -p eda-cpu-platform --test i8086_opcode_matrix -- --test-threads=1
# → 29 passed; 0 failed; finished in 1914.08s（并行审计负载下）

# 2. 可移植 PDK signoff（fast + slow）
cargo test --release -p eda-tests --test i8086_pdk_signoff
# → 1 passed; finished in 1429.47s
# → Overall signoff: PASS

# 3. 独立前端 LEC（slang vs eda-hdl）
SLANG_BIN=/tmp/slang-build/bin/slang \
  cargo test --release -p eda-tests --test i8086_independent_lec
# → 1 passed; finished in 251.70s

# 4. DFT / ATPG
cargo test --release -p eda-tests --test i8086_dft_atpg -- --nocapture
# → i8086 scan: chain=544 flops, sequential=544 scan_cells=544 chains=1
# → i8086 opcode-decoder ATPG: faults=1950 detected=1273 coverage=65.28%
# → 1 passed; finished in 43.25s

# 5. 程序 fixture 与 MZ 运行
cargo test --release -p eda-cpu-platform --test program_fixtures
# → 8 passed
cargo test --release -p eda-cpu-platform --test i8086_run
# → 1 passed

# 6. RTL-vs-netlist LEC + 物理流程
cargo run --release -p eda-tests --example i8086_rtl_lec_probe
# → equivalent=true gold_cells=251734 mapped_cells=93773
#   drc_errors=0 delay_ns=389.65000000000174

# 7. 全量 suite 中所有名字含 8086 的用例
SUITE_FILTER=8086 cargo test --release -p eda-tests --test suite full_suite_all_green
# → 1 passed; 0 failed; finished in 1210.15s
```

审计报告生成时间：2026-11-07。
