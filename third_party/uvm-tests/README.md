# Pinned Accellera UVM test fixture

This directory contains unmodified test fixtures from Accellera's upstream
`uvm-tests` contribution branch.

- Source: `https://github.com/accellera-official/uvm-tests`
- Pinned commit: `c3c4e3a48ddba52131ad57d6f0ca19c9cf904427`
- Provenance: unmerged upstream contribution ref `refs/pull/1/head`; the
  repository `main` branch currently contains only its initial empty commit.
- `tests/00basic/00hello/test.sv`
  SHA-256: `2f7bab5ece003406f164fb5d9908081ad71323a922ea58fee6ed62fc94f677a0`
- `tests/00basic/99compat/7455/test.sv`
  SHA-256: `f9322af923d8ff75cd519f70de05e8f4c05db697f5fd7693e27c631ee5d41198`
  The pinned UVM 2020.3.1 library reports minor revision 3 while this issue
  fixture compares it with 2.0, so the fixture intentionally selects its
  mismatch branch in this repository.
- License: Apache-2.0 (`LICENSE.txt`)

The fixtures are intentionally not reformatted or modified. Regression wrappers
must preserve this distinction and must not label adapted repository-owned
tests as upstream fixtures.
