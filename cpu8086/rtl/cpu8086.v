// ---------------------------------------------------------------------------
// cpu8086 — 执行单元 + 简化总线接口（Phase 1 核）
//
// 三级取指-执行状态机:
//   S_FETCH  — 从 mem[ip] 取指令字到 IR，IP+1；若是双字指令转 S_FIMM
//   S_FIMM   — 取立即数/跳转目标到 IMM，IP+1
//   S_EXEC   - 译码执行: ALU / 寄存器写回 / 标志更新 / 跳转 / 访存 / IO
//   S_HALT   — 停机
//
// 指令格式（16 位定长，字寻址）:
//   [15:12] op | [11:9] rd | [8:6] rs | [5:0] funct
// 寄存器: 0=AX 1=CX 2=DX 3=BX 4=SP 5=BP 6=SI 7=DI（8086 EU 通用组）
// ---------------------------------------------------------------------------
module cpu8086(
    input  wire        clk,
    input  wire        rst,
    // 统一程序/数据存储器端口（字宽 16，64 字）
    output wire [5:0]  mem_addr,
    output wire        mem_we,
    output wire [15:0] mem_wd,
    input  wire [15:0] mem_rd,
    // I/O 输出端口
    output wire        io_wr,
    output wire [15:0] io_data,
    output wire        halted
);
    // ---- 状态编码 ----
    localparam S_FETCH = 3'd0;
    localparam S_FIMM  = 3'd1;
    localparam S_EXEC  = 3'd2;
    localparam S_HALT  = 3'd3;

    reg [2:0]  state;
    reg [5:0]  ip;      // 指令指针
    reg [15:0] ir;      // 指令寄存器
    reg [15:0] imm;     // 立即数/跳转目标
    // 标志位（8086 PSW 低 16 位中的 6 个状态标志）
    reg cf, pf, af, zf, sf, of;

    // ---- 指令字段（对 IR 译码） ----
    wire [3:0] op    = ir[15:12];
    wire [2:0] rd    = ir[11:9];
    wire [2:0] rs    = ir[8:6];
    wire [5:0] funct = ir[5:0];

    // ---- 取指时刻对存储器输出直接预译码（决定是否需要第二字） ----
    wire [3:0] f_op = mem_rd[15:12];
    wire need_imm_fetch = (f_op == 4'h1) || (f_op == 4'hB) || (f_op == 4'hC);

    // ---- 写回通道（先声明后使用） ----
    reg [15:0] reg_wd;
    reg        reg_we;

    // ---- 寄存器堆 ----
    wire [15:0] rd_val, rs_val;
    regfile16 u_regs(
        .clk(clk), .rst(rst),
        .rs1(rd), .q1(rd_val),
        .rs2(rs), .q2(rs_val),
        .wa(rd), .wd(reg_wd), .we(reg_we)
    );

    // ---- ALU ----
    // 操作数 a = rd 寄存器；b = 立即数 / 1(INC,DEC) / rs 寄存器
    wire [15:0] alu_b = (op == 4'h1) ? imm
                      : ((op == 4'h7) || (op == 4'h8)) ? 16'h0001
                      : rs_val;
    // 操作码映射到 ALU 内部选择信号
    reg [3:0] alu_sel;
    always @(*) begin
        case (op)
            4'h2: alu_sel = 4'h0;             // ADD
            4'h3: alu_sel = 4'h1;             // SUB
            4'h9: alu_sel = 4'h1;             // CMP
            4'h4: alu_sel = 4'h2;             // AND
            4'h5: alu_sel = 4'h3;             // OR
            4'h6: alu_sel = 4'h4;             // XOR
            4'hA: alu_sel = 4'h5 + funct[1:0]; // SHF: 5=SHL 6=SHR 7=ROL 8=ROR
            4'h7: alu_sel = 4'h0;             // INC
            4'h8: alu_sel = 4'h1;             // DEC
            default: alu_sel = 4'h0;
        endcase
    end
    wire [15:0] alu_y;
    wire alu_cf, alu_pf, alu_af, alu_zf, alu_sf, alu_of;
    alu16 u_alu(
        .a(rd_val), .b(alu_b), .sel(alu_sel),
        .y(alu_y),
        .cf(alu_cf), .pf(alu_pf), .af(alu_af),
        .zf(alu_zf), .sf(alu_sf), .of(alu_of)
    );

    // ---- 写回数据选择 ----
    always @(*) begin
        if (op == 4'h0) reg_wd = rs_val;          // MOV reg,reg
        else if (op == 4'h1) reg_wd = imm;         // MVI reg,imm
        else if ((op == 4'hF) && (funct[0] == 1'b0)) reg_wd = mem_rd; // LD
        else reg_wd = alu_y;                       // ALU 类
    end

    // ---- 写使能 ----
    always @(*) begin
        case (op)
            4'h0, 4'h1, 4'h2, 4'h3, 4'h4, 4'h5, 4'h6, 4'h7, 4'h8, 4'hA:
                reg_we = (state == S_EXEC);
            4'hF: reg_we = (state == S_EXEC) && (funct[0] == 1'b0); // 仅 LD
            default: reg_we = 1'b0;
        endcase
    end

    // ---- 标志更新（INC/DEC 保持 CF，x86 语义） ----
    wire flag_upd = (state == S_EXEC) &&
        ((op == 4'h2) || (op == 4'h3) || (op == 4'h4) || (op == 4'h5) ||
         (op == 4'h6) || (op == 4'h7) || (op == 4'h8) || (op == 4'h9) ||
         (op == 4'hA));

    // ---- 条件跳转判定 ----
    reg jcc_taken;
    always @(*) begin
        case (funct[2:0])
            3'd0: jcc_taken = zf;         // JZ / JE
            3'd1: jcc_taken = ~zf;        // JNZ / JNE
            3'd2: jcc_taken = cf;         // JC / JB
            3'd3: jcc_taken = ~cf;        // JNC / JAE
            3'd4: jcc_taken = sf;         // JS
            3'd5: jcc_taken = ~sf;        // JNS
            default: jcc_taken = 1'b0;
        endcase
    end

    // ---- 存储器地址/写通道 ----
    assign mem_addr = ((state == S_EXEC) && (op == 4'hF)) ? rs_val[5:0] : ip;
    assign mem_we   = (state == S_EXEC) && (op == 4'hF) && (funct[0] == 1'b1);
    assign mem_wd   = rd_val;

    // ---- I/O ----
    assign io_wr   = (state == S_EXEC) && (op == 4'hD);
    assign io_data = rd_val;
    assign halted  = (state == S_HALT);

    // ---- 主时序: 状态机 + 取指 + 执行 ----
    always @(posedge clk) begin
        if (rst) begin
            state <= S_FETCH;
            ip    <= 6'd0;
            ir    <= 16'h0000;
            imm   <= 16'h0000;
            cf <= 1'b0; pf <= 1'b0; af <= 1'b0;
            zf <= 1'b0; sf <= 1'b0; of <= 1'b0;
        end else begin
            case (state)
                S_FETCH: begin
                    ir <= mem_rd;
                    ip <= ip + 6'd1;
                    state <= need_imm_fetch ? S_FIMM : S_EXEC;
                end
                S_FIMM: begin
                    imm   <= mem_rd;
                    ip    <= ip + 6'd1;
                    state <= S_EXEC;
                end
                S_EXEC: begin
                    if (flag_upd) begin
                        pf <= alu_pf;
                        af <= alu_af;
                        zf <= alu_zf;
                        sf <= alu_sf;
                        of <= alu_of;
                        if ((op == 4'h7) || (op == 4'h8))
                            cf <= cf;      // INC/DEC 不影响 CF
                        else
                            cf <= alu_cf;
                    end
                    case (op)
                        4'hB: begin ip <= imm[5:0]; state <= S_FETCH; end // JMP
                        4'hC: begin                     // JCC
                            if (jcc_taken) ip <= imm[5:0];
                            state <= S_FETCH;
                        end
                        4'hE: state <= S_HALT;          // HLT
                        default: state <= S_FETCH;
                    endcase
                end
                S_HALT: state <= S_HALT;
                default: state <= S_FETCH;
            endcase
        end
    end
endmodule
