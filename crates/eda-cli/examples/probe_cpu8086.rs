use eda_core::ir::{Expr, LValue, Range, Sens, Stmt};
use std::collections::HashSet;

fn expr_nets(e: &Expr, out: &mut HashSet<usize>) {
    match e {
        Expr::Const(_)
        | Expr::SimulationTime { .. }
        | Expr::MemorySize { .. }
        | Expr::AssocExists { .. } => {}
        Expr::RealConst(_)
        | Expr::RealRef { .. }
        | Expr::ShortRealConst(_)
        | Expr::ShortRealRef { .. }
        | Expr::ProcessSelf => {}
        Expr::ProcessStatus(handle) => expr_nets(handle, out),
        Expr::Past { value, .. } => expr_nets(value, out),
        Expr::Sampled(value) => expr_nets(value, out),
        Expr::SampledValue { value, .. } => expr_nets(value, out),
        Expr::RealBinary { left, right, .. } => {
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::RealCompare { left, right, .. } => {
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::RealNeg(operand) => expr_nets(operand, out),
        Expr::ShortRealBinary { left, right, .. } => {
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::ShortRealNeg(operand)
        | Expr::ShortRealToReal(operand)
        | Expr::RealToShortReal(operand)
        | Expr::BitsToReal(operand)
        | Expr::BitsToShortReal(operand) => expr_nets(operand, out),
        Expr::RealToInt { value, .. }
        | Expr::RealTruncToInt { value, .. }
        | Expr::IntToReal { value, .. } => expr_nets(value, out),
        Expr::RealTime { .. } => {}
        Expr::AssocRead { index, .. } => expr_nets(index, out),
        Expr::Ref { net, .. } => {
            out.insert(*net);
        }
        Expr::Un { a, .. } => expr_nets(a, out),
        Expr::Bin { a, b, .. } => {
            expr_nets(a, out);
            expr_nets(b, out);
        }
        Expr::Cond { c, t, f } => {
            expr_nets(c, out);
            expr_nets(t, out);
            expr_nets(f, out);
        }
        Expr::Concat(v) => v.iter().for_each(|e| expr_nets(e, out)),
        Expr::Repeat { e, .. } => expr_nets(e, out),
        Expr::Index { e, .. } => expr_nets(e, out),
        Expr::Signed { e, .. } => expr_nets(e, out),
        Expr::StringLength { e } => expr_nets(e, out),
        Expr::StringConcat { left, right } => {
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::StringFormat { args, .. } => {
            args.iter().for_each(|argument| expr_nets(argument, out));
        }
        Expr::StringGetByte { e, index } => {
            expr_nets(e, out);
            expr_nets(index, out);
        }
        Expr::StringCase { e, .. } => expr_nets(e, out),
        Expr::StringCompare { left, right, .. } => {
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::StringPutc { e, index, value } => {
            expr_nets(e, out);
            expr_nets(index, out);
            expr_nets(value, out);
        }
        Expr::StringSubstr { e, left, right } => {
            expr_nets(e, out);
            expr_nets(left, out);
            expr_nets(right, out);
        }
        Expr::StringToRadix { e, .. } => expr_nets(e, out),
        Expr::StringFromRadix { value, .. } => expr_nets(value, out),
        Expr::MemoryPattern { .. } => {}
        Expr::EnumPattern { value, .. } => expr_nets(value, out),
        Expr::StructPattern { value, .. } => expr_nets(value, out),
        Expr::EventTriggered { net } => {
            out.insert(*net);
        }
        Expr::MemoryRead { indexes, .. } => {
            indexes.iter().for_each(|e| expr_nets(e, out));
        }
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes.iter().for_each(|e| expr_nets(e, out));
            expr_nets(start, out);
        }
        Expr::Random { args, .. } => args.iter().for_each(|e| expr_nets(e, out)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => expr_nets(value, out),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            expr_nets(path, out);
            expr_nets(mode, out);
        }
        Expr::FileGetc { fd } => expr_nets(fd, out),
        Expr::FileUngetc { value, fd } => {
            expr_nets(value, out);
            expr_nets(fd, out);
        }
        Expr::FileEof { fd } => expr_nets(fd, out),
        Expr::FileTell { fd } => expr_nets(fd, out),
        Expr::FileSeek { fd, offset, whence } => {
            expr_nets(fd, out);
            expr_nets(offset, out);
            expr_nets(whence, out);
        }
    }
}

fn lv_bits(lv: &LValue) -> Vec<usize> {
    match lv.range {
        Range::Bit(b) => vec![lv.net + b],
        Range::Span { msb, lsb } => ((lv.net + lsb)..=(lv.net + msb)).collect(),
    }
}

fn feedback(s: &Stmt) -> bool {
    fn walk(s: &Stmt, reads: &mut HashSet<usize>, writes: &mut HashSet<usize>) {
        match s {
            Stmt::Block(v) => v.iter().for_each(|s| walk(s, reads, writes)),
            Stmt::Forever(b) => walk(b, reads, writes),
            Stmt::NBl { lv, e } | Stmt::Bl { lv, e } => {
                expr_nets(e, reads);
                writes.extend(lv_bits(lv));
            }
            Stmt::MemoryBl { indexes, e, .. } | Stmt::MemoryNBl { indexes, e, .. } => {
                indexes.iter().for_each(|e| expr_nets(e, reads));
                expr_nets(e, reads);
            }
            Stmt::MemoryCopy { .. } => {}
            Stmt::MemoryNew { size, .. } => expr_nets(size, reads),
            Stmt::MemoryDelete { .. } => {}
            Stmt::QueuePush { value, .. } => expr_nets(value, reads),
            Stmt::QueuePop { .. } => {}
            Stmt::QueueInsert { index, value, .. } => {
                expr_nets(index, reads);
                expr_nets(value, reads);
            }
            Stmt::QueueDelete { index, .. } => expr_nets(index, reads),
            Stmt::AssocTraversal {
                key, destination, ..
            } => {
                reads.extend(lv_bits(key));
                writes.extend(lv_bits(destination));
            }
            Stmt::AssocWrite { index, value, .. } => {
                expr_nets(index, reads);
                expr_nets(value, reads);
            }
            Stmt::AssocDelete { index, .. } => {
                if let Some(index) = index {
                    expr_nets(index, reads);
                }
            }
            Stmt::MemoryBlPart {
                indexes, start, e, ..
            }
            | Stmt::MemoryNBlPart {
                indexes, start, e, ..
            } => {
                indexes.iter().for_each(|e| expr_nets(e, reads));
                expr_nets(start, reads);
                expr_nets(e, reads);
            }
            Stmt::NBlIndexed {
                net,
                decl_width,
                start,
                e,
                ..
            }
            | Stmt::BlIndexed {
                net,
                decl_width,
                start,
                e,
                ..
            } => {
                expr_nets(start, reads);
                expr_nets(e, reads);
                writes.extend(*net..*net + *decl_width);
            }
            Stmt::If { c, t, f } => {
                expr_nets(c, reads);
                walk(t, reads, writes);
                if let Some(fs) = f {
                    walk(fs, reads, writes);
                }
            }
            Stmt::Case { e, arms, dflt } => {
                expr_nets(e, reads);
                for (ae, st) in arms {
                    expr_nets(ae, reads);
                    walk(st, reads, writes);
                }
                if let Some(d) = dflt {
                    walk(d, reads, writes);
                }
            }
            Stmt::Delay { body, .. } => walk(body, reads, writes),
            Stmt::Wait { cond, body } => {
                expr_nets(cond, reads);
                walk(body, reads, writes);
            }
            Stmt::EventWait { watches, body } => {
                reads.extend(watches.iter().map(|watch| watch.net));
                walk(body, reads, writes);
            }
            Stmt::WaitOrder {
                events,
                success,
                failure,
            } => {
                reads.extend(events.iter().copied());
                if let Some(statement) = success {
                    walk(statement, reads, writes);
                }
                if let Some(statement) = failure {
                    walk(statement, reads, writes);
                }
            }
            Stmt::Fork { branches, .. } => branches
                .iter()
                .for_each(|branch| walk(branch, reads, writes)),
            Stmt::NamedBlock { body, .. } => walk(body, reads, writes),
            Stmt::While { cond, body, .. } => {
                expr_nets(cond, reads);
                walk(body, reads, writes);
            }
            Stmt::Repeat { count, body } => {
                expr_nets(count, reads);
                walk(body, reads, writes);
            }
            Stmt::Force { e, .. } => expr_nets(e, reads),
            Stmt::ProceduralAssign { e, .. } => expr_nets(e, reads),
            Stmt::Release { .. } => {}
            Stmt::ProceduralDeassign { .. } => {}
            Stmt::DisableFork | Stmt::WaitFork | Stmt::NamedBlockEnd => {}
            Stmt::DisableNamedBlock { .. } => {}
            Stmt::ProcessKill(handle)
            | Stmt::ProcessAwait(handle)
            | Stmt::ProcessSuspend(handle)
            | Stmt::ProcessResume(handle) => expr_nets(handle, reads),
            Stmt::Break | Stmt::Continue | Stmt::ReturnBarrier | Stmt::Return => {}
            Stmt::Scope(body) => walk(body, reads, writes),
            Stmt::Assert { cond, .. } | Stmt::Assume { cond, .. } | Stmt::Cover { cond, .. } => {
                expr_nets(cond, reads)
            }
            Stmt::WriteMem { start, finish, .. } => {
                if let Some(start) = start {
                    expr_nets(start, reads);
                }
                if let Some(finish) = finish {
                    expr_nets(finish, reads);
                }
            }
            Stmt::FileClose { fd } | Stmt::FileFlush { fd } => expr_nets(fd, reads),
            Stmt::FileGets { fd, string, status } => {
                expr_nets(fd, reads);
                writes.extend(lv_bits(string));
                writes.extend(lv_bits(status));
            }
            Stmt::FileError {
                fd,
                message,
                status,
            } => {
                expr_nets(fd, reads);
                writes.extend(lv_bits(message));
                writes.extend(lv_bits(status));
            }
            Stmt::FileRead { fd, target, status } => {
                expr_nets(fd, reads);
                writes.extend(lv_bits(target));
                writes.extend(lv_bits(status));
            }
            Stmt::FileReadMemory {
                fd,
                start,
                count,
                status,
                ..
            } => {
                expr_nets(fd, reads);
                if let Some(start) = start {
                    expr_nets(start, reads);
                }
                if let Some(count) = count {
                    expr_nets(count, reads);
                }
                writes.extend(lv_bits(status));
            }
            Stmt::FileScan {
                fd,
                targets,
                status,
                ..
            } => {
                expr_nets(fd, reads);
                for target in targets {
                    writes.extend(lv_bits(target));
                }
                writes.extend(lv_bits(status));
            }
            Stmt::StringScan {
                source,
                targets,
                status,
                ..
            } => {
                expr_nets(source, reads);
                for target in targets {
                    writes.extend(lv_bits(target));
                }
                writes.extend(lv_bits(status));
            }
            Stmt::FileDisplay { fd, args, .. } => {
                expr_nets(fd, reads);
                args.iter().for_each(|e| expr_nets(e, reads));
            }
            Stmt::RandomSeeded { seed, destination } => {
                reads.extend(lv_bits(seed));
                writes.extend(lv_bits(seed));
                writes.extend(lv_bits(destination));
            }
            Stmt::ImmediateAssertion {
                cond,
                pass_action,
                fail_action,
                ..
            } => {
                expr_nets(cond, reads);
                for action in pass_action.iter().chain(fail_action.iter()) {
                    walk(action, reads, writes);
                }
            }
            Stmt::Display { .. }
            | Stmt::TimeFormat { .. }
            | Stmt::AssertControl(_)
            | Stmt::Severity(_)
            | Stmt::Finish
            | Stmt::Stop => {}
        }
    }
    let mut r = HashSet::new();
    let mut w = HashSet::new();
    walk(s, &mut r, &mut w);
    let both: Vec<_> = r.intersection(&w).collect();
    if !both.is_empty() {
        println!("  reads∩writes has {} nets", both.len());
    }
    !both.is_empty()
}

fn main() {
    let mut src = String::new();
    for f in [
        "primitives",
        "ripple_adder16",
        "alu16",
        "regfile16",
        "cpu8086",
        "ram64x16",
        "sys8086",
    ] {
        src.push_str(&std::fs::read_to_string(format!("cpu8086/rtl/{f}.v")).unwrap());
        src.push('\n');
    }
    let d = eda_api::compile(&src, Some("sys8086")).expect("compile");
    let mut n = 0;
    for (i, p) in d.processes.iter().enumerate() {
        if matches!(p.sens, Sens::Comb) && feedback(&p.body) {
            println!("comb process #{i} flagged");
            n += 1;
        }
    }
    println!("{n} flagged of {} processes", d.processes.len());
}
