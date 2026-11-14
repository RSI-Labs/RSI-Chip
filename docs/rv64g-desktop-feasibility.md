# RV64G 桌面级 CPU：可行性、差距与分阶段计划

日期：2026-11-08
状态：**P0 用户级 RV64G ISS、P1 RV64I RTL、P2 的 RTL M/A 扩展已完成；P3 的 FP 寄存器堆与非算术 F 子集已进入 RTL，浮点算术/P4-P6 未完成。完整商用桌面 RV64G 目标仍无法在当前环境完成。**

## 1. 结论

当前仓库只有 RV32I 内核（`platform/rtl/rv32i_full_core.sv`，约 3.7 万 mapped gates）
和 RV32I 工具/验证链。没有 RV64、S/U 特权级、MMU、缓存、总线、DDR/PCIe、
中断控制器或桌面平台 SoC。仓库也没有任何 foundry 标准单元库/PDK 文件。

“商用级别”不仅是 ISA 译码：它意味着流水线/乱序、分支预测、L1/L2 缓存、
缓存一致性、Sv39/Sv48 MMU、M/S/U 特权级、定时器/中断、DDR/PCIe 控制器、
DFT/ATPG、形式验证、门级/SDF 回归、物理实现、signoff、流片与硅后验证。
这是数十人年到数百人年的工程，需要商业 EDA、认证 PDK、验证 IP 和流片资源。

因此本轮不能诚实地声称目标完成；也不会用简化内核冒充商用 RV64G。

## 2. 现有资产（可复用）

| 资产 | 证据 |
|---|---|
| RV32I 多周期核 | `platform/rtl/rv32i_full_core.sv`，mapped ~37k gates |
| RV32I 指令矩阵 / 门级 / SDF | `rv32i_instruction_matrix` 7/7；`netlist_to_design_with_timing` |
| MiniOS-RV32I + 用户 asm | `platform/os/rv32i`，RTL/golden console+PC+寄存器一致 |
| 事件驱动仿真器 | `eda-sim`，VCD、SDF、断言、MMIO 平台运行器 |
| LEC | RTL-vs-mapped、双前端、strict 无 cut、Kissat/Z3/CVC5 |
| 现有最大 LEC 规模 | i8086 全核 93,773 mapped gates，strict 457.9s |
| PDK signoff 框架 | 合成 `lib1` + 可移植 manifest；无 foundry 库 |

## 3. RV64G 桌面级需求分解

### 3.1 ISA

- RV64I：64 位 GPR/PC、LD/SD、LWU、ADDIW/ADDW/SUBW/SLLW/SRLW/SRAW、
  6 位 shift、64 位地址/立即数
- M：MUL/MULH/MULHSU/MULHU/DIV/DIVU/REM/REMU + W 形式
- A：LR/SC、AMOADD/SWAP/XOR/AND/OR/MIN/MAX（32/64，signed/unsigned）
- F/D：浮点寄存器堆、FADD/FSUB/FMUL/FDIV/FSQRT/FMA、比较、符号注入、
  min/max、分类、转换、舍入模式、NaN boxing、fflags/frm/fcsr
- Zicsr/Zifencei；RV64 64 位 CSR 语义
- 特权级：M/S/U、trap delegation、mstatus/sstatus、counteren
- MMU：Sv39（桌面级至少 Sv39；服务器级 Sv48/Sv57）
- 中断：CLINT/PLIC、定时器、外部中断、WFI

### 3.2 微架构/桌面平台

- 流水线（至少 5 级），分支预测，BTB/BHT，返回栈
- I/D L1 缓存、L2、缓存一致性（多核）
- 总线/NoC、DDR 控制器、PCIe、UART、SPI、I2C、GPU/显示接口
- 功耗管理、时钟/复位、JTAG、DFT/scan/ATPG
- 启动 ROM / bootloader / UEFI 或 Linux boot

### 3.3 验证与签核

- ISA 合规套件（riscv-arch-test / riscv-tests）
- 随机指令差分、特权级/MMU/缓存一致性验证
- 形式验证（LEC、属性证明、等价检查）
- 门级 + SDF、功耗/IR/EM、DFT、DRC/LVS、封装、可靠性、硅后
- 商业 LEC/STA/DRC/LVS 独立对照
- 认证 PDK、标准单元库、SRAM/IO IP

## 4. 差距（可核查）

| 项目 | 当前 | 目标 |
|---|---|---|
| RTL ISA | RV32I | RV64G + 特权/MMU |
| 数据通路 | 32 位 | 64 位 + FPU |
| 特权级 | M only | M/S/U |
| MMU | 无 | Sv39+ |
| 缓存 | 无 | L1/L2 + 一致性 |
| 多核 | 无 | SMP |
| 桌面 I/O | MMIO console | DDR/PCIe/显示/中断控制器 |
| 规模 | 3.7 万 mapped gates | 数百万门 |
| LEC 规模 | ~9.4 万 gates 实测 | 数百万门 + 商业工具 |
| PDK | 合成 lib1 | 认证 foundry PDK |
| 流片 | 无 | 硅后 bring-up |

## 5. 分阶段计划（可在多轮内推进，但每阶段都有明确 exit criteria）

### P0：规范与参考模型（可立即开始）
- 冻结 RV64G 用户级 ISA 子集与特权/MMU 范围
- 写 RV64G ISS（Rust）作为 golden model：完整 IMAFD + Zicsr
- riscv-tests/arch-test 适配；随机指令差分
- Exit：RV64G 用户级 ELF 程序在同一条 ISS 上跑通，与 QEMU/Spike 对照

### P1：RV64I RTL
- 64 位 GPR/PC/ALU、LD/SD、W 形式、CSR/trap
- 事件驱动平台 + MMIO console
- LEC：RTL-vs-mapped + 双前端
- Exit：RV64I 指令矩阵、门级/SDF、LEC 全通过

### P2：M/A
- 64 位乘除、LR/SC、AMO
- 差分随机测试 + LEC
- Exit：M/A 指令矩阵通过

### P3：F/D
- IEEE 754 单/双精度 FPU（加、乘、除、sqrt、FMA、转换、标志）
- NaN boxing、舍入模式、异常标志
- 与 ISS/QEMU 随机差分；LEC
- Exit：RV64G 用户级 ISA 全通过

### P4：特权级 + MMU
- M/S/U、Sv39、页错误、TLB、trap delegation、定时器/中断
- 跑小型 OS（RV64 Linux 或定制 unikernel）
- Exit：特权/MMU/中断测试通过

### P5：桌面 SoC
- 流水线、L1/L2、总线、DDR、PCIe、UART、PLIC/CLINT
- 多核一致性
- Exit：启动 Linux/桌面级 OS，跑应用程序

### P6：商用签核
- DFT/ATPG、功耗/IR/EM、DRC/LVS、封装、可靠性
- 认证 PDK、商业 EDA 对照、流片/硅后
- Exit：tapeout signoff + 硅后 bring-up

## 6. 外部阻塞（需要用户/组织提供）

1. **范围决策**：目标工艺节点、频率、功耗、核数、缓存/MMU 级别、是否需要 Linux/PCIe/显示。
2. **认证 PDK 与标准单元/ SRAM/IO IP**：当前仓库只有合成 `lib1`。
3. **商业 EDA license**：Formality/Conformal、PrimeTime/Tempus、Calibre/StarRC、VCS/Xcelium 等。
4. **验证 IP/合规套件**：riscv-arch-test、特权级/MMU 验证、商业 VIP。
5. **团队与时间**：商用桌面 CPU 是多年、多团队项目，无法在单轮会话完成。
6. **流片/封装/硅后资源**。

## 7. 进展（2026-11-08 更新）

- **P0 已完成**：RV64I/M/A/F/D + Zicsr 用户级 ISS，6 组 LLVM 汇编测试
  （I/M/A/F/D/CSR）全部通过。详见 `docs/rv64g-p0-p1.md`。
- **P1 已完成**：`platform/rtl/rv64i_full_core.sv` 通过 RTL-vs-mapped 全电路
  LEC、独立 slang-vs-hdl LEC、strict 无 cut LEC；同一批镜像在 RTL 与 ISS 上
  寄存器逐一相同。
- **P2 已完成（RTL M/A）**：RV64M（MUL/MULH/MULHSU/MULHU/DIV/DIVU/REM/REMU
  及 W 形式）用 64 位迭代乘/除法器实现；RV64A 的 LR/SC、全部 AMO
  （AMOADD/SWAP/XOR/AND/OR/MIN/MAX/MINU/MAXU，.W/.D）已进入 RTL。
  `rv64m.bin` 和 `rv64a.bin` 在 RTL 与 ISS 上寄存器逐一相同。
- **P3 部分完成**：RV64F 的 32×64 位浮点寄存器堆、FMV.W.X/FMV.X.W、
  FSGNJ/N/X、FMIN/FMAX、FEQ/FLT/FLE、FCLASS 已进入 RTL，`rv64f_ops.bin`
  在 RTL 与 ISS 上寄存器一致。FADD/FSUB/FMUL/FDIV/FSQRT/FMA、浮点
  加载/存储、转换与异常标志仍未进入 RTL。
- **未完成**：RTL 的浮点算术、D、S/U 特权级、MMU、缓存、桌面 SoC、
  商用签核。
- **仍不能声称“商用级别 RV64G 桌面级 CPU”已完成。**
- 下一步：P2（M/A RTL）→ P3（F/D RTL）→ P4（特权级/MMU）→ P5（桌面 SoC）
  → P6（商用签核）。这些阶段需要认证 PDK、商业 EDA、验证 IP 和流片资源。
