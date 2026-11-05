# Vendored sv-tests SVA corpus

Source repository: https://github.com/chipsalliance/sv-tests

Pinned upstream commit: `b8cbf49962ccac836dcb54ecb00feb2ae1fa4f9f`

License: ISC. See [LICENSE.txt](LICENSE.txt) and SPDX identifiers in each source file.

This directory intentionally vendors the non-UVM IEEE 1800 Chapter 10
assignment/continuous-assignment tests, Chapter 12 procedural-statement tests, Chapter 16 assertion
tests, and the selected Chapter 20 constant-math, bit-query, array-query, typename, and unbounded tests as an
external conformance corpus. The upstream UVM variants remain outside this
first slice because their required simulation drivers and resource DB behavior
are broader than the initial direct-lowering gate.

Files are copied verbatim from `tests/chapter-10`, `tests/chapter-12`, `tests/chapter-16`, and the reviewed slice of `tests/chapter-20` except
that upstream `*-uvm.sv` variants are not included in this initial slice.
