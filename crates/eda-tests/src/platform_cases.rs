//! SystemVerilog CPU-platform contract tests shared by RV32I and i8086.

use crate::CaseList;
use eda_core::logic::LogicWord as LW;
use eda_sim::Simulator;

const RV32I_DECODER: &str = include_str!("../../../platform/rtl/rv32i_isa_decoder.sv");
const I8086_PREFIX_DECODER: &str = include_str!("../../../platform/rtl/i8086_prefix_decoder.sv");
const I8086_OPCODE_DECODER: &str = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");
const I8086_MODRM_DECODER: &str = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
const RV32I_FULL_CORE: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const RV32I_FULL_SOC_16: &str = include_str!("../../../platform/rtl/rv32i_full_soc_16.sv");
const RV32I_FULL_TB_16: &str = include_str!("../../../platform/rtl/rv32i_full_tb_16.sv");
const I8086_FULL_CORE: &str = include_str!("../../../platform/rtl/i8086_full_core.sv");
const I8086_FULL_SOC_64: &str = include_str!("../../../platform/rtl/i8086_full_soc_64.sv");
const I8086_FULL_TB_64: &str = include_str!("../../../platform/rtl/i8086_full_tb_64.sv");

pub fn register(c: &mut CaseList) {
    c.add(
        "platform_rv32i_decoder_classifies_all_instruction_groups",
        || {
            let design = eda_hdl::compile(RV32I_DECODER, Some("rv32i_isa_decoder"))
                .map_err(|e| e.to_string())?;
            let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
            let vectors: &[(u32, bool, u64, &str)] = &[
                (0x00000093, true, 7, "ADDI"),
                (0x002081b3, true, 8, "ADD"),
                (0x402081b3, true, 8, "SUB"),
                (0x00208463, true, 4, "BEQ"),
                (0x000000ef, true, 2, "JAL"),
                (0xfe208fa3, true, 6, "SB"),
                (0x0ff0000f, true, 9, "FENCE"),
                (0x00000073, true, 10, "ECALL"),
                (0x00000000, false, 10, "illegal-zero"),
            ];
            for &(instruction, legal, kind, name) in vectors {
                sim.set_input("instruction", &LW::from_u64(32, u64::from(instruction)))?;
                let got_legal = sim
                    .read_port("legal")
                    .and_then(|value| value.to_u64())
                    .ok_or("missing legal output")?;
                let got_kind = sim
                    .read_port("kind")
                    .and_then(|value| value.to_u64())
                    .ok_or("missing kind output")?;
                if got_legal != u64::from(u8::from(legal)) || got_kind != kind {
                    return Err(format!(
                        "{name}: instruction={instruction:#010x} legal={got_legal} kind={got_kind}"
                    ));
                }
            }
            Ok(())
        },
    );

    c.add("platform_i8086_prefix_decoder_decodes_prefix_bytes", || {
        let design = eda_hdl::compile(I8086_PREFIX_DECODER, Some("i8086_prefix_decoder"))
            .map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        for byte in [0x26_u32, 0x2e, 0x36, 0x3e, 0xf0, 0xf2, 0xf3, 0x90] {
            sim.set_input("first_byte", &LW::from_u64(8, u64::from(byte)))?;
            let prefix = sim
                .read_port("is_prefix")
                .and_then(|value| value.to_u64())
                .ok_or("missing is_prefix")?;
            let expected = u64::from(u8::from(matches!(
                byte,
                0x26 | 0x2e | 0x36 | 0x3e | 0xf0 | 0xf2 | 0xf3
            )));
            if prefix != expected {
                return Err(format!(
                    "byte={byte:#04x} prefix={prefix} expected={expected}"
                ));
            }
        }
        Ok(())
    });

    c.add(
        "platform_i8086_opcode_decoder_covers_primary_classes",
        || {
            let design = eda_hdl::compile(I8086_OPCODE_DECODER, Some("i8086_opcode_decoder"))
                .map_err(|e| e.to_string())?;
            let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
            let vectors: &[(u32, bool, u64)] = &[
                (0x00, true, 1),
                (0x06, true, 2),
                (0x40, true, 3),
                (0x58, true, 4),
                (0x74, true, 5),
                (0x80, true, 6),
                (0x88, true, 7),
                (0x8d, true, 8),
                (0x98, true, 9),
                (0xa4, true, 10),
                (0xb0, true, 12),
                (0xc3, true, 13),
                (0xcd, true, 14),
                (0xd1, true, 15),
                (0xd7, true, 16),
                (0xd8, true, 17),
                (0xe2, true, 18),
                (0xe9, true, 19),
                (0xe4, true, 20),
                (0xf4, true, 21),
                (0xf6, true, 22),
                (0xff, true, 23),
                (0x60, false, 0),
            ];
            for &(opcode, valid, class) in vectors {
                sim.set_input("opcode", &LW::from_u64(8, u64::from(opcode)))?;
                sim.set_input("modrm", &LW::from_u64(8, 0xc0))?;
                let got_valid = sim
                    .read_port("valid")
                    .and_then(|value| value.to_u64())
                    .ok_or("missing valid output")?;
                let got_class = sim
                    .read_port("opcode_class")
                    .and_then(|value| value.to_u64())
                    .ok_or("missing opcode_class output")?;
                if got_valid != u64::from(u8::from(valid)) || got_class != class {
                    return Err(format!(
                        "opcode={opcode:#04x} valid={got_valid} class={got_class} expected={class}"
                    ));
                }
            }
            Ok(())
        },
    );

    c.add("platform_i8086_modrm_decoder_computes_segmented_addresses", || {
        let design =
            eda_hdl::compile(I8086_MODRM_DECODER, Some("i8086_modrm_decoder"))
                .map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        let vectors: &[(u8, u16, bool, bool, u16, u8)] = &[
            (0x00, 0x1234, false, true, 0x0140, 3),
            (0x02, 0x1234, false, true, 0x0090, 2),
            (0x06, 0x1234, false, true, 0x1234, 3),
            (0x46, 0x0005, false, true, 0x0055, 2),
            (0x86, 0x0100, false, true, 0x0150, 2),
            (0xc0, 0x1234, false, false, 0x0000, 3),
            (0x00, 0x1234, true, true, 0x0140, 0),
        ];
        for &(modrm, displacement, override_present, memory, offset, segment) in vectors {
            sim.set_input("modrm", &LW::from_u64(8, u64::from(modrm)))?;
            sim.set_input("displacement", &LW::from_u64(16, u64::from(displacement)))?;
            sim.set_input("override_present", &LW::from_u64(1, u64::from(override_present)))?;
            sim.set_input("override_segment", &LW::from_u64(2, 0))?;
            sim.set_input("bx", &LW::from_u64(16, 0x0100))?;
            sim.set_input("bp", &LW::from_u64(16, 0x0050))?;
            sim.set_input("si", &LW::from_u64(16, 0x0040))?;
            sim.set_input("di", &LW::from_u64(16, 0x0060))?;
            let got_memory = sim
                .read_port("memory_operand")
                .and_then(|value| value.to_u64())
                .ok_or("missing memory_operand")?;
            let got_offset = sim
                .read_port("effective_offset")
                .and_then(|value| value.to_u64())
                .ok_or("missing effective_offset")?;
            let got_segment = sim
                .read_port("default_segment")
                .and_then(|value| value.to_u64())
                .ok_or("missing default_segment")?;
            if got_memory != u64::from(memory)
                || got_offset != u64::from(offset)
                || got_segment != u64::from(segment)
            {
                return Err(format!(
                    "modrm={modrm:#04x} memory={got_memory} offset={got_offset:#x} segment={got_segment}"
                ));
            }
        }
        Ok(())
    });

    c.add("platform_systemverilog_decoder_contracts_pass_lec", || {
        for (source, top) in [
            (RV32I_DECODER, "rv32i_isa_decoder"),
            (I8086_PREFIX_DECODER, "i8086_prefix_decoder"),
            (I8086_OPCODE_DECODER, "i8086_opcode_decoder"),
            (I8086_MODRM_DECODER, "i8086_modrm_decoder"),
        ] {
            let flow = eda_api::run_flow(source, Some(top))
                .map_err(|e| format!("{top} RTL flow failed: {e}"))?;
            if !flow.equivalent {
                return Err(format!("{top} failed RTL-versus-mapped LEC"));
            }
            if flow.drc.has_errors() {
                return Err(format!("{top} decoder layout has DRC errors"));
            }
        }
        Ok(())
    });

    c.add("platform_rv32i_full_core_runs_memory_program", || {
        let instructions: &[u32] = &[
            0x801230b7, // lui x1, 0x80123
            0x45608093, // addi x1, x1, 0x456 -> 0x80123456
            0x00102023, // sw x1, 0(zero)
            0x00100103, // lb x2, 1(zero)
            0x00304183, // lbu x3, 3(zero)
            0x00201203, // lh x4, 2(zero)
            0x00100073, // ebreak
        ];
        let mut golden_memory = eda_cpu_platform::SparseMemory::new();
        for (index, instruction) in instructions.iter().enumerate() {
            golden_memory.write_le_u32((index as u32) * 4, *instruction);
        }
        let mut golden = eda_cpu_platform::rv32i::Rv32iMachine::baremetal(0, golden_memory);
        if golden.run(100) != eda_cpu_platform::rv32i::StopReason::Ebreak {
            return Err("golden RV32I model did not reach EBREAK".into());
        }
        let source = full_core_tb_source(&[
            0x801230b7, 0x45608093, 0x00102023, 0x00100103, 0x00304183, 0x00201203, 0x00100073,
        ])?;
        let design =
            eda_hdl::compile(&source, Some("rv32i_full_tb_16")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.run_until(35)?;
        let regs = sim.read_port("dbg_x").ok_or("missing dbg_x output")?;
        let expected = [
            golden.regs[1],
            golden.regs[2],
            golden.regs[3],
            golden.regs[4],
        ];
        if (0..4).any(|index| extract_word(&regs, index) != u64::from(expected[index])) {
            return Err(format!(
                "x1={:#010x} x2={:#010x} x3={:#010x} x4={:#010x}",
                extract_word(&regs, 0),
                extract_word(&regs, 1),
                extract_word(&regs, 2),
                extract_word(&regs, 3)
            ));
        }
        Ok(())
    });

    c.add(
        "platform_rv32i_full_core_executes_checked_in_elf_image",
        || {
            let mut golden = eda_cpu_platform::rv32i::Rv32iMachine::load_elf(
                include_bytes!("../../../platform/programs/rv32i_exit42.elf"),
                &[],
            )
            .map_err(|e| e.to_string())?;
            if golden.run(100) != eda_cpu_platform::rv32i::StopReason::Exit(42) {
                return Err("golden RV32I model rejected checked-in ELF".into());
            }
            let bytes: Vec<u8> = include_bytes!("../../../platform/programs/rv32i_exit42.elf")
                .iter()
                .skip(0x100)
                .copied()
                .collect();
            let mut instructions = Vec::new();
            for index in (0..bytes.len()).step_by(4) {
                let mut word = 0_u32;
                for (shift, byte) in bytes[index..index + 4].iter().enumerate() {
                    word |= u32::from(*byte) << (shift * 8);
                }
                instructions.push(word);
            }
            let source = full_core_tb_source(&instructions)?;
            let design =
                eda_hdl::compile(&source, Some("rv32i_full_tb_16")).map_err(|e| e.to_string())?;
            let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
            sim.run_until(30)?;
            let a0 = sim
                .read_port("dbg_x10")
                .and_then(|value| value.to_u64())
                .ok_or("missing dbg_x10 output")?;
            let cause = sim
                .read_port("trap_cause")
                .and_then(|value| value.to_u64())
                .ok_or("missing trap_cause output")?;
            let expected_a0 = u64::from(golden.regs[10]);
            if a0 != expected_a0 || cause != 11 {
                return Err(format!("a0={a0} expected={expected_a0} trap_cause={cause}"));
            }
            Ok(())
        },
    );

    c.add("platform_rv32i_full_core_passes_rtl_to_mapped_lec", || {
        let flow = eda_api::run_flow(RV32I_FULL_CORE, Some("rv32i_full_core"))
            .map_err(|e| format!("RV32I full-core flow failed: {e}"))?;
        if !flow.equivalent {
            return Err("RV32I full core failed RTL-versus-mapped LEC".into());
        }
        if flow.drc.has_errors() {
            return Err("RV32I full-core layout has DRC errors".into());
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_passes_rtl_to_mapped_lec", || {
        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}{I8086_FULL_CORE}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let route = eda_api::RouteOptions::new(512)?;
        let library = eda_api::CellLibrary::std();
        let flow = eda_api::run_flow_with_library_options_cancellable(
            &source,
            Some("i8086_full_core"),
            &library,
            &route,
            &eda_api::FlowCancellation::new(),
        )
        .map_err(|e| format!("i8086 full-core flow failed: {e}"))?;
        if !flow.equivalent {
            return Err("i8086 full core failed RTL-versus-mapped LEC".into());
        }
        if flow.drc.has_errors() {
            return Err("i8086 full-core layout has DRC errors".into());
        }
        Ok(())
    });

    c.add(
        "platform_i8086_full_core_executes_immediate_and_halt",
        || {
            let program = [0xb8_u8, 0x2a, 0x00, 0xf4];
            let mut golden = eda_cpu_platform::i8086::I8086Machine::com(&program, "")
                .map_err(|e| e.to_string())?;
            if golden.run(100) != eda_cpu_platform::i8086::I8086Stop::Halt {
                return Err("golden i8086 model did not reach HLT".into());
            }

            let source = format!(
                "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
                include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
            );
            let design =
                eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
            let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
            sim.set_input("program_image", &byte_program_image(&program))?;
            sim.run_until(30)?;
            let ax = sim
                .read_port("ax")
                .and_then(|value| value.to_u64())
                .ok_or("missing AX output")?;
            let halted = sim
                .read_port("halted")
                .and_then(|value| value.to_u64())
                .ok_or("missing halted output")?;
            if halted != 1 || ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX]) {
                return Err(format!("halted={halted} ax={ax}"));
            }
            Ok(())
        },
    );

    c.add("platform_i8086_full_core_alu_byte_flags", || {
        let program = [0xb0_u8, 0xfe, 0x04, 0x02, 0xf4];
        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(35)?;
        let ax = sim
            .read_port("ax")
            .and_then(|value| value.to_u64())
            .ok_or("missing AX")?;
        let flags = sim
            .read_port("flags")
            .and_then(|value| value.to_u64())
            .ok_or("missing flags")?;
        if ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX])
            || flags != u64::from(golden.flags)
        {
            return Err(format!(
                "ax={ax:#x} flags={flags:#x} golden-ax={:#x} golden-flags={:#x}",
                golden.regs[eda_cpu_platform::i8086::REG_AX],
                golden.flags
            ));
        }
        Ok(())
    });

    c.add(
        "platform_i8086_full_core_group_alu_and_modrm_memory",
        || {
            let program = [
                0xb8_u8, 0x78, 0x56, // mov ax, 0x5678
                0xbb, 0x20, 0x00, // mov bx, 0x20
                0x82, 0xf0, 0x02, // add al, 2 (8086 alias)
                0x89, 0x07, // mov [bx], ax
                0x8b, 0x0f, // mov cx, [bx]
                0xf4, // hlt
            ];
            let mut golden = eda_cpu_platform::i8086::I8086Machine::com(&program, "")
                .map_err(|e| e.to_string())?;
            golden.run(100);
            if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
                return Err(format!("golden stop={:?}", golden.stop));
            }

            let source = format!(
                "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
                include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
            );
            let design =
                eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
            let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
            sim.set_input("program_image", &byte_program_image(&program))?;
            sim.run_until(80)?;
            for (port, expected) in [
                ("ax", golden.regs[eda_cpu_platform::i8086::REG_AX]),
                ("bx", golden.regs[eda_cpu_platform::i8086::REG_BX]),
                ("cx", golden.regs[eda_cpu_platform::i8086::REG_CX]),
            ] {
                let value = sim
                    .read_port(port)
                    .and_then(|word| word.to_u64())
                    .ok_or_else(|| format!("missing {port} output"))?;
                if value != u64::from(expected) {
                    return Err(format!("{port}={value:#x} expected={expected:#x}"));
                }
            }
            Ok(())
        },
    );

    c.add("platform_i8086_full_core_rep_movsb", || {
        let program = [
            0xb8_u8, 0x12, 0x00, // mov ax, source address
            0x89, 0xc6, // mov si, ax
            0xbf, 0x03, 0x00, // mov di, 3
            0xb9, 0x03, 0x00, // mov cx, 3
            0xfc, // cld
            0xf3, 0xa4, // rep movsb
            0xa0, 0x03, 0x00, // mov al, [3]
            0xf4, // hlt
        ];
        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.memory.write_slice(0x12, b"ABC");
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let mut image = program.to_vec();
        image.extend_from_slice(b"ABC");
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&image))?;
        sim.run_until(100)?;
        let ax = sim
            .read_port("ax")
            .and_then(|word| word.to_u64())
            .ok_or("missing AX")?;
        let expected = u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX]);
        if ax != expected {
            return Err(format!("ax={ax:#x} expected={expected:#x}"));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_rotate", || {
        let program = [0xb8_u8, 0x34, 0x12, 0xd1, 0xc0, 0xf4];
        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(60)?;
        let ax = sim
            .read_port("ax")
            .and_then(|word| word.to_u64())
            .ok_or("missing AX")?;
        let flags = sim
            .read_port("flags")
            .and_then(|word| word.to_u64())
            .ok_or("missing flags")?;
        if ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX])
            || flags != u64::from(golden.flags)
        {
            return Err(format!(
                "ax={ax:#x} flags={flags:#x} golden={:#x}/{:#x}",
                golden.regs[eda_cpu_platform::i8086::REG_AX],
                golden.flags
            ));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_interrupt_and_iret", || {
        let mut program = vec![
            0xb8_u8, 0x42, 0x00, // mov ax, 0x42
            0xcd, 0x03, // int 3
            0xf4, // hlt (interrupt return address)
        ];
        program.resize(12, 0);
        program.extend_from_slice(&[0x10, 0x00]); // vector 3 offset
        program.extend_from_slice(&[0x00, 0x00]); // vector 3 segment
        program.extend_from_slice(&[
            0xb8, 0x24, 0x00, // handler: mov ax, 0x24
            0xcf, // iret
        ]);

        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.memory.write_le_u16(12, 0x0010);
        golden.memory.write_le_u16(14, 0x0000);
        golden.memory.write_slice(0x10, &program[16..]);
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(120)?;
        let ax = sim
            .read_port("ax")
            .and_then(|word| word.to_u64())
            .ok_or("missing AX")?;
        let halted = sim
            .read_port("halted")
            .and_then(|word| word.to_u64())
            .ok_or("missing halted")?;
        if halted != 1 || ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX]) {
            return Err(format!(
                "halted={halted} ax={ax:#x} expected={:#x} ip={ip:?} cs={cs:?} sp={sp:?}",
                golden.regs[eda_cpu_platform::i8086::REG_AX],
                ip = sim.read_port("ip").and_then(|word| word.to_u64()),
                cs = sim.read("dut.core.dbg_cs").and_then(|word| word.to_u64()),
                sp = sim.read("dut.core.sp").and_then(|word| word.to_u64())
            ));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_runs_hello_program", || {
        let mut program = vec![
            0xb4_u8, 0x09, // mov ah, 9
            0xba, 0x0c, 0x00, // mov dx, message
            0xcd, 0x21, // int 21h
            0xb8, 0x00, 0x4c, // mov ax, 4c00h
            0xcd, 0x21, // int 21h
        ];
        program.extend_from_slice(b"Hello$");

        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.memory.write_slice(0x000c, b"Hello$");
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt || golden.output != b"Hello" {
            return Err(format!(
                "golden stop={:?} output={:?}",
                golden.stop, golden.output
            ));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        let mut output = Vec::new();
        let mut previous_clock = false;
        for time in 1..100_u64 {
            sim.run_until(time)?;
            let clock = sim.read_port("clock").and_then(|word| word.to_u64()) == Some(1);
            let write = sim
                .read_port("console_write")
                .and_then(|word| word.to_u64())
                == Some(1);
            if write && clock && !previous_clock {
                let character = sim
                    .read_port("console_char")
                    .and_then(|word| word.to_u64())
                    .ok_or("missing console character")?;
                output.push(character as u8);
            }
            previous_clock = clock;
        }
        let halted = sim
            .read_port("halted")
            .and_then(|word| word.to_u64())
            .ok_or("missing halted")?;
        if halted != 1 || output != b"Hello" {
            return Err(format!("halted={halted} output={output:?}"));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_mul_and_div", || {
        let program = [
            0xb0_u8, 0x0c, // mov al, 12
            0xb3, 0x0a, // mov bl, 10
            0xf6, 0xe3, // mul bl
            0xf6, 0xf3, // div bl
            0xf4, // hlt
        ];
        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(120)?;
        let ax = sim
            .read_port("ax")
            .and_then(|word| word.to_u64())
            .ok_or("missing AX")?;
        let halted = sim
            .read_port("halted")
            .and_then(|word| word.to_u64())
            .ok_or("missing halted")?;
        if halted != 1 || ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX]) {
            return Err(format!(
                "halted={halted} ax={ax:#x} expected={:#x}",
                golden.regs[eda_cpu_platform::i8086::REG_AX]
            ));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_aam_arbitrary_base", || {
        let program = [0xb0_u8, 0x3f, 0xd4, 0x07, 0xf4];
        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(80)?;
        let ax = sim
            .read_port("ax")
            .and_then(|word| word.to_u64())
            .ok_or("missing AX")?;
        if ax != u64::from(golden.regs[eda_cpu_platform::i8086::REG_AX]) {
            return Err(format!(
                "ax={ax:#x} expected={:#x}",
                golden.regs[eda_cpu_platform::i8086::REG_AX]
            ));
        }
        Ok(())
    });

    c.add("platform_i8086_full_core_not_far_call_and_return", || {
        let mut program = vec![
            0xb3_u8, 0x5a, // mov bl, 0x5a
            0xf6, 0xd3, // not bl
            0x9a, 0x0a, 0x00, 0x00, 0x00, // call far 0000:000a
            0xf4, // halt after far return
        ];
        program.resize(10, 0);
        program.extend_from_slice(&[
            0xcb, // far return
        ]);

        let mut golden =
            eda_cpu_platform::i8086::I8086Machine::com(&program, "").map_err(|e| e.to_string())?;
        golden.memory.write_u8(0x000a, 0xcb);
        golden.run(100);
        if golden.stop != eda_cpu_platform::i8086::I8086Stop::Halt {
            return Err(format!("golden stop={:?}", golden.stop));
        }

        let source = format!(
            "{I8086_PREFIX_DECODER}\n{I8086_OPCODE_DECODER}\n{}\
             {I8086_FULL_CORE}\n{I8086_FULL_SOC_64}\n{I8086_FULL_TB_64}",
            include_str!("../../../platform/rtl/i8086_modrm_decoder.sv")
        );
        let design =
            eda_hdl::compile(&source, Some("i8086_full_tb_64")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("program_image", &byte_program_image(&program))?;
        sim.run_until(120)?;
        let bx = sim
            .read_port("bx")
            .and_then(|word| word.to_u64())
            .ok_or("missing BX")?;
        let ip = sim
            .read_port("ip")
            .and_then(|word| word.to_u64())
            .ok_or("missing IP")?;
        let halted = sim
            .read_port("halted")
            .and_then(|word| word.to_u64())
            .ok_or("missing halted")?;
        if halted != 1 || bx != u64::from(golden.regs[eda_cpu_platform::i8086::REG_BX]) {
            return Err(format!(
                "halted={halted} ip={ip} bx={bx:#x} trap={:?}",
                sim.read_port("trap").and_then(|word| word.to_u64())
            ));
        }
        Ok(())
    });
}

fn program_image(words: &[u32]) -> Result<LW, String> {
    if words.len() > 16 {
        return Err("16-word SOC accepts at most 16 instructions".into());
    }
    let mut bits = Vec::with_capacity(512);
    for index in 0..16 {
        let word = words.get(index).copied().unwrap_or(0);
        bits.extend(LW::from_u64(32, u64::from(word)).bits);
    }
    Ok(LW { bits })
}

fn full_core_tb_source(words: &[u32]) -> Result<String, String> {
    let program = program_image(words)?;
    let mut digits = String::with_capacity(512);
    // LogicWord is LSB-first while a Verilog literal is MSB-first.
    for bit in program.bits.iter().rev() {
        digits.push(bit.to_char());
    }
    let testbench = RV32I_FULL_TB_16
        .replace(
            "module rv32i_full_tb_16(prog, halted,",
            "module rv32i_full_tb_16(halted,",
        )
        .replace(
            "  input [511:0] prog;\n",
            &format!("  wire [511:0] prog = 512'b{digits};\n"),
        );
    Ok(format!(
        "{RV32I_FULL_CORE}\n{RV32I_FULL_SOC_16}\n{testbench}"
    ))
}

fn extract_word(word: &LW, index: usize) -> u64 {
    let base = index * 32;
    let mut value = 0_u64;
    for index in 0..32 {
        let bit = word.bits[base + index].as_bool().unwrap_or(false);
        value |= u64::from(bit) << index;
    }
    value
}

fn byte_program_image(bytes: &[u8]) -> LW {
    let mut bits = Vec::with_capacity(512);
    for index in 0..64 {
        let byte = bytes.get(index).copied().unwrap_or(0);
        bits.extend(LW::from_u64(8, u64::from(byte)).bits);
    }
    LW { bits }
}
