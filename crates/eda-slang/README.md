# `eda-slang`

This crate is the explicit integration boundary to
[slang](https://github.com/MikePopoloski/slang), a full SystemVerilog compiler
frontend. It uses slang for:

1. include-directory and macro processing;
2. complete SystemVerilog parsing, type checking, and elaboration;
3. post-elaboration AST JSON export;
4. production of a single preprocessed compilation unit for the repository's
   bounded downstream IR elaborator;
5. post-lowering comparison of slang's top-module, port, simple
   declaration-width, top-parameter, continuous/procedural-target/RHS
   operand-shape/order/symbol identity, sized integer-literal, simple
   type-width/signedness, and direct/generated-instance inventory against the
   resulting repository `Design`, while retaining source spans and per-instance
   post-elaboration parameter values as authoritative metadata;
6. explicit direct AST lowering for a small verified subset: ports/nets,
   initializers, continuous assignments, integer/reference/parameter-constant
   leaves, unary and binary operators, concatenation, conditional expressions,
   constant selects, initial/always procedures, blocking/non-blocking
   assignments, blocks, if/case control flow, and single posedge/negedge
   sensitivity, plus recursive parameterized module hierarchy, port
   connections, and generated members.

The adapter deliberately reports slang diagnostics instead of silently
bypassing them. If the downstream elaborator cannot represent a construct, that
failure remains visible after slang has accepted the source.
`compile_files` parses the same include/macro-aware elaborated AST and rejects
a bounded result whose authoritative structural summary disagrees with slang.
`compile_files_via_ast` uses the first direct AST-to-IR lowering path for the
simple subset and returns an explicit unsupported-construct error rather than
silently falling back. This remains a prototype, not full AST-to-IR lowering.

## Installing the external frontend

`eda-slang` does not vendor a platform-specific binary. Build or install slang
and point the workspace at it:

```sh
git clone https://github.com/MikePopoloski/slang
cmake -S slang -B slang-build
cmake --build slang-build --target slang_driver -j8
export SLANG_BIN="$PWD/slang-build/bin/slang"
```

`SLANG` is accepted as an alternative environment variable. If neither is set,
the adapter searches `PATH` for `slang`.

The optional official-UVM conformance test is enabled with:

```sh
git clone https://github.com/accellera-official/uvm-core
export UVM_HOME="$PWD/uvm-core"
```
