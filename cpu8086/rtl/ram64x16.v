// ---------------------------------------------------------------------------
// cpu8086 — 64 字 x 16 位静态 RAM
// 受限 HDL 子集不支持存储器数组与动态索引，因此用 case 多路器实现
// 读端口与写端口 — 这正是综合后寄存器堆的真实门级结构。
// prog_ld 为高时，一拍并行载入 1024 位程序映像（复位引导）。
// ---------------------------------------------------------------------------
module ram64x16(
    input  wire         clk,
    input  wire         ld,
    input  wire [1023:0] prog,
    input  wire [5:0]   addr,
    input  wire         we,
    input  wire [15:0]  wd,
    output reg  [15:0]  rd
);
    reg [15:0] m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, m10, m11, m12, m13, m14, m15, m16, m17, m18, m19, m20, m21, m22, m23, m24, m25, m26, m27, m28, m29, m30, m31, m32, m33, m34, m35, m36, m37, m38, m39, m40, m41, m42, m43, m44, m45, m46, m47, m48, m49, m50, m51, m52, m53, m54, m55, m56, m57, m58, m59, m60, m61, m62, m63;

    always @(posedge clk) begin
        if (ld) begin
            m0 <= prog[15:0];
            m1 <= prog[31:16];
            m2 <= prog[47:32];
            m3 <= prog[63:48];
            m4 <= prog[79:64];
            m5 <= prog[95:80];
            m6 <= prog[111:96];
            m7 <= prog[127:112];
            m8 <= prog[143:128];
            m9 <= prog[159:144];
            m10 <= prog[175:160];
            m11 <= prog[191:176];
            m12 <= prog[207:192];
            m13 <= prog[223:208];
            m14 <= prog[239:224];
            m15 <= prog[255:240];
            m16 <= prog[271:256];
            m17 <= prog[287:272];
            m18 <= prog[303:288];
            m19 <= prog[319:304];
            m20 <= prog[335:320];
            m21 <= prog[351:336];
            m22 <= prog[367:352];
            m23 <= prog[383:368];
            m24 <= prog[399:384];
            m25 <= prog[415:400];
            m26 <= prog[431:416];
            m27 <= prog[447:432];
            m28 <= prog[463:448];
            m29 <= prog[479:464];
            m30 <= prog[495:480];
            m31 <= prog[511:496];
            m32 <= prog[527:512];
            m33 <= prog[543:528];
            m34 <= prog[559:544];
            m35 <= prog[575:560];
            m36 <= prog[591:576];
            m37 <= prog[607:592];
            m38 <= prog[623:608];
            m39 <= prog[639:624];
            m40 <= prog[655:640];
            m41 <= prog[671:656];
            m42 <= prog[687:672];
            m43 <= prog[703:688];
            m44 <= prog[719:704];
            m45 <= prog[735:720];
            m46 <= prog[751:736];
            m47 <= prog[767:752];
            m48 <= prog[783:768];
            m49 <= prog[799:784];
            m50 <= prog[815:800];
            m51 <= prog[831:816];
            m52 <= prog[847:832];
            m53 <= prog[863:848];
            m54 <= prog[879:864];
            m55 <= prog[895:880];
            m56 <= prog[911:896];
            m57 <= prog[927:912];
            m58 <= prog[943:928];
            m59 <= prog[959:944];
            m60 <= prog[975:960];
            m61 <= prog[991:976];
            m62 <= prog[1007:992];
            m63 <= prog[1023:1008];
        end else if (we) begin
            case (addr)
                6'd0: m0 <= wd;
                6'd1: m1 <= wd;
                6'd2: m2 <= wd;
                6'd3: m3 <= wd;
                6'd4: m4 <= wd;
                6'd5: m5 <= wd;
                6'd6: m6 <= wd;
                6'd7: m7 <= wd;
                6'd8: m8 <= wd;
                6'd9: m9 <= wd;
                6'd10: m10 <= wd;
                6'd11: m11 <= wd;
                6'd12: m12 <= wd;
                6'd13: m13 <= wd;
                6'd14: m14 <= wd;
                6'd15: m15 <= wd;
                6'd16: m16 <= wd;
                6'd17: m17 <= wd;
                6'd18: m18 <= wd;
                6'd19: m19 <= wd;
                6'd20: m20 <= wd;
                6'd21: m21 <= wd;
                6'd22: m22 <= wd;
                6'd23: m23 <= wd;
                6'd24: m24 <= wd;
                6'd25: m25 <= wd;
                6'd26: m26 <= wd;
                6'd27: m27 <= wd;
                6'd28: m28 <= wd;
                6'd29: m29 <= wd;
                6'd30: m30 <= wd;
                6'd31: m31 <= wd;
                6'd32: m32 <= wd;
                6'd33: m33 <= wd;
                6'd34: m34 <= wd;
                6'd35: m35 <= wd;
                6'd36: m36 <= wd;
                6'd37: m37 <= wd;
                6'd38: m38 <= wd;
                6'd39: m39 <= wd;
                6'd40: m40 <= wd;
                6'd41: m41 <= wd;
                6'd42: m42 <= wd;
                6'd43: m43 <= wd;
                6'd44: m44 <= wd;
                6'd45: m45 <= wd;
                6'd46: m46 <= wd;
                6'd47: m47 <= wd;
                6'd48: m48 <= wd;
                6'd49: m49 <= wd;
                6'd50: m50 <= wd;
                6'd51: m51 <= wd;
                6'd52: m52 <= wd;
                6'd53: m53 <= wd;
                6'd54: m54 <= wd;
                6'd55: m55 <= wd;
                6'd56: m56 <= wd;
                6'd57: m57 <= wd;
                6'd58: m58 <= wd;
                6'd59: m59 <= wd;
                6'd60: m60 <= wd;
                6'd61: m61 <= wd;
                6'd62: m62 <= wd;
                6'd63: m63 <= wd;
            endcase
        end
    end

    always @(*) begin
        case (addr)
                6'd0: rd = m0;
                6'd1: rd = m1;
                6'd2: rd = m2;
                6'd3: rd = m3;
                6'd4: rd = m4;
                6'd5: rd = m5;
                6'd6: rd = m6;
                6'd7: rd = m7;
                6'd8: rd = m8;
                6'd9: rd = m9;
                6'd10: rd = m10;
                6'd11: rd = m11;
                6'd12: rd = m12;
                6'd13: rd = m13;
                6'd14: rd = m14;
                6'd15: rd = m15;
                6'd16: rd = m16;
                6'd17: rd = m17;
                6'd18: rd = m18;
                6'd19: rd = m19;
                6'd20: rd = m20;
                6'd21: rd = m21;
                6'd22: rd = m22;
                6'd23: rd = m23;
                6'd24: rd = m24;
                6'd25: rd = m25;
                6'd26: rd = m26;
                6'd27: rd = m27;
                6'd28: rd = m28;
                6'd29: rd = m29;
                6'd30: rd = m30;
                6'd31: rd = m31;
                6'd32: rd = m32;
                6'd33: rd = m33;
                6'd34: rd = m34;
                6'd35: rd = m35;
                6'd36: rd = m36;
                6'd37: rd = m37;
                6'd38: rd = m38;
                6'd39: rd = m39;
                6'd40: rd = m40;
                6'd41: rd = m41;
                6'd42: rd = m42;
                6'd43: rd = m43;
                6'd44: rd = m44;
                6'd45: rd = m45;
                6'd46: rd = m46;
                6'd47: rd = m47;
                6'd48: rd = m48;
                6'd49: rd = m49;
                6'd50: rd = m50;
                6'd51: rd = m51;
                6'd52: rd = m52;
                6'd53: rd = m53;
                6'd54: rd = m54;
                6'd55: rd = m55;
                6'd56: rd = m56;
                6'd57: rd = m57;
                6'd58: rd = m58;
                6'd59: rd = m59;
                6'd60: rd = m60;
                6'd61: rd = m61;
                6'd62: rd = m62;
                6'd63: rd = m63;
            default: rd = 16'h0000;
        endcase
    end
endmodule
