use eda_cpu_platform::i8086::{I8086Machine, I8086Stop};
use eda_cpu_platform::rv32i::{Rv32iMachine, StopReason};
use std::io::Write;
use std::process::ExitCode;

#[derive(Debug)]
struct CommandError(String);

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, CommandError> {
    let command = args
        .first()
        .ok_or_else(|| CommandError(usage().to_string()))?
        .as_str();
    match command {
        "validate" => {
            let results = eda_cpu_platform::validation::platform_validation();
            for result in &results {
                println!(
                    "{} {} {}",
                    if result.passed { "PASS" } else { "FAIL" },
                    result.name,
                    result.detail
                );
            }
            if results.iter().all(|result| result.passed) {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        "rv32i" => run_rv32i(&args[1..]),
        "i8086" => run_i8086(&args[1..]),
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            Ok(ExitCode::SUCCESS)
        }
        other => Err(CommandError(format!(
            "unknown command '{other}'\n{}",
            usage()
        ))),
    }
}

fn run_i8086(args: &[String]) -> Result<ExitCode, CommandError> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CommandError("missing i8086 action".into()))?;
    if action != "run" {
        return Err(CommandError(format!(
            "unsupported i8086 action '{action}'; expected 'run'"
        )));
    }
    let path = args
        .get(1)
        .ok_or_else(|| CommandError("i8086 run requires a .COM or MZ .EXE path".into()))?;
    let mut max_steps = 100_000_000_u64;
    let mut arguments = Vec::new();
    let mut use_systemverilog = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--steps" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CommandError("--steps requires a value".into()))?;
                max_steps = value
                    .parse()
                    .map_err(|_| CommandError(format!("invalid step limit '{value}'")))?;
                index += 2;
            }
            "--arg" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CommandError("--arg requires a value".into()))?;
                arguments.push(value.clone());
                index += 2;
            }
            "--sv" => {
                use_systemverilog = true;
                index += 1;
            }
            other => return Err(CommandError(format!("unknown i8086 run option '{other}'"))),
        }
    }
    let bytes = std::fs::read(path)
        .map_err(|error| CommandError(format!("cannot read '{path}': {error}")))?;
    if use_systemverilog {
        if bytes.len() >= 2 && bytes[0] == b'M' && bytes[1] == b'Z' {
            return Err(CommandError(
                "--sv currently supports full-size COM images, not MZ executables".into(),
            ));
        }
        let result = eda_cpu_platform::systemverilog::run_i8086_com(&bytes)
            .map_err(|error| CommandError(format!("SystemVerilog i8086 execution: {error}")))?;
        if !result.output.is_empty() {
            let _ = std::io::stdout().write_all(&result.output);
        }
        eprintln!(
            "i8086-sv halted={} trap={} instructions={} ip={:#06x} ax={:#06x}",
            result.halted, result.trap, result.instructions, result.ip, result.ax
        );
        return if result.trap {
            Ok(ExitCode::FAILURE)
        } else {
            Ok(ExitCode::SUCCESS)
        };
    }
    let command_tail = arguments.join(" ");
    let mut machine = if bytes.len() >= 2 && bytes[0] == b'M' && bytes[1] == b'Z' {
        I8086Machine::mz(&bytes, 0x1000)
    } else {
        I8086Machine::com(&bytes, &command_tail)
    }
    .map_err(|error| CommandError(format!("cannot load i8086 program: {error}")))?;
    let stop = machine.run(max_steps);
    if !machine.output.is_empty() {
        let _ = std::io::stdout().write_all(&machine.output);
    }
    eprintln!(
        "i8086 stop={stop:?} instructions={} exit={:?} pages={}",
        machine.instructions_retired,
        machine.exit_code,
        machine.memory.page_count()
    );
    match stop {
        I8086Stop::Halt if machine.exit_code.unwrap_or(0) == 0 => Ok(ExitCode::SUCCESS),
        I8086Stop::Halt => {
            eprintln!("program exited with {}", machine.exit_code.unwrap_or(0));
            Ok(ExitCode::FAILURE)
        }
        I8086Stop::MaxInstructions => {
            eprintln!("instruction limit reached");
            Ok(ExitCode::FAILURE)
        }
        I8086Stop::Running => Ok(ExitCode::SUCCESS),
        other => {
            eprintln!("i8086 execution fault: {other:?}");
            Ok(ExitCode::FAILURE)
        }
    }
}

fn run_rv32i(args: &[String]) -> Result<ExitCode, CommandError> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| CommandError("missing rv32i action".into()))?;
    if action != "run" {
        return Err(CommandError(format!(
            "unsupported rv32i action '{action}'; expected 'run'"
        )));
    }
    let path = args
        .get(1)
        .ok_or_else(|| CommandError("rv32i run requires an ELF path".into()))?;
    let mut max_steps = 100_000_000_u64;
    let mut program_args = Vec::new();
    let mut use_systemverilog = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--steps" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CommandError("--steps requires a value".into()))?;
                max_steps = value
                    .parse()
                    .map_err(|_| CommandError(format!("invalid step limit '{value}'")))?;
                index += 2;
            }
            "--arg" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CommandError("--arg requires a value".into()))?;
                program_args.push(value.clone());
                index += 2;
            }
            "--sv" => {
                use_systemverilog = true;
                index += 1;
            }
            other => return Err(CommandError(format!("unknown rv32i run option '{other}'"))),
        }
    }

    let bytes = std::fs::read(path)
        .map_err(|error| CommandError(format!("cannot read '{path}': {error}")))?;
    if use_systemverilog {
        let result = eda_cpu_platform::systemverilog::run_rv32i_elf(&bytes)
            .map_err(|error| CommandError(format!("SystemVerilog RV32I execution: {error}")))?;
        eprintln!(
            "rv32i-sv halted={} trap={} trap_cause={} pc={:#010x} a0={:#010x} cycles={}",
            result.halted,
            result.trap,
            result.trap_cause,
            result.pc,
            result.a0,
            result.instructions
        );
        return Ok(ExitCode::SUCCESS);
    }
    let mut machine = Rv32iMachine::load_elf(&bytes, &program_args)
        .map_err(|error| CommandError(format!("cannot load RV32I ELF: {error}")))?;
    let stop = machine.run(max_steps);
    if !machine.output.is_empty() {
        let _ = std::io::stdout().write_all(&machine.output);
    }
    eprintln!(
        "rv32i stop={stop:?} instructions={} cycles={} pages={}",
        machine.instructions_retired,
        machine.cycle,
        machine.memory.page_count()
    );
    match stop {
        StopReason::Exit(code) => {
            if code == 0 {
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!("program exited with {code}");
                Ok(ExitCode::FAILURE)
            }
        }
        StopReason::Ebreak => Ok(ExitCode::SUCCESS),
        StopReason::MaxSteps => {
            eprintln!("instruction limit reached");
            Ok(ExitCode::FAILURE)
        }
        StopReason::Fault => {
            eprintln!("architectural fault: {:?}", machine.trap);
            Ok(ExitCode::FAILURE)
        }
        StopReason::Running => Ok(ExitCode::SUCCESS),
    }
}

fn usage() -> &'static str {
    "usage:\n  cpu-platform validate\n  cpu-platform rv32i run <rv32i.elf> [--steps N] [--arg VALUE]... [--sv]\n  cpu-platform i8086 run <program.com|program.exe> [--steps N] [--arg VALUE]... [--sv]"
}
