# Capability Gap Audit — 2026-10-10

## Purpose

The objective is an API-first EDA tool with commercial-tool breadth. A claim of
completion is not useful without a live audit. Therefore the code exposes the
audit through `eda_api::capabilities()`, `eda_api::missing_capabilities()`, and
the CLI command `eda capabilities`.

## Verified additions in this iteration

### Digital simulator correctness and scalability follow-up

The CPU8086 system regression exposed several event-scheduler defects that are
now covered by focused tests:

* zero-delay updates from the same continuous-assign driver are coalesced so a
  stale event cannot overwrite that driver's newest value;
* `case` label comparison zero-extends the narrower operand, matching sized
  Verilog case matching when a 32-bit parameter labels a narrow state;
* the zero-delay activity guard is scoped to one settle operation rather than
  accumulating across every timestamp;
* current-value no-op events do not consume the loop guard, allowing wide
  hierarchical vector loads to stay within the budget;
* external input stimulation settles queued time-zero initialization before
  injecting a first edge;
* focused regressions cover localparam-labeled clocked FSMs, hierarchical NBA
  propagation, repeated clock edges through X, and 1024-bit vector connections.

This does not close the larger SystemVerilog language gap: the simulator is
still a deliberately bounded synthesizable subset rather than a commercial
event kernel.

### Missing domain found: analog/mixed-signal simulation

The pre-existing workspace was strongly digital and physical-signoff oriented.
There was no SPICE frontend, DC MNA solver, AC phasor sweep, or transient
integrator. That gap has now been filled at the foundational level by `eda-ams`.

Implemented capabilities:

* case-insensitive SPICE subset parser and deterministic writer;
* engineering numeric suffixes (`k`, `meg`, `u`, `n`, `p`, `f`, and others);
* R/C/L, independent V/I sources, VCVS/VCCS/CCVS/CCCS, and diodes;
* DC, DC sweep, AC, and transient directives;
* dense real and complex MNA solvers;
* backward-Euler and trapezoidal companions;
* source models DC/SIN/PULSE/PWL, including AC magnitude/phase;
* nonlinear diode Newton iteration with continuation;
* initial capacitor-voltage and inductor-current enforcement at t=0;
* physical resistors through `SHEET`, `L`, and `W`, including an optional
  card multiplier;
* per-layer `.rmodel` sheet resistance with temperature coefficients,
  width offset, and minimum/maximum width limits;
* width-bin interpolation for both sheet resistance and width offset;
* external layer tables through `parse_resistor_layer_models(...)` or
  `eda ams input.spice --rmodels layers.rmodels`;
* CSV export and rising/falling/any threshold-crossing queries;
* one-call `eda-api` functions and CLI `ams` command.

### Follow-up: compact transistors

The first AMS iteration supported diodes but no transistor models. That gap
has now been advanced from “missing” to “partial”:

* Level-1 NMOS/PMOS square-law DC and small-signal models;
* body-effect threshold modulation;
* channel-length modulation;
* `THETA` overdrive mobility degradation;
* `VSAT` velocity-saturated drain voltage and current limiting;
* voltage-dependent source/drain-bulk junction sidewall capacitance through
  `CJSW/CJSWS/CJSWD/PB/MJ/FC`, including forward-knee linear continuation;
* W/L/M instance scaling;
* forward/reverse Ebers-Moll NPN/PNP terminal currents;
* `VAF/VAR` forward/reverse Early output-resistance modulation;
* lumped BJT `RTH/CTH` self-heating with transient device temperature;
* AREA instance scaling;
* nonlinear Jacobian stamping and Newton solution;
* logarithmic source-step continuation for BJT convergence;
* lumped MOS `RTH/CTH` self-heating with transient device temperature;
* SPICE model and instance parsing/writer round trips.

The AMS analysis implementation is split by analysis responsibility:
`analysis.rs` owns the public DC/transient/AC/PSS/envelope/aging/noise facade,
`analysis/devices.rs` compact-device temperature/electrothermal helpers,
`analysis/solver.rs` nonlinear MNA and device stamps,
`analysis/statistical.rs` Monte Carlo sampling, `analysis/optimization.rs`
scalar/Pareto optimization, and `analysis/random.rs` deterministic correlated
sampling/circuit mutation. The derivative domain is further divided into local
DC, transient, AC/Jet, shared parameter discovery, and global variance modules
under `analysis/derivatives/`. The public `eda_ams::analysis` API is unchanged.

### Follow-up: small-signal noise

The AMS engine now supports `.noise`:

* frequency-linear/decade/octave sweeps;
* output-node selection through `V(node)`;
* input-source transfer and input-referred noise;
* resistor Johnson-Nyquist thermal noise;
* diode shot noise;
* BJT base-emitter/base-collector shot noise;
* MOS drain thermal noise from its operating-point transconductance;
* per-device contribution reporting;
* one-call API and typed `NoiseResult`.

Current limitations are incomplete foundry noise/correlation modeling. Basic
MOS/diode/BJT `KF/AF/EF` flicker noise and temperature dependence are now
included.

Temperature is no longer fixed: `.temp` now controls resistor thermal noise,
junction thermal voltage, diode/BJT saturation-current scaling, MOS mobility
scaling, and resistor `TC1/TC2` coefficients. MOS cards with `RTH` solve a
lumped power/temperature/current fixed-point iteration, so DC current falls as
local mobility and BSIM threshold temperature scaling respond to dissipation.
BJT cards use the same thermal-resistance convention with their
temperature-scaled saturation current, thermal voltage, and Early factors.
The local electrothermal solution also feeds MOS intrinsic charge and BJT
transit charge, so AC/transient companion stamps use the same temperature as
the nonlinear DC operating point rather than the ambient circuit temperature.
When `CTH` is present, transient samples carry a typed per-element temperature
map and integrate the lumped thermal pole with an explicit-power,
implicit-temperature companion. `CTH=0` retains the quasi-static `RTH`
fixed-point solution. `TransientResult` exposes sorted device names and
temperature vectors, reports include `T(element)=...K`, CSV export can mix
`V(node)` and `T(element)` columns, and the daemon `spiceRun` response returns
structured transient samples with `deviceTemperatures` in addition to the text
report.
Geometry-derived `SHEET`, `.rmodel LAYER`, imported generic layer tables, and
width-bin interpolation are supported. Certified tool-specific foundry files,
coupled thermal-node networks, and spatial thermal networks remain future work.

### Follow-up: deterministic power-law aging

`.aging [target] V(output)` applies a deterministic power-law stress model to
the same OP/transient/AC target used by sensitivity and Monte Carlo:

```spice
.aging V(out) years=9 exp=0.5 rinc=0.1
```

It reports nominal and aged outputs, the applied stress, the output delta, and
a ready-to-run aged netlist. Resistor resistance uses
`1 + rinc * years^exp`; MOS VTO shifts by `vtshift * years^exp` with polarity
aware magnitude, while KP scales by `1 - kploss * years^exp`. Twenty
data-driven cases cover varied duration, exponent, resistor degradation,
threshold shift, and KP loss. Calibrated BTI/HCI physics, stochastic aging,
and foundry lifetime models remain future work.

### Follow-up: local sensitivity analysis

`.sens` now computes central-difference local derivatives for operating-point,
transient-final, and AC-magnitude targets. Parameters include R/C/L, DC source
values, diode `IS`, MOS `VTO/KP`, and BJT `BF/IS`. Results include absolute and
normalized sensitivity plus high/low output samples. This is a local
one-at-a-time derivative analysis.

### Follow-up: analytic DC, nonlinear-diode, AC, and transient derivatives

`.deriv V(output) params=R1.R,Vin.DC` computes exact first and second
derivatives for linear DC operating points. It solves one adjoint system per
parameter and a second system per Hessian entry, supporting resistor `R` and
DC voltage/current source values. The result exposes the full symmetric
Hessian and is used by `.optim` as an exact bounded Newton polish for linear
DC match problems.

DC diode circuits now use the implicit-function theorem at the converged
Newton operating point. Parameters include resistor `R`, DC source values, and
diode model `IS`; the full Hessian includes the diode's second voltage-derivative
tensor. Twenty data-driven resistor/diode operating-point cases compare every
gradient and Hessian entry with the closed-form scalar implicit equations, and
`.optim` can use the resulting Hessian for diode `IS` matching.

The diode-connected ideal-NMOS DC path
`V--R--D/G(out),NMOS(source=bulk=ground)` now has exact implicit
gradients and full Hessians for load `R`, model `VTO/KP`, and source DC.
The path currently requires an ideal Level-1 NMOS with zero body effect,
mobility degradation, velocity saturation, and channel-length modulation.
Twenty data-driven cases compare the MNA Newton operating point and every
derivative entry with the closed-form quadratic solution; `.optim` uses the
same Hessian for exact bounded match polishing.

The corresponding diode-connected ideal-NPN path
`V--R--C/B(out),NPN(emitter=ground)` now computes implicit gradients and full
Hessians for load `R`, model `IS/BF`, instance `AREA`, and source DC at the
Ebers-Moll operating point. Because collector and base are tied, the DC
terminal current is
`IS*AREA*(1+1/BF)*expm1(V/VT)`. Twenty data-driven cases vary supply, load,
saturation current, beta, reverse beta, and instance area, solve an independent
scalar equation, and compare every Hessian entry; `.optim` uses the resulting
Hessian for bounded match polishing, including instance-AREA matching. General
multi-terminal BJT networks remain future work.

`.deriv ac V(output) freq=... params=...` now applies the same adjoint idea to
linear AC magnitude targets. It supports R, C, L, and AC source-magnitude
parameters, returns exact complex first derivatives and the full magnitude
Hessian, and round-trips through the typed API and CLI. Twenty data-driven RC
low-pass cases compare every gradient and Hessian entry with the closed-form
transfer function. Linear AC match optimization now uses those derivatives for
a Newton/Gauss-Newton polish; an indefinite full Hessian falls back to a
positive semidefinite Gauss-Newton step and backtracks candidate updates.

The source-R/ideal-diode small-signal topology now supports nonlinear AC
magnitude derivatives at its converged DC operating point. Parameters include
series `R`, diode model `IS` and `TT`, source DC bias, and AC source
magnitude. The result includes the full implicit Hessian of the complex
dynamic-conductance/diffusion-plus-depletion-capacitance divider, including
direct `CJO/VJ/M/FC` derivatives and the SPICE forward-knee continuation.
Twenty ideal-diode cases, ten transit-time cases, ten depletion-capacitance
cases, and ten junction-parameter cases compare gradients and Hessians against
independent finite-difference or scalar-recurrence references. General
MOS/BJT nonlinear-device networks and arbitrary-waveform transient adjoint
Hessians remain future work.

`.deriv tran V(output) params=...` has two verified paths. The scalar series
topology `Vin--R--V(out),C(out--ground)` uses the exact continuous-time
closed form. All other linear R/C/L/V/I networks use full backward-Euler and
trapezoidal tangent/second-tangent recurrences through the same MNA matrices
as the transient solver. The recurrence propagates capacitor current and
inductor voltage/current companion state, computes exact discrete first
derivatives and complete Hessians for `R`, `C`, `L`, and DC source values,
and preserves parallel-UIC capacitor handling. Twenty scalar cases, twenty
multi-element backward-Euler cases, and twenty multi-element trapezoidal
cases compare every Hessian entry with independent recurrence results.
Nonlinear devices and arbitrary time-varying source derivatives remain future
work.

### Follow-up: global variance sensitivity

`.sobol` now estimates first-order and total-order variance indices with a
deterministic seed. It supports the same OP/transient/AC targets and the same
R/C/L/source/model parameter set as `.sens`. Correlated-parameter global
decomposition is supported through pairwise correlation entries and a local
linear covariance surrogate. Fully nonlinear covariance-aware variance
decomposition remains future work.

### Follow-up: Monte Carlo statistical simulation

The AMS engine now supports `.mc`:

* deterministic xorshift RNG and explicit seed;
* independent R/C/L tolerance sampling;
* diode and BJT saturation-current decade sampling;
* MOS VTO/KP sampling;
* BJT BF sampling;
* built-in uniform, truncated-normal, and lognormal marginal distributions through
  `dist=uniform|normal|lognormal`;
* operating-point target extraction;
* transient final-value target extraction;
* AC magnitude target extraction at a requested frequency;
* same-type global process correlation through `corr` and per-class correlation controls;
* explicit pairwise parameter correlation matrices such as
  `matrix=R1.R|R2.R|-0.75`, including cross-device and cross-class entries;
* full normal/lognormal covariance input through
  `cov=R1.R:2500,R2.R:10000,R1.R|R2.R:-2500`, including positive-definite
  Cholesky validation and exact natural-to-log covariance conversion;
* imported statistical models through `.stats` or the typed
  `parse_statistical_model` API, including parameter-specific distributions,
  relative/absolute sigma, and pairwise correlations;
* CLI import with `eda ams input.spice --stats vendor.stats`;
* compact-DEF location extraction with `apply_physical_def_locations(...)` and
  `eda ams input.spice --def-locations layout.def`; components are matched to
  SPICE elements by stable name and database units are converted to meters;
* spatial mismatch through `.location NAME X Y`, `spatialcorr=`, and
  `spatialscale=`, using Gaussian distance-dependent correlation;
* min/max/mean/sigma statistics;
* per-sample randomized-object counts;
* parse/write round trips and API/CLI reports.

This is not yet a certified foundry statistical flow: DEF-to-SPICE matching
requires stable names rather than an automatic correspondence model, the
statistical import format is generic rather than a certified tool-specific
vendor format, and correlated stochastic aging and vendor correlation models
are not implemented.

The remaining gap is no longer the absence of transistors. A BSIM3-compatible
`LEVEL=8` core evaluates `VTH0/K1/K2`, oxide-field mobility degradation
through `TOX/U0/UA/UB/UC`, physical `VSAT`, a length/DIBL threshold reduction
using `DVT0/DVT1/DVT2`, and a narrow-width threshold shift using
`DVT0W/DVT1W/W0`. Threshold and mobility temperature scaling use
`KT1/KT2/KT1L/UTE`. Model cards named `NAME.N` with
`WMIN/WMAX/LMIN/LMAX` are selected automatically by instance geometry and
reject uncovered or overlapping bins. One hundred independent DC cases compare
Newton operating points/currents against independent scalar references,
spanning long-channel, short-channel, narrow-width, body-bias,
low/high-temperature, and binned geometry.
`.sens` and `.sobol` expose
`VTH0/K1/U0/TOX/VSAT/DVT0/DVT1/DVT2/DVT0W/DVT1W/W0/KT1/KT2/KT1L`. The
dynamic engine now also stamps bias-dependent source/drain junction sidewall
capacitances from the same cards, scales them by W/M, and round-trips the
typed fields through the SPICE writer. The
`RTH/CTH` cards add lumped DC and transient self-heating to the same
temperature-dependent threshold, mobility, charge, and BJT transit equations.
The remaining gap is the full advanced foundry model surface: complete DVT
equations, vendor parameter
interpolation/binning semantics, PSP/HiSIM/VBIC/HICUM equations, nonlinear
foundry charge, coupled spatial thermal networks, and advanced noise.

### Follow-up: foundational transistor charge storage

The transistor models now include foundational dynamic charge:

* MOS `CGSO`, `CGDO`, and gate-bulk capacitance;
* MOS intrinsic oxide charge with region-dependent `COX` capacitance;
* MOS saturation channel-charge partition through `XPART`;
* instance scaling by `W`, `L`, and `M`;
* BJT `CJE/CJC` depletion capacitance with `VJE/VJC/MJE/MJC/FC`;
* diode `CJO/VJ/M/FC` depletion capacitance;
* diode `TT` transit/diffusion charge;
* BJT `TF/TR` transit/diffusion charge;
* BJT instance `AREA` scaling;
* transient BE/trapezoidal companion stamps;
* internal capacitor-current state tracking;
* AC admittance stamps for transistor capacitances;
* parse/write round trips for all new model properties.

This is not yet a foundry-grade nonlinear charge model: bias-dependent transit
modulation, non-quasi-static effects, mechanical-stress dependence, and
BSIM/PSP charge equations remain future work.

### Test-driven defects found and fixed

The new 1,000-case suite exposed real defects during development:

* SPICE suffix ordering incorrectly parsed `1meg`;
* model properties were not joined across spaces inside `D(...)`;
* diode card arity incorrectly expected two arguments instead of three;
* PULSE-only sources were rejected as having no value;
* RC transients did not enforce initial capacitor voltage;
* inductor transients did not enforce t=0 initial branch current;
* auxiliary C/L branch rows made AC systems singular;
* PULSE periods shorter than rise+width+fall collapsed to the initial value.

All are covered by the current workspace tests.

### Follow-up: quantized optimization and AC branch stamping

`.optim` parameters now accept `name:lower:upper[:initial[:quantum]]` (or a
shared `quantum=` property). Quantized candidates are evaluated only on the
engineering grid, final values never exceed the bounds, and a deterministic
axis refinement replaces continuous Newton polish. Fifty data-driven cases
cover integer/E-series resistor and picofarad-capacitor grids. The same work
exposed an AC MNA bug: identity equations intended for eliminated L/C branch
rows were also being added to voltage-source and controlled-source branch
equations, leaking a hidden `Ibranch` term into their constitutive rows. AC now
initializes only capacitor/inductor auxiliary rows; source-node and ideal-VCVS
phasors are regression-tested exactly.

### Follow-up: Pareto multi-objective optimization

`.pareto [target] V(output1) V(output2) ... sense=min,max,... params=...` now
builds a deterministic two-through-eight-objective differential-evolution
archive. It supports
OP/transient/AC outputs, continuous or quantized parameters, explicit bounds,
one sense per output, and bounded point-count pruning while preserving both
front extremes through normalized multi-objective crowding distance.
`front=nsga3 divisions=P` switches pruning to normalized Das-Dennis simplex
reference directions; each returned point carries its unit reference vector.
`adaptive=true` adds a second data-dependent layer built from normalized front
points, then uses farthest-direction sampling while preserving objective axes.
Constraints such as `constraints=V(node)>=bound` and arithmetic expressions
like `constraints=(2*V(a)+V(b)-0.5*V(c))<=2.8` use the same
simulated target and participate in Deb-style constrained dominance: feasible
candidates dominate infeasible ones, infeasible candidates compete by total
violation, and feasible candidates compete by objective dominance. Bound-corner
seeding improves feasibility discovery. Every returned point carries its typed
parameter values, constraint values/violation, and a ready-to-run optimized
netlist. Fifty unconstrained, twenty-five node-bound constrained, twenty
arithmetic-constrained, ten function-constrained, and fifteen three-objective
ladder cases verify
mathematical nondominance, ordering, parameter bounds, evaluation count, and
actual output tradeoffs. Expression constraints support `+`, `-`, `*`, `/`,
`^`/`**`, parentheses, constants, node values, and `abs`, `sqrt`, `exp`,
`ln`/`log`, `log10`, `min`, and `max`. User-defined or differential constraint
functions remain future work.

### Follow-up: bounded global optimization

`.optim` performs deterministic bounded differential evolution over the same
R/C/L/source/model parameter set used by sensitivity analysis. It supports
`match`, `min`, and `max` objectives; OP, transient-final, and AC targets;
explicit bounds, optional initial values, and per-parameter quantization;
configurable population, seed, generations, crossover, and differential
weight. Linear-DC match objectives use `.deriv` adjoint gradients and Hessians
for an exact bounded Newton polish; nonlinear-device, AC, and transient
analytic Hessian optimization remains future work. Node-bound-constrained
multi-objective Pareto fronts are handled by `.pareto`.

### Follow-up: driven periodic steady state

`.pss` now performs shooting-style cycle convergence on a transient waveform
and emits a one-sided harmonic spectrum. It is suitable for externally driven
periodic circuits such as switched RC networks and sinusoidally excited
filters. `auto` estimates the fundamental from stable rising threshold
crossings before the steady-state search. Capacitor `IC=` conditions can start
a three-stage CMOS ring oscillator; twenty varied autonomous-ring cases now
check startup, estimated fundamental, dominant harmonic, and cycle residual.
Full nonlinear and multi-tone harmonic balance remain future work.

### Follow-up: envelope following

`AM(offset amplitude carrier modulation depth)` now provides an amplitude-
modulated transient source. `.env V(output) carrier=... cycles=... points=...`
performs quadrature demodulation of the transient waveform and returns the
extracted envelope and carrier phase over time. Multi-carrier envelope
systems and an autonomous envelope-step solver remain future work.

### Follow-up: persistent agent daemon

`eda daemon --listen <ip>:<port>` provides a dependency-free, line-delimited
JSON RPC service over TCP. Protocol version 1 supports `ping`,
`capabilities`, `spiceDc`, `spiceRun` for every parsed AMS analysis directive, `coverage`, `assertions`,
`waveformMeasure`,
`layoutDrc`,
`rtlEquiv`, `rtlSta`, `rtlPnr`, `rtlDft`, `rtlCts`, `rtlEco`, `rtlCdc`, `rtlFault`, `rtlSim`, `rtlPower`, `rtlIr`, `rtlBmc`, `rtlFlow`, `rtlFlowCache`, `flowCacheStats`, `flowCacheClear`, and `pdkSignoff`, including escaped source/artifact strings, typed
SPICE operating-point maps, RTL equivalence statistics, structured STA
delay/hold/case-analysis data, placement/routing/via/wire-length/DRC summaries,
power/IR row/cell/finding summaries, temporal-property status/counterexample
traces, custom-port scan-chain audits, PDK correlation results, and
clock-domain CTS summaries, structural ECO change logs, sampled VCD assertion statuses, clock-domain crossings/findings, stuck-at fault detections/STIL patterns, PDK
correlation results, and Verilog/LEF/DEF/SDF artifacts. Remote agents can
also use
`jobSubmit`, `jobStatus`, `jobCancel`, bounded `jobWait`, and the remote
worker/artifact RPCs; all TCP
connections share the same persistent typed job manager with deterministic
worker execution, status polling, failure capture, cancellation before start,
cooperative cancellation at RTL/PDK major-stage boundaries, inside CDCL SAT
propagation/search, generic BMC, equivalence miter solving, k-induction
unrolling, temporal-property unrolling, global/maze/negotiated-rip-up routing,
RTL simulator delta scheduling, single-corner physical-timing
extraction/STA/antenna/hold/optimization stages, and AMS
transient/Monte-Carlo/Sobol/PSS/optimizer/quantized-refine loops, and
concurrent clients.
Expanding the direct method surface to every specialized API and pushing
cancellation into every nested multi-corner physical-timing analysis stage
remains future work.

### Follow-up: typed PDK signoff

The API now parses and writes typed PDK manifests containing routing-layer
geometry, process/voltage/temperature corners, optional per-corner delay
scales, inline per-corner Liberty views with optional SHA-256 integrity checks,
an optional inline layer/via rule deck with SHA-256 integrity and full typed
rule validation, an optional foundry antenna-ratio rule that replaces the
default physical-verification threshold, nominal-versus-golden model
correlations with relative-error tolerances, and the required signoff checks.
`run_pdk_signoff` imports an inline Liberty view when present (or creates a
scaled built-in library otherwise), reruns the RTL implementation flow for
every PVT-tagged corner, and reports per-corner model correlation, synthesis
equivalence, finite STA delay, DRC severity, and nonempty LEF/DEF/SDF physical
databases. `eda pdk design.v --pdk manifest.pdk` exposes the same report. This
is a portable MMMC signoff framework, not a foundry-certified PDK: a Liberty
or rule-deck checksum detects accidental corruption but does not establish vendor identity;
external provenance/certification, imported compact-model cards, and golden
certification remain future work.

### Follow-up: API-first editable schematic database

`eda-schematic` now provides a GUI-toolkit-independent document model. It
includes symbol outlines and pins, instance placement with rotation/mirror
transforms, wires, named ports, deterministic validation, nearest-pin/port/wire
hit testing, net extraction through point-on-wire connectivity, XML-escaped SVG
rendering, and atomic undo/redo edits. `eda-api` re-exports the complete typed
surface. A desktop/browser event shell and polygon-level layout editing remain
future work.

### Follow-up: waveform signal measurement

`Vcd` now provides typed edge extraction for scalar rising/falling and vector
value transitions, clock period/frequency/duty metrics, high/low pulse-width
metrics scaled by the VCD timescale, and width-checked multi-bit signal
comparison with matching/differing/unknown sample and Hamming-distance counts.
The `eda-api` facade re-exports the complete result and edge types, and daemon
method `waveformMeasure` returns the typed clock, pulse, and optional comparison
payload. This iteration adds focused malformed/non-clock rejection tests and
200 generated regression cases: 100 clock geometries and 100 vector comparison
patterns.

### Follow-up: polygon- and via-level layout editing

`eda_pnr::layout` adds a typed Manhattan layout database with rectangles,
simple rectilinear polygons, layer rules, via rules, exact polygon area and
intersection geometry, point hit testing, shape translation, atomic edits, and
undo/redo. DRC now checks layer width/area, exact same-layer different-net
spacing, unknown layers/rules, via cut dimensions, and bottom/top enclosure.
Exact Manhattan union, intersection, difference, and symmetric-difference
operations return disjoint rectangle covers with exact total area. Typed fix
suggestions cover minimum width/area growth, horizontal spacing movement, and
via enclosure patches. The complete typed surface is re-exported by `eda-api`,
and daemon method
`layoutDrc` accepts JSON layer rules, via rules, rectangles, polygons, and vias.
This iteration adds 300 generated cases: 100 spacing geometries, 100
two-layer via-enclosure geometries, and 100 overlap sweeps across all four
Boolean operations.

`eda_pnr::any_angle` now provides a typed `AnyAnglePolygon` primitive for
simple polygons whose edges need not be Manhattan. It validates three-or-more
distinct vertices, rejects self-intersections and zero-area rings, computes
exact `i128` doubled signed area, winding direction, bounding boxes, boundary
inclusive point containment, checked translation, squared edge lengths, and
floating-point perimeter. The type is re-exported by `eda-api`; it is a
first-class geometry primitive. It also detects boundary/interior intersection
and computes the minimum squared distance between disjoint arbitrary-angle
polygons as an exact rational number; `check_spacing_to` compares that rational
against a squared integer spacing requirement without floating-point rounding
and returns typed required distance, actual distance, and compliance fields.
One hundred geometry cases cover arbitrary
triangles, trapezoids, concave notched polygons, diamonds, clockwise and
counterclockwise windings, exact area, bbox and containment, boundary hits,
translation invariance, equal diamond edges, and invalid bowtie/repeated-close
rings. Another hundred spacing cases cover triangles, diamonds, trapezoids,
concave polygons, diagonal perpendicular projection, integer and rational
squared distances, exact-boundary compliance, larger-than-required spacing,
violating spacing, touching/overlap zero distance, and API-facing typed
results. A further hundred cases integrate `LayoutGeometry::AnyAngle` into the
typed layout document: shapes validate on insertion, participate in point hit
testing, render as SVG polygons, preserve exact doubled area, move with checked
translation, satisfy or violate exact layer spacing DRC, and are explicitly
rejected by Manhattan Boolean operations. The API and browser editor now also
expose a dedicated `AnyAnglePolygon` tool: pointer clicks retain unrestricted
angles, double-click/finish validates and stores the typed shape, properties
report the geometry kind, and Select drags an individual vertex without the
Manhattan orthogonal constraint; undo/redo preserves the exact point sequence.
A hundred generated editor sessions rotate triangles, diamonds, trapezoids, and
concave polygons, vary preview point sequences, selected vertices, arbitrary
drag targets, and undo/redo behavior. A separate direct two-pin
`AnyAngleRouteRequest` API now creates straight arbitrary-angle routes,
retains exact integer squared lengths, rejects polygon blockage crossings,
rejects different-net crossings, enforces exact rational inter-net spacing,
allows same-net branches to meet, and sums typed route lengths. One hundred
cases rotate exact diagonal lengths, parallel net spacing, spacing violations,
polygon blockage rejection, same-net crossing branches, and zero-length
endpoint rejection. A bounded visibility-graph maze router now complements the
direct path: it generates clearance candidates around polygon corners, uses
Dijkstra over legal arbitrary-angle visibility edges, preserves exact squared
lengths for every polyline segment, honors blockage and different-net spacing,
and returns multi-bend paths around simple polygon obstacles. A negotiated
rip-up mode independently routes the initial population, records exact
conflicting segment pairs as history cost, removes and reroutes nets with
present-conflict and history penalties, reports typed initial conflicts and
pass count, and converges crossing nets to conflict-free detours.

The typed geometry API now also performs exact Boolean operations on two simple
arbitrary-angle polygons. Union, intersection, and left-minus-right difference
split boundaries at checked integer/rational intersections, classify subsegments
by exact rational containment, remove duplicate undirected boundary edges,
assemble closed result loops (including an opposite-oriented interior hole for
a contained difference), and preserve rational intersection vertices such as
`13/5` rather than rounding them to integer database units. Total area uses
signed loop orientation so a hole subtracts from its enclosing polygon while
disjoint results add. Crate-local tests cover overlap area identities, rational
vertices, identical inputs, containment with a hole, and disjoint polygons.

Layer rules now carry optional exact density limits in basis points and a
positive square evaluation window. DRC unions all Manhattan shapes on a layer,
evaluates a deterministic critical-edge sliding-window set (overall-union center
and windows aligned with each disjoint-cover edge), clips the cover to every
window, and computes integer area before comparing minimum/maximum density.
Overlapping same-layer shapes are counted once rather than double-counted, and
a local 90% stripe is caught even when the overall union center falls in an
empty region. The window search is bounded to these critical candidates rather
than every DBU translation.
Rules reject densities above 100%, inverted ranges, and nonpositive windows.
The daemon `layoutDrc` method accepts `minDensityBp`, `maxDensityBp`,
`densityWindow`, and arbitrary-angle polygon shapes, so remote agents can submit
the same typed foundry rule-deck fragment used by the Rust API.

Layer rules also carry a typed cross-layer spacing table. For different-net
shapes on two layers, DRC examines both directional entries, applies the
stricter requirement, and uses the exact geometry distance engine. Same-net
cross-layer shapes remain exempt so stacked connecting metal is not reported as
a spacing violation. The daemon parses a `spacingTo` object in each remote layer
rule.

Same-layer different-net shapes are additionally checked for positive-area
overlap using exact Manhattan or arbitrary-angle Boolean intersection. The
arbitrary-angle path first applies an exact positive-area predicate: identical,
contained, or properly crossing interiors are Boolean-intersected, while
boundary-only contact skips the degenerate line result. A real overlap reports
`LAYOUT.SHORT` even when a legacy deck sets `minSpacing` to zero;
boundary-only touching remains governed by the explicit spacing rule and is
legal at zero spacing. Same-net overlap remains legal and is unioned for density
accounting.

The typed layer/via rule deck now has deterministic text import/export. The
format carries version, all layer width/area/spacing/notch/density fields,
cross-layer spacing edges, and complete via cut/enclosure/extension/cut-spacing
fields. Parsing rejects duplicate or forward-referenced layers/vias and unknown
keywords, comments are allowed, and writing emits the same typed objects in
sorted order. Daemon method `layoutRuleDeck` validates a remote deck, returns
typed counts, and emits the normalized representation.

Layout DRC results now support a separate auditable waiver layer. A typed
waiver names an exact violation code plus optional layer/net/object selectors,
owner, and reason; duplicate IDs are rejected. Applying waivers never mutates
the raw DRC computation—the returned object retains active violations, waived
violations with owner/reason attribution, used waiver IDs, and a text audit
report. The daemon `layoutDrc` method accepts the same waiver objects and also
reports pre-waiver error/violation counts.

Those physical waivers are also persistent across daemon requests and async
jobs. Agents can create bounded named waiver registries, add or remove typed
rules with an audit actor, list/get/delete registries, and pass
`waiverRegistryId` to `layoutDrc`. Explicit inline waivers and registry-backed
waivers are mutually exclusive. Registry responses retain active rules and
immutable added/removed event snapshots; removing a waiver immediately restores
the underlying violation to the active DRC result.

Via rules additionally carry an optional exact `minCutSpacing`. DRC checks every
pair of cuts on a common cut layer using exact rectangle spacing and reports
`LAYOUT.VIA_CUT_SPACING`; when two rules disagree, the stricter spacing wins.
Positive-area cut overlap is independently illegal and reports
`LAYOUT.VIA_CUT_OVERLAP` even when a legacy deck leaves `minCutSpacing` at zero;
merely touching cuts remain governed by the explicit spacing rule. The daemon
parses the same `minCutSpacing` field from a remote rule deck and returns both
typed pair diagnostics.

Layer rules also carry an optional `minNotch`. For rectilinear polygons, DRC
takes scanlines between every adjacent pair of horizontal edge coordinates,
forms exact covered intervals from vertical-edge crossings, and reports the
narrowest gap between disjoint intervals as the notch opening. A 20 DBU notch
against a 25 DBU rule therefore fails exactly, while a 30 DBU notch passes.
Arbitrary-angle polygons fail closed with `LAYOUT.NOTCH_GEOMETRY` when a notch
rule is enabled because that geometry family does not yet have a certified notch
decomposition. The daemon accepts `minNotch` in remote layer-rule objects.

Via rules also support independent bottom/top X/Y metal extensions beyond the
cut. DRC combines each directional extension with the legacy uniform enclosure
requirement and checks that same-net metal contains the resulting axis-expanded
rectangle. This supports common asymmetric via tables where, for example, bottom
metal must extend 10 units in X and 5 in Y while top metal extends 5 in X and 10
in Y. The daemon parses all four fields from remote rule decks and reports the
same typed `LAYOUT.VIA_ENCLOSURE` diagnosis.

### Follow-up: interactive schematic event session

`eda-schematic` now adds a GUI-toolkit-independent pointer event session above
the document database. It tracks select and wire tools, pin/port/instance/wire
selection, atomic instance dragging, endpoint snapping, wire previews, wire
creation, deletion, and undo/redo. Every action is exposed as a typed
`SchematicEvent` and returns a typed outcome through `eda-api`. This iteration
adds 200 generated event cases: 100 drag vectors and 100 wire snap/connectivity
sweeps.

### Follow-up: browser schematic editor host

`eda-schematic::browser` now bundles a dependency-free browser host. It serves
an HTML/JavaScript editor, current SVG, and a local HTTP event endpoint. Pointer
down/move/up coordinates are transformed from screen space into schematic
space and dispatched to the same persistent `SchematicSession`; toolbar and
keyboard actions drive tool changes, undo/redo, and deletion. The server and
typed service are re-exported by `eda-api`, so an agent can embed or launch the
host without a GUI dependency. Focused tests cover HTML/SVG/event requests,
JSON errors, and a real TCP request/response cycle; 100 generated browser-host
wire-event HTTP round trips were added to the full regression suite.

### Follow-up: browser layout editor host

`eda_pnr::layout` now has a typed pointer event session and
`eda_pnr::layout_browser` bundles a dependency-free browser host. The session
supports select/move/delete, rectangle drawing, rule-sized via placement,
undo/redo, DRC checks, unique object naming, and SVG rendering. The browser host
dispatches screen-space pointers into database-unit layout coordinates and
exposes Select/Rectangle/Via, undo/redo/delete, and Check-DRC controls through
the same persistent session. `eda-api` re-exports both service/server types.
Focused tests cover HTTP HTML/SVG/event behavior and a real TCP request cycle,
and 100 generated layout rectangle-event HTTP round trips were added to the
full regression suite.

Polygon follow-up support adds a Manhattan polygon tool and polygon-vertex
selection. Vertex drags preserve rectilinear geometry by moving the selected
corner and its adjacent corner atomically. The browser toolbar exposes Polygon,
double-click finishes a polygon, and Select can drag existing polygon vertices.
One hundred generated browser polygon-event round trips cover heights 20..39.

Both browser hosts now include bounded zoom, directional and Alt/middle-mouse
panning, keyboard view controls, Ctrl-wheel zoom, a view reset, and a typed
selection property grid. `/properties` and every event response expose the same
selection metadata used by the HTML grid, so automation can query the editor
state without parsing SVG.

The HDL elaborator is now organized by responsibility rather than as one
monolithic implementation. `elab.rs` owns the public compile/elaborate entry,
top selection, generate flattening, package constant resolution, and `Env`;
`elab/types.rs` state/net/package/type models; `elab/substitution.rs`
parameter substitution; `elab/classes.rs` built-in UVM class collection;
`elab/arrays.rs` net/array/indexed-select helpers; `elab/modules.rs`
module/gate/interface/instance elaboration; `elab/statements.rs` procedural
statements and lvalues; `elab/expressions.rs` expression lowering;
`elab/functions.rs` function inlining and aggregate-state evaluation;
`elab/strings.rs` string methods; and `elab/class_elab.rs` the runtime class
object model. Elaborator tests live in `elab/tests.rs`; the public HDL API is
unchanged. The data-driven HDL integration cases are likewise split by domain
under `eda-tests/src/hdl_cases/` into lexer/parser/indexed-array structure,
packages/functions/aggregates, interfaces/properties, and class/UVM suites,
with `hdl_cases.rs` retaining only the stable registration facade.

The `eda-api` facade follows the same structure. `lib.rs` keeps the facade
imports, common compile/design helpers, `Flow`, and physical-timing option
types; `rtl.rs` owns RTL analysis/implementation/simulation APIs; `routing.rs`
placement/routing/interconnect APIs; `timing_flow.rs` physical-timing
iteration; `timing.rs` constraints/MMMC/waivers; `physical.rs`
SDF/SPEF/antenna/power/post-route IR; `integration.rs` waveform, DFT, ECO,
hold repair, CTS, and UPF helpers; `formal.rs` equivalence/BMC/safety wrappers;
and `cancellation_tests.rs` the cross-flow cancellation suite. Public API paths
remain unchanged through module re-exports.

The STA constraint implementation is split into typed constraint/state
definitions in `constraints.rs`, SDC tokenization and command parsing in
`constraints/parser.rs`, constrained slack analysis in
`constraints/analysis.rs`, and path reconstruction/reporting/object matching in
`constraints/paths.rs`, with focused tests under `constraints/tests.rs`.
The timing-violation lifecycle is likewise split into typed violation/store
models, stable-name dirty-object derivation, path extraction, scenario stores,
cross-corner registries, waiver serialization, and reports under
`violations/`. Likewise `eda_pnr::layout` now separates shared geometry primitives in
`layout/geometry.rs`, document/rule-deck/DRC/SVG behavior in
`layout/document.rs`, editor/session behavior in `layout/editor.rs`, and tests
in `layout/tests.rs`; public paths remain stable through re-exports.
The same cleanup now reaches the IR, AMS, simulator, STA core, and PnR core
facades. `eda-ir` separates types, single-layer mesh solving, multilayer
mesh/EM, UPF analysis, validation, and reports under `src/ir/`. `eda-ams`
separates typed circuit/device/analysis/result models under `src/types/`,
SPICE directive, element, model, utility, and writer parsing under
`src/parser/`, and its unit tests into core/device/statistical/dynamic/
derivative/optimization/advanced domains. `eda-sim` keeps the runtime engine in
`lib.rs` while tests are grouped into core, property, aggregate/interface, and
function/local-state domains. The STA core separates public timing types,
arrival propagation, library/device delay helpers, incremental updates,
analysis APIs, and path reports under `src/timing/`; the PnR core separates its
shared model, placement, global/maze routing, and routing/DRC checks under
`src/pnr/`. The large STA/API/AMS/HDL integration case files use stable
registration facades over domain-specific child modules, preserving every named
regression case and public Rust API path. The CLI similarly moves formal-proof,
timing/MMMC reporting, and physical/parasitic export helpers into focused
modules under `src/cli/` while retaining argument parsing/output handling in
`main.rs`; the command match itself is isolated in `cli/dispatch.rs` with a
typed read-only `RunContext`. The formal temporal-property implementation is
separated into typed result models, SAT encoding/unrolling, bounded-model
checking, report formatting, VCD replay/verification, and temporal semantics
under `eda-formal/src/property/`. HDL module elaboration is split into module
body, instance/gate/interface, and dynamic-array method domains. Interface,
SDC/MMMC, and class/UVM integration-case suites have additional domain-specific
child modules, while the CLI end-to-end tests are split into AMS, STA/PDK,
core/daemon, and physical-flow groups.
These splits improve maintainability but do not expand process coverage or
foundry certification beyond the behavior already described in this audit.

### Follow-up: parameterized generate elaboration

The HDL frontend now parses Verilog generate regions containing block, `if`,
`for`, and constant-selection `case` constructs with optional block labels.
Generate case supports multiple comma-separated labels, a default item, nested
generate loops, and first-match constant selection; an unmatched selector
without `default` is rejected. Elaboration resolves module
parameters first, evaluates generate conditions and loop bounds, expands each
iteration before port/net declaration, and substitutes the genvar through
expressions, ranges, assignments, processes, gates, and instance connections.
Focused tests cover for-loop expansion, both generate-if branches, multi-label
case selection, fallback, and unmatched/no-default rejection. The full
regression suite adds 100 parameterized generate-for cases spanning widths
1..100 and another 100 generate-case cases spanning exact/multi-label/default
selection, nested loops, simulation outcomes, widths 1..16, and invalid
unmatched selectors. This remains a targeted generate subset: generated-object
name mangling and more exotic generate forms are not yet complete.

### Follow-up: four-state wildcard case matching

The parser now distinguishes exact `case`, `casez`, and `casex`. Based binary
literals correctly map `?` to high-impedance wildcard bits. During elaboration,
wildcard cases lower to ordered conditional logic using per-bit case equality:
`casez` ignores Z in either operand while `casex` ignores X/Z in either operand.
The generated conditions preserve source-arm priority and default behavior for
both simulation and synthesis. One hundred regression cases rotate between
casez/casex and exercise all 16 four-bit selector patterns.

### Follow-up: SystemVerilog variable types

The frontend now parses packed `wire`, `tri`, `reg`, `logic`, `bit`, `byte`,
`shortint`, `int`, `integer`, and `longint` declarations in ANSI ports,
non-ANSI body declarations, and repeated port lists. Optional `signed` and
`unsigned` qualifiers are accepted before or after the type keyword. Explicit
ranges override default widths, while `byte`, `shortint`, `int`, and `longint`
receive their standard packed widths when no range is present. One hundred
generated cases cover all ten types, both signed-qualifier positions, explicit
and default widths, and variable/net classification.

### Follow-up: SystemVerilog parameter packages

The frontend now parses `package`/`endpackage`, `parameter`, and `localparam`
package constants, top-level and module-level imports, wildcard imports, named
imports, and `pkg::constant` scoped references. Package constants resolve
across packages, feed module-local parameter scope, and participate in ranges,
generate expressions, and expression elaboration. One hundred generated cases
vary package width/depth values and verify scoped references plus imported
constant widths.

### Follow-up: SystemVerilog enum typedefs

The frontend now parses `typedef enum` declarations with optional packed
ranges, explicit enum values, automatic successor values, wrapping auto-update
at the enum width, local module types, package types, wildcard type imports,
and named type imports. Enum constants enter local parameter scope for
expression elaboration, while custom declarations and ANSI ports receive the
resolved enum width. One hundred generated cases rotate local versus package
types, explicit versus default width, and wildcard versus named imports while
checking port/net widths and explicit constant values.

### Follow-up: packed struct typedefs

The frontend now parses `typedef struct packed` fields with built-in or
previously-defined type widths, computes the packed object width, records
field offsets in declaration/MSB-first order, and lowers `object.field`
selection for both lvalues and RHS expressions. Local and package struct
types work with wildcard and named imports. One hundred generated cases
rotate local/package types, wildcard/named imports, and field widths; every
case runs the simulator to verify high/low field extraction from a packed
object.

### Follow-up: packed union typedefs

The frontend also parses `typedef union packed`, sizes the union to its widest
field, overlays every member at offset zero, and supports local/package
definitions plus wildcard/named imports. Member selection works on both sides
of assignments and observes packed union aliasing. One hundred generated cases
rotate local/package definitions and import forms while running the simulator
to verify wide and narrow views of the same packed object.

### Follow-up: simple SystemVerilog functions

The frontend parses local and package `function automatic` declarations with
typed return ranges and input argument lists. A first safe subset supports
functions whose body assigns the function return name exactly once; call sites
are inlined by substituting actual arguments for formal names before expression
elaboration. Package functions are imported through wildcard or named package
imports. This preserves simulation and synthesis behavior without adding a
runtime call stack. One hundred local cases rotate
add/subtract/bitwise-and/or/xor and multiply across widths 1..16. Another
hundred package cases rotate add/subtract/AND/OR across widths 1..12; both
groups verify wrapped results in the simulator.

Non-recursive nested calls are now expanded recursively as ordinary
expressions. Actual arguments are first expanded in the caller context, then
substituted for formal arguments, and finally calls in the return expression
are expanded with the current function on an elaboration call stack. Re-entry
of an active function name is rejected, so sibling calls sharing a dependency
remain valid while direct or mutual recursion is rejected. One hundred
generated cases combine `add`, `increment`, and `pipeline` helpers with nested
actual arguments across widths 1..16 and verify wrapped arithmetic; a focused
unit test also checks mutual-recursion rejection.

Straight-line scalar function locals are now parsed and symbolically inlined in
assignment order. Locals may be zero-initialized by their declared width or
explicitly initialized, reassigned, passed through nested calls, and used in
the final return assignment. Unknown assignments, non-blocking assignments,
shadowing, and unsupported procedural control statements remain explicit
errors. One hundred generated cases alternate local/package functions and
wildcard/named imports across widths 1..16 while checking local initialization,
reassignment, nested helper calls, and wrapped simulation results.

Function `if/else` assignments are symbolically merged into conditional
expressions. Both branches are evaluated with separate local/return states,
changed values are merged as ternary expressions, and nested `else if` trees
compose recursively. Branch-local declarations, one-sided return assignment,
and non-blocking assignments remain explicit errors. One hundred generated
cases cover direct return branches, local-variable branches, nested `else if`,
one-sided local updates, local/package definitions, wildcard/named imports,
AND/OR/XOR/add expressions, both selector states, and widths 1..16.

Exact function `case` statements now merge branch-local and return states into
priority-ordered conditional expressions. Multiple labels are ORed, `default`
becomes the unmatched path, missing defaults preserve pre-case state, and `if`
inside case arms composes with the existing branch evaluator. `casez` and
`casex` labels lower through an internal wildcard-match node that reuses the
same four-state bit semantics as procedural wildcard case statements, rather
than incorrectly treating `?`/`x`/`z` as ordinary equality. One hundred exact
cases cover local and direct-return branches, multi-label arms, no-default
state preservation, nested `if`, selector widths 1..4, data widths 1..16, both
local/package definitions, and wildcard/named imports. Another hundred
wildcard cases rotate `casez`/`casex`, one-sided no-default updates,
priority-overlapping labels, AND/OR/XOR/add, all selector values, data widths
1..16, local/package definitions, and both import forms.

A conservative function `for` subset now unrolls during inlining. The parser
accepts canonical integer loops such as
`for (int i = 0; i < N; i = i + 1)`, while elaboration requires start,
condition, and step expressions to be constant, rejects shadowing and
non-advancing loops, and imposes a 256-iteration limit. Loop state reuses the
same symbolic local/return machinery, so loops compose with nested loops,
`if/else`, and exact `case`. One hundred generated cases cover zero-trip,
single, nested, and branch-containing loops across data widths 1..16 and
local/package functions with both import forms.

Constant-count function `repeat` statements also unroll through the same
symbolic state machinery. Repeat counts must be constant and bounded to 0..256;
nested repeats, zero-trip repeats, branch-containing bodies, local/package
definitions, and wildcard/named imports are covered by one hundred generated
simulation cases.

Bounded function `while` statements unroll when their condition remains constant
during symbolic elaboration, as with an integer loop counter. Conditions that
depend on runtime inputs are rejected, and execution is capped at 256
iterations. One hundred generated cases cover single, nested, zero-trip, and
branch-containing while loops with local/package functions and both import
forms.

Inline functions can now own packed struct and packed union locals. Field reads
lower to constant slices of the symbolic aggregate, field writes preserve the
other struct bits, assignment widths are normalized to the declared field and
function return widths, and union writes overlay the declared field. Function
formal arguments now preserve a shared packed range across names such as
`input logic [7:0] left, right`. One hundred generated cases exercise struct
construction, field reads, field arithmetic, conditional field updates, nested
helper calls, local/package types, wildcard/named imports, and widths through
20 bits; focused tests also cover union field aliasing.

Inline functions can also own fixed one-dimensional unpacked arrays such as
`logic [7:0] table [0:3]`. Element updates and reads use elaboration-time
constant indices, preserve declaration index direction, normalize element
width, compose with `for`, `if/else`, and case state merging, and use separate
element storage to avoid quadratic symbolic-concat growth. One hundred
generated cases cover loop-filled arrays, direct element writes, conditional
element updates, ascending/descending index ranges, element-width rotations,
array lengths 2..9, local/package functions, and wildcard/named imports.

Function locals now extend to fixed multidimensional unpacked arrays such as
`logic [7:0] table [0:1][1:0]`. Multi-index writes and reads preserve each
dimension's base/stride/length, use row-major element storage, and normalize to
the declared element width. They compose with nested bounded `for`, `if/else`,
conditional element updates, whole-array symbolic concatenation, local
functions, package functions, and wildcard/named imports. One hundred generated
cases vary 2x2 through 3x3 shapes, independent ascending/descending dimensions,
element widths 1..6, loop/direct/conditional updates, local/package definitions,
and simulation readback. Typed aggregate arrays and dynamic function arrays
remain unsupported.

SystemVerilog indexed part-selects `value[start +: width]` and
`value[start -: width]` are elaborated for conventional `[msb:lsb]` and reversed
packed vectors in read expressions. Conventional and reversed declarations work
in procedural blocking and non-blocking assignment targets; conventional
and reversed declarations also work in inline function read expressions.
Reversed declarations additionally support constant-base continuous-assignment
lvalues. Zero-delay scalar and unpacked-array element continuous lvalues accept
runtime bases, lowering to bounds-checked indexed processes that preserve all
fields outside the selected range; explicit nonzero delays on runtime indexed
continuous lvalues are rejected. Width must be
an elaboration-time positive constant. Constant starts lower directly to a bit span. Runtime starts are also
supported in module read expressions, procedural blocking assignment targets,
and procedural non-blocking assignment targets for both `+:` and `-:`. Reads
dynamically check bounds and return X when out-of-range; writes preserve the
target when the computed field is out-of-range. Non-blocking writes sample the
start/value in the active region and commit only the selected field in the NBA
region, so disjoint dynamic fields merge correctly. One hundred constant-base cases vary slice widths 1..8, starts across a
16-bit vector, `+:` and `-:` forms, direct/function/procedural lowering, and
ADD/XOR/AND/OR transforms. Another hundred runtime-base cases vary selector
inputs, both directions, widths 1..8, starts, and simulated slices. A third
hundred generated cases exercise runtime blocking writes to non-aligned fields,
both directions, preserved upper/lower bits, ADD/XOR/AND/OR values, and clocked
simulation. Inline functions now retain scalar declaration widths symbolically
and lower runtime indexed selects from formal arguments or local copies through
the same bounds-checked shift/mask/X form. A fourth hundred generated cases vary
direct/local-copy function bodies, local/package definitions, wildcard/named
imports, selector inputs, both directions, widths 1..8, starts, and simulated
results. A fifth hundred generated cases schedule two disjoint runtime-indexed
NBA fields in one clocked process, varying widths, offsets, initial values,
both write directions, and ADD/XOR/AND/OR values; focused tests also verify
read X propagation and write no-op behavior outside the range. A sixth hundred
cases synthesize reset-plus-two-runtime-indexed-NBA processes, check sequential
cell counts, and compare RTL simulation with gate-level simulation across
widths 1..4, offsets, both directions, and ADD/XOR/AND/OR values. A seventh
hundred reversed-declaration read cases vary bases `[0:15]` and `[2:17]`,
constant/runtime starts, `+:` and `-:`, widths 1..8, starts, transforms, and
simulation; focused tests verify out-of-range X propagation. Indexed selects on
dynamic arrays and zero/negative widths remain outside this subset. An eighth hundred reversed-declaration write cases vary blocking and
NBA forms, `[0:15]` / `[2:17]` bases, runtime selectors, both directions,
widths 1..8, starts, transforms, preserved field-external bits, and simulation;
focused synthesis also checks reversed runtime-indexed NBA RTL-versus-gate
behavior. A ninth hundred cases exercise reversed indexed reads in inline
functions, varying formal/local declaration bodies, local/package definitions,
wildcard/named imports, selector inputs, `[0:15]` / `[2:17]` bases, both
directions, widths 1..8, starts, transforms, and simulated results.
A tenth hundred reversed continuous-lvalue cases vary `[0:15]` / `[2:17]`
bases, both directions, widths 1..8, starts, transforms, assignment structure,
and selected-field simulation.
Module unpacked-array elements now retain their packed declaration bounds and
support `memory[i][base +: width]` / `memory[i][base -: width]`. Module reads,
module blocking/NBA indexed assignment targets, and zero-delay continuous
indexed lvalues may use runtime packed bases; array-element continuous lvalues
also accept runtime element indexes. Invalid element
indices are X for reads and no-ops for writes; valid blocking/NBA writes update
only the selected field and preserve the remainder of the element. An eleventh hundred
cases vary conventional/reversed packed elements, 1D/2D array declarations,
ascending/descending unpacked dimensions, runtime packed selectors, both
directions, element widths 4..8, slice widths, starts, blocking/NBA forms,
transforms, whole-element preservation, and simulation.
Inline-function unpacked arrays use the same element bounds metadata. Constant
and runtime array indexes can select a symbolic element for reads, runtime
packed bases bounds-check to X, constant-index writes preserve the remainder
of the element before a whole-element or selected-field return, and runtime
array-index writes update only the equality-selected candidate. A twelfth hundred
cases vary local/package functions, conventional/reversed elements, 1D/2D
arrays, ascending/descending dimensions, runtime selectors, both directions,
element widths 4..8, slice widths/starts, transforms, whole-element
preservation, and simulation.
Module runtime array-element reads lower each valid logical index tuple to an
equality-selected element mux before applying the packed indexed select. A
thirteenth hundred cases vary 1D/2D arrays, independent ascending/descending
dimensions, conventional/reversed packed elements, runtime element indices,
runtime packed bases, both directions, element widths 4..8, starts, and
simulation; focused tests also verify invalid element-index X propagation.
Function runtime element reads use the same logical-index equality mux before
symbolic packed selection. A fourteenth hundred cases vary local/package
functions, 1D/2D arrays, independent ascending/descending dimensions,
conventional/reversed packed elements, runtime element indices, runtime packed
bases, both directions, element widths 4..8, starts, and simulation.
Module runtime indexed assignment targets likewise enumerate legal logical index
tuples and lower each candidate to an equality-guarded constant-element
assignment. A fifteenth hundred cases vary 1D/2D arrays, independent
ascending/descending unpacked dimensions, conventional/reversed packed
elements, runtime element indices, runtime packed indices, both directions,
blocking/NBA forms, ADD/XOR/AND/OR transforms, and full flat-memory state
readback. A sixteenth hundred synthesis cases retain the same structural
coverage, check one DFF per physical memory bit, compare RTL with mapped gate
simulation, and alternate invalid-element-index and invalid-packed-base no-op
checks against reset state.
Runtime array-element continuous lvalues lower the legal index tuples to a
combinational equality-guarded process; the selected candidate receives a
constant-base indexed field update while other elements retain state. A
seventeenth hundred cases vary 1D/2D arrays, independent ascending/descending
dimensions, conventional/reversed packed elements, both indexed-select
directions, element widths 4..8, starts, transforms, RHS updates, selection
movement, invalid-index no-ops, and selected-element readback.
Function runtime indexed assignment targets apply the same logical-index
equality to every symbolic element. The matching candidate receives the packed
field update; invalid element indexes and invalid packed bases preserve that
invocation's initialized array state. An eighteenth hundred cases vary
local/package functions, 1D/2D arrays, independent ascending/descending
dimensions, conventional/reversed packed elements, runtime element indexes,
runtime packed indexes, both directions, ADD/XOR/AND/OR transforms, whole-array
readback, invalid-element no-ops, and invalid-packed-base no-ops.
Scalar runtime packed-base continuous lvalues lower to a combinational indexed
process. They preserve field-external state across RHS updates and selector
movement, reject out-of-range bases as write no-ops, and normalize nonzero
conventional declaration bounds for both dynamic reads and writes. A nineteenth
hundred cases vary conventional/reversed declarations with zero or nonzero
bases, `+:` / `-:`, widths 1..8, starts, transforms, repeated RHS updates,
whole-vector state, invalid-base no-ops, and selected-field X readback.

Modules can now declare fixed multidimensional unpacked net arrays such as
`logic [7:0] memory [0:3]` or `logic [7:0] memory [0:1][0:2]`. Elaboration
stores each object as one flat packed net while retaining unpacked index
base/stride/length metadata for every dimension; `memory[i]` and
`memory[i][j]` read or write the declared element-width slice and preserve
independent element state. Multi-index selection uses row-major element
layout, and every dimension may be independently ascending or descending.
Ascending and descending ranges, generate-for population, continuous
assignments, procedural blocking assignments, and conditional element
expressions are supported. One hundred one-dimensional cases vary element
widths 1..7, lengths 2..9, both range directions, continuous/procedural/
generate fill, conditional updates, concatenation readback, and simulation
results. Another hundred multidimensional cases vary 2x2 through 3x3 shapes,
independent dimension directions, AND/OR/XOR/add element expressions, widths
1..4, row-major concatenation readback, and simulation results.

ANSI module ports can use the same fixed unpacked-array declaration subset:
`input logic [7:0] in_mem [0:3]` and multidimensional forms such as
`output logic [7:0] out_mem [0:1][1:0]`. Non-ANSI header/body declarations are
merged before elaboration, so a plain `mem` header can be refined by
`input logic [7:0] mem [3:0];`. Each array port is represented as one
flat packed net with element-width and per-dimension index metadata, so element
assignments/selects work exactly as they do for module nets and an instance can
connect the complete array port to a parent vector of the same total width with
named or positional connections. Module instance-array expansion uses the same
total width, so each expanded child can receive/drive its row-major slice of a
wider parent vector. Module arrays follow the current 4096-element/one-million-bit
flat budget and row-major layout (small inline-function arrays retain the older
tighter elaboration bound). One hundred generated cases cover one-dimensional and
two-dimensional ports, widths 1..8, ascending/descending dimensions, add/XOR/
AND/OR element transforms, both connection styles, child hierarchy widths, and
simulation readback. Dynamic array ports and arbitrary aggregate port assignment
rules remain outside this subset. A
focused test additionally verifies non-ANSI array/body declaration merging,
top-level port width propagation, and module-instance-array slice composition.

One-dimensional fixed unpacked array slices can now be used directly in instance
port connections. `source[1:2]` and `result[2:3]` lower to contiguous flat-bit
spans while retaining declaration direction; input slices feed a child array
port and output slices receive one. Both named and positional connections work,
and slices that reverse a dimension's declared direction are rejected. One
hundred generated cases vary element widths 1..8, source lengths 4..8, slice
lengths 2..4, independent source/result directions, distinct input/output
storage offsets, add/XOR/AND/OR transforms, and simulated whole-array readback.
Multidimensional unpacked slices, slices with non-unit stride, dynamic arrays,
and arbitrary aggregate assignments remain unsupported.

The frontend now supports a foundational SystemVerilog interface subset.
Interfaces can declare parameters and scalar/vector signals, top modules can
instantiate them, modules can declare `interface name.modport object` ports,
and named instance connections bind those objects to a shared interface
instance. Inside a module, `object.signal` references elaborate directly to the
shared flat net rather than to a copied port, preserving producer/consumer
connectivity through simulation. Interfaces can declare explicit modports such
as `modport master(output valid, data);`. Selected modports are validated,
signals not exposed by the modport are rejected, input interface signals cannot
be assigned, and output-only interface signals cannot be read. One hundred
generated producer/consumer cases cover two directed data signals, valid
gating, conditional interfaces, data arithmetic/XOR, widths 1..16, and simulated
end-to-end results; focused tests reject output reads and input writes.

Fixed one-dimensional interface instance arrays are also supported:
`bus shared[0:3]();` elaborates one interface hierarchy element per index, and
module connections such as `.b(shared[1])` bind that element's signal set.
Both ascending and descending declaration ranges work, selected elements keep
independent flat nets, and modport direction checks apply per connected object.
One hundred generated cases cover array lengths 2..5, both range directions,
distinct first/last element selections, producer/consumer pairs, widths 1..16,
hierarchy element checks, and simulated isolation between elements.

Fixed multidimensional interface arrays are supported as well, including
ascending or descending ranges on each dimension and connections such as
`.b(shared[1][2])`. Shape, stride, and row-major element indexing are resolved
during elaboration; each element receives independent flat nets and hierarchy
metadata. The current guard accepts up to 256 elements per dimension and 4096
total elements. One hundred generated cases vary 2x2 through 3x3 shapes,
direction combinations, distinct element selections, producer/consumer pairs,
widths 1..16, hierarchy count, and simulated element isolation.

Fixed multidimensional module instance arrays are now elaborated as separate
child instances. Named or positional vector connections are sliced by child
port width in source array order: `leaf u[0:3](.a(a), .b(b), .y(y))` feeds
each element from the corresponding slice of `a` and `b` and drives the
matching `y` slice. Multiple dimensions use row-major source order, so
`u[0:1][0:2]` expands `(0,0), (0,1), (0,2), (1,0), ...`, and packed vector
connections follow that same order. A scalar-width input may be reused by every
element, while output connections must provide the full concatenated vector.
Each dimension supports ascending/descending ranges up to 256 elements and the
total is capped at 4096; every element receives its own hierarchy record and
flat nets. One hundred one-dimensional cases vary element widths 1..8, lengths
2..5, direction ranges, named/positional connections, AND/OR/XOR/add children,
and simulated slice isolation. Another hundred multidimensional cases vary 2x2
through 3x3 shapes, independent dimension directions, row-major slice mapping,
named/positional connections, child widths 1..4, and simulation results.

Static virtual interface ports are supported in the form
`virtual interface bus.master object`. They reuse the same interface-object
binding, flat-net member references, and modport direction checks as concrete
interface ports. This is deliberately a static subset: class handles, procedural
rebinding, null handles, and virtual interface arrays remain unsupported. One
hundred generated producer/consumer cases vary source/sink modports, two data
signals, valid gating, arithmetic/XOR, widths 1..16, and simulated results.

Procedural immediate assertions are now parsed, elaborated, and executed by the
event simulator. `assert (condition) else $error("message")` carries the message
into IR; false, X, or Z conditions return a timestamped simulation error, while
true conditions continue execution. A foundational concurrent subset also
lowers both `|->` and one-step `|=>` implications to clock-edge processes.
Overlapping checks sample the antecedent and consequent at the same edge;
non-overlapping checks store the antecedent in a hidden NBA-updated pending net
and test the consequent at the next sampled edge. One hundred immediate cases
exercise `<`, `>`, `==`, and `!=`
at widths 1..16, including 25 expected custom-message failures. Another hundred
clocked cases alternate posedge/negedge, logical antecedents, vector equality,
pass/fail outcomes, custom messages, and widths 1..16. A further hundred
next-sample cases verify that a false consequent is ignored at the antecedent
edge, then checked at the following edge. `assume property` uses the same
sampled implication machinery and rejects stimulus that violates an assumption,
while labeled `cover property` checks record named hit counts exposed by
`Simulator::property_cover_hits` and `take_property_covers`. One hundred
generated kind cases alternate assumption pass/failure and cover hit/miss at
posedge/negedge with vector equality widths 1..16.

Fixed-cycle temporal delay is supported in the form
`antecedent |-> ##[n] consequent` for constant `n` from 0 through 256.
Bounded ranges `antecedent |-> ##[a:b] consequent` are also supported: the
consequent may match at any sampled edge from `a` through `b`, a successful
attempt retires, and an assertion/assumption fails only if no match occurs by
the final edge. Elaboration lowers attempts to sampled shift-register state and
composes both forms with assert, assume, and cover properties. One hundred
fixed-delay cases vary delays 0..7, posedge/negedge, vector width 1..16,
assertion pass/failure, assumption failure, and cover hit/miss. Another hundred
range cases vary start/end windows, matches at window boundaries, no-match
expiration, assumption violation, cover hit/miss, and both clock edges.
Bounded consecutive repetition is supported as
`antecedent |-> consequent[*n]` for constant `n` from 1 through 256. The
consequent must be true on the antecedent sample and the following `n-1`
samples; an assertion or assumption fails when an active attempt observes a
false/unknown value, while cover records completion after the final successive
sample. Elaboration uses age-indexed sampled state so overlapping attempts
remain distinct. One hundred generated cases vary repetition 1..5,
posedge/negedge, vector width 1..16, assertion pass/failure, assumption
failure, and cover hit/miss. Bounded repetition ranges
`consequent[*a:b]` are also supported for constant `1 <= a <= b <= 256`; the
existential range can match once the minimum consecutive count is reached, and
the accepted maximum is validated. Another hundred generated cases vary both
bounds, early matching, interruption before the minimum, assumption failure,
cover hit/miss, both clock edges, and vector widths 1..16. Unbounded repetition,
clocking blocks, recursion, and formal action semantics remain future work.

Bounded two-sequence composition supports same-delay and fixed-delay forms:
`left and right`, `left or right`, `left ##[n] right`,
`left and ##[n] right`, and `left or ##[n] right`. The left expression is
sampled at the scheduled consequent edge; a delayed right expression is sampled
`n` further edges later. `and` requires both alternatives, while `or` requires
either. This composes with the leading `##[n]` scheduling and `|=>`
next-sample form, assert, assume, and labeled cover. Composition with
repetition operators and composition delay ranges are rejected until richer
sequence state is implemented. One hundred same-delay cases vary `and`/`or`,
delays 0..3, `|->`/`|=>`, posedge/negedge, vector widths 1..8, assertion
pass/failure, assumption failure, and cover hit/miss. Another hundred
fixed-delay cases vary sequential `##`, `and`, and `or`, right delays 1..4,
leading delays 0..2, `|->`/`|=>`, both clock edges, vector widths 1..8,
assertion pass/failure, assumption failure, and cover hit/miss.

Bounded goto repetition is supported as
`antecedent |-> consequent[->n]` for constant `n` from 1 through 256. The
consequent may be false between counted samples; the property can complete when
the nth nonconsequent-true sample arrives. Lowering uses count-indexed sampled
state, `disable iff` clears that state, and the subset composes with assert,
assume, and labeled cover. The current implementation merges overlapping goto
attempts occupying the same count bucket, which preserves existential
assertion/assumption behavior but does not promise one cover hit per distinct
overlapping antecedent. One hundred generated cases vary repetition 1..5,
hit and miss patterns, posedge/negedge, vector width 1..16, assertion failure,
assumption pass, and cover hit/miss.

Bounded goto ranges `consequent[->a:b]` are supported for constant
`1 <= a <= b <= 256`. In this terminal-property subset the existential range
can match once the minimum nonconsecutive count is reached; the maximum is
validated and preserved for future sequence composition. Another hundred
generated cases vary both bounds, minimum matching, post-match retirement,
pre-minimum miss patterns, assertion failure, assumption pass, cover hit/miss,
both clock edges, and vector widths 1..16.

Bounded nonconsecutive repetition `consequent[=n]` and
`consequent[=a:b]` is supported for constant `1 <= a <= b <= 256`. As with the
terminal goto subset, arbitrary false gaps are allowed, the attempt can match
when the minimum count is reached, and later occurrences do not invalidate a
matched attempt. It reuses the count-indexed sampled state and disable-iff
abort machinery. One hundred generated cases alternate exact counts and ranges,
vary repetition bounds, post-match occurrences, hit and miss patterns,
assert/assume/cover, both clock edges, and vector widths 1..16.

Concurrent properties accept `disable iff (condition)` after the clocking
event. A disabled sample skips the check and clears hidden delay/range/repetition/goto
attempt state, aborting overlapping attempts rather than merely suppressing one
error message. This composes with `|->`, `|=>`, fixed/range delays, bounded
repetition, assert/assume, and labeled cover. One hundred generated cases vary
those forms, posedge/negedge, vector width 1..16, disabled failure/hit edges,
and fresh enabled attempts after re-enabling.

Signed arithmetic now carries signedness through declarations, references,
sized signed literals such as `8'sh80`, `$signed`, and `$unsigned`. Simulation
uses Verilog operand signedness propagation, signed comparisons, add/subtract,
multiply, divide, modulo, and arithmetic right shift; division by zero remains
X, and synthesis intentionally rejects division/modulo rather than lowering an
incorrect circuit. One hundred generated cases cover signed declarations,
positive/negative operands, widths 2..16, wrapping add/subtract, signed
minimum selection, explicit signed-literal casts, truncating division, and
negative remainders.
Signed synthesis now preserves operand signedness during sizing, sign-extends
narrower signed operands, lowers signed comparisons by sign-bias mapping,
performs arithmetic right shifts, and maps `$signed` casts into the same
RTL-versus-gate cone. Another hundred generated cases cover add/subtract/
multiply, all ordered comparisons, equality/inequality, arithmetic shifts,
mixed-width signed addition, signed conditional selection, widths 2..8,
positive/negative operands, and mapped-gate equivalence. Division/modulo
synthesis remains intentionally rejected.

### Follow-up: generated clocks and clock groups

The SDC parser now supports `create_generated_clock` with `-source`,
`-divide_by`, `-multiply_by`, and generated output ports. Parent clocks are
resolved either by clock name or by a source port, generated period is derived
from the parent, and source/network latency is inherited. `set_clock_groups`
supports asynchronous, logically-exclusive, and physically-exclusive groups with
multiple `-group` access lists. Typed queries return the relationship between
two clocks, and constraint diffing marks clock-group changes as requiring
constrained timing recomputation.

The regression suite adds 100 generated-clock divide-by cases and 100 clock-group
relationship cases rotating across all three group kinds.

Timing exceptions now honor `-setup` and `-hold` scoping. A false path or
multicycle path without a check flag remains applied to both classes. With
`-setup`, it suppresses/modifies late setup and recovery checks without
relaxing early hold/removal analysis; with `-hold`, the reverse is true. The
typed `PathExceptionCheck` is public through the STA and facade APIs, reports
label the selected class, and excluded-path audit entries only contain the
checks actually waived.

The parser now also accepts `set_units` with time and capacitance dimensions.
Subsequent clock periods/edges, uncertainty, latency, transition, IO delay,
max/min delay, transition limit, input transition, capacitance limit, and load
values are normalized into the canonical ns/pF representation. Resistance,
voltage, and current unit metadata is retained in the typed `SdcUnits` record,
duplicate/unknown dimensions fail closed, and commands before a later
`set_units` use the unit state active at that line.

The SDC parser also supports design-wide and object-scoped signal-quality
limits: `set_max_transition`, `set_max_capacitance`, and `set_max_fanout`.
It additionally accepts typed `set_input_transition` and `set_load` rules.
`set_clock_transition` stores independent minimum and maximum rise/fall values
on root and generated clocks and injects both bounds into the clock source net.
The maximum transition feeds late setup/recovery analysis; the minimum feeds
early hold/removal analysis. Downstream clock-buffer transition propagation
uses the corresponding min/max transition cone, so indexed Liberty constraint
lookups consume the SDC values by check polarity.
Object lists accept typed ports, nets, cells, and output pins, including
wildcard selectors. Values are validated, preserved in typed
`TimingConstraints`, exposed through SDC diffing with
`quality_recompute_required`, and converted into `QualityOptions`; the last
matching scoped rule overrides the design-wide fallback during quality
analysis. Input transitions apply to primary-input ports/nets or specific
cell input pins before arc propagation; pin and port/net loads contribute to
driven-net capacitance. One hundred generated cases vary tight and relaxed
design-wide limits
and fanout bounds 1..16, verify typed round-trip and incremental quality
marking, and compare predicted violation presence against mapped-netlist
quality analysis. Another hundred cases rotate exact port, exact net, wildcard
cell, and wildcard output-pin selectors, vary tight/relaxed scoped transition,
capacitance, and fanout values, and verify scoped precedence over relaxed
design-wide fallbacks. A further hundred cases rotate input-transition
port/net/pin selectors and load port/net/output-pin selectors, verify duplicate
object overwrite, tight/relaxed transition and load values, diff marking, and
mapped-netlist transition/capacitance predictions. Clock and cell objects,
flags in place of trailing object lists, wire-load models, and non-quality
timing accessors remain rejected.
A hundred clock-transition cases vary root/generated targets, no/rise/fall/
min/max flags, transitions 0.10..0.19, typed clock fields, SDC diff marking,
source-net min/max transition, and downstream inverter transition propagation.
Another hundred cases store distinct minimum and maximum clock transitions,
then compare full analysis against max-only setup/recovery references and
min-only hold/removal references over indexed Liberty LUTs.

### Follow-up: bounded regression flow cache

`eda_api::flow_cache` adds deterministic content-addressed keys over RTL source,
selected top, and a canonical fingerprint of the synthesis library's cells,
functions, arcs, timing tables, sequential definitions, and physical/electrical
metadata. The typed cache is bounded, LRU, records hits/misses/inserts/evictions,
and can wrap the complete RTL implementation flow. `SharedFlowCache` serializes
concurrent agents behind a mutex while returning cloned flow results. The suite
adds 100 source-specific hit/miss cases plus invalid-source and eight-thread
concurrency checks. This closes the local bounded cache layer; a network-backed
distributed cache remains future work.

The persistent daemon now owns the same `SharedFlowCache` across TCP clients
and async jobs. New `rtlFlowCache`, `flowCacheStats`, and `flowCacheClear`
methods return the complete structured RTL flow plus `cacheHit`/`cacheId`;
`jobSubmit` executes cached flows through the service-owned dispatcher rather
than a stateless global path.

The daemon source is organized by responsibility rather than as one monolithic
implementation: `protocol.rs` owns JSON/request wire types, `service.rs` the
shared service facade, `jobs.rs` local/remote scheduling and artifacts,
`server.rs` line/TCP lifecycle, and domain handlers live in `analysis.rs`,
`implementation.rs`, `layout.rs`, `physical.rs`, `physical_timing.rs`,
`timing.rs`, and `verification.rs`. Integration tests mirror those domains
under `src/tests/`, keeping the public crate API unchanged.

The async job manager now uses a bounded local worker pool (four workers by
default, configurable at service construction) instead of one execution thread.
Workers share the flow cache and persistent timing/layout waiver registries,
preserve queued/running cancellation semantics, and shutdown deterministically.
New `jobWorkerStats` returns worker count, queued/running/completed/failed/
cancelled counts, and active job IDs.

Cross-host execution is also RPC-addressable. `jobSubmitRemote` reserves a job
for an external worker rather than enqueueing it locally. It accepts a positive
`maxAttempts` budget, defaults to three, and returns the limit with the job ID.
It also accepts a non-negative `priority`; larger values are claimed first,
the default is zero, the value survives lease reassignment, and `workerClaim`
echoes it. Remote agents
register with `workerRegister`, refresh `workerHeartbeat`, list peers with
`workerList`, pull work with `workerClaim`, observe cooperative cancellation
through `workerClaimStatus`, and return typed success/failure with
`workerComplete`. Claims are owner-checked, respect remote capacity, integrate
with `jobStatus`/`jobWait`/`jobCancel`, and appear in `jobWorkerStats` as
remote active jobs.

Workers may register string metadata labels such as pool, architecture, tool
version, or license class. A remote submission can require an exact subset of
those labels with `requiredWorkerLabels`; claims are then routed only to
matching workers, the constraint survives lease reassignment, and the claim
response echoes it. `workerList` exposes each peer's labels so agents can
inspect the discovered fleet. Network-level peer discovery and object storage
remain external at this stage.

Each remote claim owns a timestamped lease with a positive per-worker timeout
(60 seconds by default). Registration, heartbeat, and claim renew the lease;
`workerExpireLeases` explicitly requeues expired requests, while every
`workerClaim` first performs the same check so another healthy worker can take
over stale work. Reassignment removes the old running token and ownership
before restoring the queued request, counts the transition in
`jobWorkerStats.remoteReassignments`, and rejects late completion from the
previous owner. The same expiration pass prunes idle workers whose
registration heartbeat has exceeded their lease timeout; active workers with a
live claim are retained, removed IDs are returned by `workerExpireLeases`, and
`jobWorkerStats.remoteWorkerExpirations` counts fleet cleanup. The daemon does
not bundle network object storage or an automatic worker fleet; those remain
future work. When an expired lease has
already consumed the attempt budget, the job transitions directly to a typed
failed state instead of being requeued; `jobWorkerStats.remoteRetryExhausted`
counts that terminal condition and the claim response exposes
`attempt`/`maxAttempts`.

Remote completion now accepts string and binary result artifacts alongside
success or failure. Each artifact requires a unique non-empty name. UTF-8 is
the default encoding; canonical padded base64 can carry GDS/OASIS, FSDB, or
other binary tool output. Every artifact records its content type, encoding,
decoded byte size, and SHA-256 checksum of the decoded bytes. `jobArtifactList`
exposes metadata only and supports bounded `offset`/`limit` pagination,
`jobArtifactGet` returns transfer content and its checksum, and
`jobArtifactDelete` removes one artifact. A completion accepts at most 64
artifacts and 8 MiB of decoded payload, fail-closed before the claim is
released. Artifacts are persisted only after the owner-checked claim is
finalized: failed jobs may retain diagnostics, cancelled jobs retain none, and
a stale owner cannot write them. The whole service registry is additionally
bounded to 512 artifacts and 64 MiB; when a new completion exceeds that budget,
the least-recently-fetched artifact from another job is evicted first.
`jobWorkerStats` reports current count/bytes, both capacities, and cumulative
evictions.

The daemon can also use a local persistent artifact store. `eda daemon
--artifact-store <dir>` (or the typed service constructor) creates the
directory, atomically publishes one checksum-validating JSON record per
terminal remote job, rewrites the record after artifact deletion, and reloads
terminal job state plus artifacts on restart. A separate atomically published
job-ID watermark prevents a new daemon instance from reusing an ID allocated
before a crash even when that job never reached a terminal record. Loaded
artifacts are still subject to the in-memory global LRU budget. This is a
single-daemon local object store, not yet concurrent network object storage.

### Follow-up: safe sequential ECO replacement

Structural ECO replacement no longer rejects every sequential instance. A
generic `Dff`/`DffR` can be replaced by a library `DFF`/`DFFR` when the clock,
data, and reset pin roles and input count match exactly. Mixed
sequential/combinational replacements and changes such as DFF-to-DFFR remain
rejected transactionally, preventing an ECO from silently moving a reset or
clock connection. Unit tests cover positive DFF/DFFR swaps and both negative
role/class cases; the named integration case also proves the swapped DFF is
formally equivalent to the original netlist.

### Follow-up: DC solver cancellation reaches Newton iterations

The AMS cancellation surface now includes the two previously synchronous DC
entry points. `dc_operating_point_cancellable` checks before validation and at
every Newton iteration; `dc_sweep_cancellable` checks before every sweep point
and inside each Newton solve. The typed API dispatcher routes `.op` and `.dc`
through those paths, and daemon `spiceDc` jobs use the same token. Unit tests
cover direct AMS calls, API dispatch, and RPC cancellation, while transient
integration now inherits the same per-iteration checks.

### Follow-up: LVS and UPF power-intent RPC surfaces

The persistent daemon adds two signoff-facing methods. `rtlLvs` accepts two
RTL views (with a shared or per-view top), runs the cancellable implementation
flow on each, performs labelled graph LVS/ERC, and returns structured cell/net
matches, shorts, opens, notes, both netlist statistics, and the text report.
`rtlPowerIntent` compiles RTL, parses UPF, checks domain/supply/switch/
isolation/retention/level-shifter intent, and returns typed counts, findings,
an error verdict, and report text. Both methods are available to direct line
RPC and asynchronous `jobSubmit` workers and honor the shared cancellation
token before and after each expensive stage.

### Follow-up: formal safety and physical-timing RPC surfaces

Two additional signoff-facing daemon methods close API-first gaps. `rtlSafety`
runs the cancellable RTL implementation flow, selects a named bad net and bound
`depth`, invokes bounded reachability plus k-induction, and returns status,
depth, inductiveness, counterexample/failed-invariant flags, mapped statistics,
and the proof report. `rtlPhysicalTiming` parses track/iteration/pass and
interconnect R/C options, runs placement → routing → RC extraction → SDF
back-annotation → STA, and returns baseline/best delays, iteration and
wirelength data, RC/DRC counts, the serialized SPEF and SDF, mapped statistics,
and the full physical-timing report. Both methods are wired into direct RPC
and cancellable asynchronous jobs.

### Follow-up: RTL antenna repair API and RPC

The unified API now wraps the complete physical antenna loop:
RTL compile → library synthesis → placement → detailed maze routing →
physical verification → DIODE insertion → placement extension → rerouting →
post-repair verification. The typed result retains the baseline DRC evidence,
per-net old/new antenna ratios and diode counts, final placement/routing,
mapped statistics, and the human-readable repair report. Cancellation is
checked around compilation, synthesis, placement, routing, and final assembly.
Daemon method `rtlAntennaRepair` exposes the same flow with patch-name,
diode-limit, track-capacity, and congestion-policy options; it returns typed
baseline DRC, repair records, final physical statistics, and the report, and is
available to direct RPC and cancellable async jobs.

The antenna threshold itself is now a typed DRC option rather than a hidden
global constant. `drc_with_options`, `physical_verification_with_options`, and
`implement_rtl_pnr_with_drc_options` accept a finite positive
`max_antenna_ratio`; invalid limits fail closed. Daemon `rtlPnr` accepts
`drcOptions.maxAntennaRatio`, so a foundry-specific ratio can drive the same
LEF/DEF, placement, routing, and structured physical-verification response
without changing tool source.

### Follow-up: arbitrary-angle routing RPC

`layoutAnyAngleRoute` exposes the typed exact-geometry two-pin router to
remote agents. Requests supply one or more net/start/end pairs, optional
simple-polygon blockages, and an integer minimum-spacing rule. The method
returns exact rational-squared segment lengths, floating lengths for clients,
total wirelength, route/blockage counts, and the applied spacing. It rejects
blockage crossings, net crossings, spacing violations, malformed geometry, and
empty request lists, and is available through both direct RPC and cancellable
asynchronous jobs.

### Follow-up: arbitrary-angle maze routing RPC

`layoutAnyAngleMazeRoute` exposes the bounded visibility-graph arbitrary-angle
router to remote agents. It accepts one or more two-pin requests, simple
polygon blockages, integer minimum spacing, and a bounded search-node limit.
The response returns every route point, segment count, exact rational-squared
segment lengths, per-route and total lengths, blockage count, and the applied
options. With `ripgup` enabled and a positive `maxPasses`, the RPC invokes
negotiated arbitrary-angle rip-up/reroute and additionally returns the typed
initial conflict list and convergence pass count. Direct RPC and cancellable
asynchronous jobs share the same path.

### Follow-up: arbitrary-angle Boolean RPC

`layoutAnyAngleBoolean` exposes the exact two-polygon geometry engine to remote
agents. It accepts two simple integer-coordinate polygons and `union`,
`intersection`, or `difference`, and returns every result loop with exact
rational vertex numerators/denominators, floating coordinates for convenience,
per-loop area and signed area, loop count, and total oriented area. Rational
intersections remain exact rather than being rounded to integer database units;
for example the RPC preserves a `13/5` coordinate. Invalid operations and
malformed/self-intersecting polygons fail closed, and the method is available to
direct RPC and cancellable asynchronous jobs.

### Follow-up: two-view RTL equivalence RPC

`rtlEquivViews` compares two independently supplied RTL/ECO views rather
than only checking one RTL implementation against its own mapped netlist. It
runs the cancellable implementation flow for each source/top pair, compares
the stable unoptimized reference views with sequential LEC, and returns the
equivalent verdict, binary-string counterexample, proof notes, and per-view
net/cell statistics. This gives remote agents a direct regression/ECO API for
before-and-after design versions.

### Follow-up: coverage-plan validation and crate-local coverage tests

Functional coverage parsing now rejects malformed plans at parse time instead
of deferring structural failures to waveform evaluation. Duplicate covergroup,
coverpoint, bin, cross, and toggle names are rejected; empty covergroups and
empty plans are rejected; crosses must reference distinct existing
coverpoints. Crate-local tests grew from two to eight and now cover posedge
and negedge sampling, exact bin/cross hit counts, 100% percentage accounting,
whole/bit/slice selectors, non-contiguous bin ranges, report formatting,
missing signal/clock fail-closed behavior, and all duplicate/empty-object
errors.

### Follow-up: expanded crate-local CTS tests

`eda-cts` crate-local tests grew from two to seven. Direct checks now cover a
nine-sink balanced tree with bounded leaf fanout, preserved sequential count,
inserted buffers and finite delay/skew metrics; separate clock roots producing
separate trees; placement-aware CTS with a legal physical view and finite
physical skew/wire delay; all CTS and timing-driven option bounds; a
sequential-free netlist rejection; and a placement/netlist cardinality
mismatch rejection.

### Follow-up: DFT scan option validation and expanded tests

Scan insertion now rejects blank chain names and duplicate scan-control port
names in addition to existing-name, missing-library, and sequential-input
errors. Crate-local DFT tests grew from two to five. They now verify a
three-flop chain's exact order, inserted mux/buffer and net counts, every
mux's functional/previous-Q connectivity, functional versus shift selection,
scan-out observation, DFT analysis counts, report output, malformed options,
and structural audit rejection of empty or non-MUX chains.

### Follow-up: fault-pattern bounds and expanded fault tests

Automatic fault-pattern generation now rejects fewer than two random patterns
and an exhaustive-input limit above 24 bits, preventing accidental memory
explosion and shift overflow. Crate-local fault tests grew from two to seven,
covering exact exhaustive pattern order, exact single-pattern stuck-at
coverage and detection indices, deterministic reproducible random generation
beyond the exhaustive limit, invalid options, sequential/no-input/no-output
fail-closed behavior, missing pattern inputs, invalid STIL names/patterns, and
malformed or unknown STIL signals.

### Follow-up: SPEF unit conversion and structural validation

SPEF serialization now converts capacitance and resistance values from the
in-memory unit system into its canonical NS/PF/OHM output units; previously a
non-default-unit SPEF changed physical quantities on write/read round trip.
Writer validation rejects nonpositive/non-finite units and non-finite
parasitics, parser validation rejects negative/non-finite capacitance and
resistance, invalid total capacitance, duplicate capacitor/resistor IDs, and
empty parasitic nodes. Quoted escaped design/net/node names and connection
coordinates now round-trip. Crate-local tests grew from three to nine, adding
exact non-default-unit conversion, escaped identifiers, parasitic report
totals/unmatched nets/worst RC, SDF INTERCONNECT export, and malformed
parser/writer rejection coverage.

### Follow-up: LVS structural validation and expanded tests

Both reference and layout views are now validated before graph comparison:
empty or duplicate port/net/cell names are rejected, and combinational cycles
fail closed rather than entering signature refinement. This closes a port-map
ambiguity where duplicate port names could collapse through a hash map.
Crate-local LVS tests grew from four to ten, adding exact cell/net mappings
and report contents; count/name/direction/width port mismatches; pin/connectivity
mismatch; sequential-boundary matching with a renamed clock; unloaded input
and undriven output ERC checks; and duplicate-name/combinational-cycle
rejection.

### Follow-up: UPF domain-consistency checks and expanded tests

UPF parsing now rejects repeated domain elements and supply-port directions
outside `in`, `out`, and `inout`. Power-intent checking additionally enforces
domain consistency for a domain's primary supplies, every supply-net/port
connection, and each switch input/output supply. A supply port connected to
multiple nets is reported as an error. The human report now starts with an
explicit PASS/FAIL verdict. Crate-local UPF tests grew from four to seven,
adding a clean end-to-end intent over a constructed design, cross-domain
supply/shared-port errors, and top/element/signal check failures while
retaining switch, mode, and strategy-current coverage.

### Follow-up: RTL MMMC timing RPC

The daemon adds `rtlMmmcTiming`, exposing the existing typed
multi-scenario timing API to remote agents. It accepts RTL, an optional top,
and an inline named scenario manifest with SDC constraints and generic
properties; runs the cancellable implementation flow; analyzes every scenario
against the mapped netlist; and returns scenario count, worst scenario,
worst setup/hold slack, per-corner clock/period/delay/slack data, all generic
properties, mapped statistics, and the full MMMC report. Unconstrained
infinite slacks serialize as JSON null rather than failing the response.
The method is available synchronously and through cancellable async jobs.

### Follow-up: timing scenario diff RPC

The daemon adds `timingScenarioDiff` so agents can compare two inline MMMC
manifests before rerunning analysis. It parses both scenario sets, computes
added/removed/changed/timing-changed scenarios, and returns per-scenario SDC
object diffs, generic property additions/removals/changes, recompute
classification flags, scenario counts, and the text report. This distinguishes
a power-only property edit from a timing constraint edit and is available
through both direct RPC and cancellable asynchronous jobs.

### Follow-up: incremental MMMC timing RPC

`rtlIncrementalMmmcTiming` executes a complete remote incremental timing flow:
parse old and new scenario manifests, run the cancellable RTL implementation
flow once, analyze the old scenario baseline, and invoke the typed incremental
MMMC engine. The response reports baseline/final worst scenarios and setup/hold
slacks, added/removed/changed/timing-changed scenarios, reused scenario names,
per-update recomputed nets/cells and recompute classification, final per-corner
summaries, text diff/final reports, and mapped statistics. Property-only edits
reuse the old STA object while timing constraint edits execute the incremental
constraint engine.

### Follow-up: RTL signal-quality RPC

`rtlSignalQuality` exposes transition, capacitance, and fanout checks to
remote agents. It parses design-wide limits, object-scoped transition/
capacitance/fanout rules, input transitions, pin/port loads, and per-net extra
capacitance; runs the cancellable RTL implementation flow; and returns check
counts, worst transition/capacitance slack, typed violations with net/cell IDs
and names, actual/limit values, mapped statistics, and the text report. The
method is available synchronously and through cancellable asynchronous jobs.

### Follow-up: timing waiver audit RPC

`timingWaiverAudit` gives remote agents a signoff-review view of immutable
waiver history. It parses the serialized audit stream, reconstructs the active
waiver set, and returns typed event and rule snapshots, actor/generation
metadata, active waiver IDs, hierarchy-scoped event distributions and latest
generations, plus both human-readable audit reports. Direct RPC and async jobs
share the same cancellation token.

### Follow-up: MMMC setup/hold/reset path RPC

`rtlMmmcTimingPaths` exposes merged cross-corner path review to remote agents.
After the cancellable RTL flow and multi-scenario timing analysis, it extracts
setup/output, hold, and recovery/removal paths and serializes endpoint/scenario
metadata, slack/required/arrival values, launch and endpoint objects, cell/net
IDs and stable names, every timing arc with delay, and per-corner slack
vectors. Separate setup, hold, and reset reports are returned alongside the
typed path arrays.

### Follow-up: MMMC timing-violation registry RPC

`rtlMmmcTimingViolations` turns the typed cross-corner violation registry into
an agent-facing signoff review operation. It accepts RTL, an optional top, an
inline named timing-scenario manifest, serialized waivers, and an audit actor;
runs the cancellable implementation flow; and returns active and waived
violations per scenario, active waiver snapshots, actor-attributed audit
reports, cross-corner endpoint summaries, hierarchy distributions, the full
registry report, and mapped statistics. Matching waivers move the affected
paths from active and cross-corner/hierarchy summaries into the per-scenario
waived map without destroying their audit history.

### Follow-up: incremental MMMC timing-violation RPC

`rtlIncrementalMmmcTimingViolations` persists a baseline violation registry by
scenario name, applies global waivers and audit actors, replaces the scenario
set, and returns the full lifecycle transition. The response includes added and
removed scenario snapshots, per-scenario added/removed/improved/degraded/
unchanged paths with old and new slacks, cross-corner endpoint lifecycle
changes, baseline and update reports, the final active/waived registry, and
mapped statistics. Waivers survive constraint updates while raw violation
lifecycle remains visible for review.

### Follow-up: persistent timing-waiver registry RPC lifecycle

The stateful daemon now owns bounded, cross-request MMMC violation registries.
Agents can create a named registry from RTL plus scenarios, list registries,
fetch the full active/waived state, add or remove global waivers with audit
actors, replace the scenario set while retaining lifecycle history and waivers,
and delete a registry. Create reuses the service-owned content-addressed RTL
flow cache; direct calls and asynchronous jobs share the same registry store.
Responses include serialized immutable waiver audit history, active waiver IDs,
scenario names, cross-corner and hierarchy summaries, mapped statistics, and
typed scenario lifecycle updates.

### Follow-up: multi-scenario post-route power/IR RPC

`rtlPostRoutePowerIr` closes another agent-driven signoff gap. It parses the
same inline timing-scenario manifest, converts `post_route.*` properties into
per-scenario power/IR options, runs the cancellable RTL implementation and
physical timing flow once, applies every power scenario to the selected
post-route parasitic view, and returns per-scenario voltage, dynamic/static/
total power, parasitic dynamic power, effective capacitance count and value,
total current, worst IR drop/row, typed findings, and reports. Worst power and
drop scenario labels, total error findings, vector count, the physical timing
report, and mapped statistics are also returned. Explicit input vectors are
supported; gate-level VCD activity is equally supported through the `vcd`
parameter and uses waveform timestamps/intervals rather than resynthesizing
stimulus. Supplying both stimulus forms fails closed. Absent vectors use the
deterministic zero/all-ones pair.

### Follow-up: multi-scenario UPF power-mode IR/EM RPC

`rtlPostRoutePowerModeIr` analyzes every timing/power scenario crossed with
every declared UPF power mode on the selected post-route physical view. It
accepts RTL, the timing-scenario manifest, UPF power intent, physical-timing
options, optional explicit vectors or gate-level VCD activity, and optional
multilayer mesh dimensions. The
response reports the scenario × mode power/current/IR matrix, per-domain power
and voltage data, worst power/drop/EM scenario and mode labels, finite versus
unbounded EM lifetime, findings, per-scenario UPF reports, the physical timing
report, stimulus kind/sample count, and mapped statistics. Direct RPC and
async jobs share cancellation.

### Follow-up: slang AST structural authority check

The `eda-slang` adapter now parses slang's include/macro-aware elaborated AST JSON into a shallow `SlangAstSummary`. The normal compilation path still uses the bounded repository elaborator for downstream IR, but after lowering it records source spans and per-instance post-elaboration parameter values and compares the slang top name, ordered port names/directions, simple declaration widths, top parameter metadata, continuous/procedural assignment targets, recursive RHS operand shapes/order/symbol identity, sized integer-literal values, simple expression width/signedness, and direct and generated instance inventory against the resulting `Design`. A dropped top port, declaration, continuous assignment, or procedural block; direction, simple-width, parameter-duplicate, top-name, literal/width/signedness/RHS-shape, or hierarchy mismatch now fails closed with an available slang source-span note instead of being silently accepted. `compile_files_via_ast` now provides the first direct elaborated-AST-to-IR path for a small verified module subset (including parameter-constant leaves, procedural assignments, if/case control flow, single-edge sensitivity, recursive hierarchy, generated members, and multidimensional sparse-memory whole/part-select operations, inline initializers, compile-time `$readmemh`/`$readmemb` loads with constant bounds and runtime `$writememh`/`$writememb` dumps, and direct inout child-memory sharing) and rejects unsupported constructs explicitly; it is not yet a general statement-level or expression-level lowering implementation.

### Follow-up: first-class sparse RTL memory IR

The elaborated IR now models unpacked memories as first-class `MemoryDecl` objects with element width, ascending/descending logical dimensions, and physical-index metadata. The simulator stores their elements sparsely and executes whole-element reads, blocking writes, and non-blocking writes through explicit memory operations. Logical-index bounds are enforced: out-of-range reads return X and out-of-range writes are no-ops. Memory changes wake continuous readers and combinational processes.

Large one-dimensional module memories (more than 4096 flattened bits) no longer allocate one flat bit per element. They retain a one-bit flat placeholder and use sparse storage. The 4 KiB runtime regression fell from 32,797 flat bits to 30, and a new 1 MiB-by-8 regression uses 38 flat bits while storing only initialized or touched elements. Indexed part-select reads and writes support runtime starts, reversed packed elements, continuous-lvalue preservation, and disjoint NBA-field merging. A direct large `inout` unpacked-array port can share a parent sparse-memory object across hierarchy, with shape checking and both parent/child readers observing the same storage. Post-elaboration inference now marks runtime-written canonical alias groups as RAM and initializer-only memories as ROM, then assigns technology-independent timing templates (`AsyncRom`, `SyncRom`, `MixedRom`, `AsyncRam`, `SyncRam`, or `MixedRam`), conservative runtime write masks, aligned byte-enable lanes, and logical banking; technology-independent macro candidate planning now feeds synthesis reports, while vendor macro selection and physical bank mapping remain future work. Sparse-memory synthesis beyond deterministic distributed ROM/RAM, explicit user-specified byte-enable contracts, general input/output aggregate ports, physical bank mapping/conflict policy, advanced initialization/dump policy controls and runtime file-task diagnostics remain future work; the VCD writer now emits dynamically declared sparse-memory words for touched elements; both direct and bounded paths cover inline initializers or two-to-four-argument constant-bounded `$readmemh`/`$readmemb` and `$writememh`/`$writememb` subsets (inline declaration initializers remain direct-only).

### Follow-up: i8086 randomized sparse-memory differential validation

The i8086 differential matrix now includes 32 deterministic randomized seeds
combining primary ALU operations, immediate groups, immediate and CL-controlled
shift/rotates, safe unary operations, segment prefixes, and ModRM loads/stores.
Each seed compares all architectural registers, IP, FLAGS, halt/trap state, and
a 768-byte sparse-memory window against the independent Rust golden model. The
new stream exposed and fixed incoming-carry leakage into register and immediate
SUB, zero-count shift flag mutation, byte/word RCL and RCR result/carry
extraction, SHL/SAL carry exhaustion past operand width, and SAR sign carry
after saturation. A shift/rotate boundary matrix additionally sweeps
every operation at the zero, width, width-plus-one, and near-maximum masked
counts.

## Remaining high-priority gaps

| Priority | Gap | Why it matters |
|---|---|---|
| P0 | Advanced foundry MOSFET/BJT compact and charge models | A BSIM3 I-V core with simplified short-channel/DIBL/narrow-width/temperature behavior, lumped MOS/BJT RTH/CTH self-heating including transient device temperature, MOS intrinsic/overlap charge, and voltage-dependent source/drain junction sidewall capacitance now exists, but full BSIM3/4 DVT/binning, charge, NQS, coupled spatial thermal networks, foundry noise, and PSP/HiSIM/VBIC/HICUM-class signoff models do not |
| P0 | Certified foundry PDK calibration | Typed PDK signoff exists, but external vendor certification and golden compact-model correlation remain required |
| P1 | Temperature/foundry noise correlation, arbitrary covariance/vendor distributions, autonomous startup/HB | Commercial analog analysis breadth |
| P1 | Full SystemVerilog/UVM and SDC semantics | Parameterized generate-if/for/case, parameter packages/imports/scoped constants, local/package enum, packed struct, and packed union typedefs, simple local/package functions with non-recursive nested inlining, recursion rejection, scalar, packed struct/union, fixed multidimensional unpacked-array locals, fixed multidimensional unpacked module nets and ports, constant-base/runtime-module-read/runtime-function-read/runtime-blocking-write/runtime-NBA-write-and-synthesis/reversed-packed-module-function-continuous-read-write/scalar-runtime-packed-base-continuous-write/runtime-array-element-constant-packed-base-continuous-write/procedural-runtime-module-array-element-read-write-and-synthesis/runtime-function-array-element-read-write indexed part-selects, if/else assignment merging, exact/wildcard-case assignment merging, bounded canonical for-loop unrolling, constant-repeat unrolling, constant-condition bounded while-loop unrolling, shared-interface signal connectivity with explicit modport direction checks, fixed multidimensional interface/module arrays, and static virtual ports, immediate assertions and clocked assert/assume/cover properties with fixed/bounded-range delays, fixed/range bounded consecutive, goto, and nonconsecutive repetition, bounded fixed-delay two-sequence and/or composition, and disable-iff aborts, signed arithmetic/casts/literals, common variable types/default widths, casez/casex, generated clocks, clock groups, and `set_units` time/capacitance normalization are implemented, but dynamic non-interface aggregates, dynamic/multidimensional virtual interfaces, unbounded temporal repetition, full arbitrary-length sequence composition, classes/UVM, and broader commercial SDC semantics remain |
| P1 | Advanced polygon/via layout operations and rule-deck breadth | A typed Manhattan polygon/via database with editing, exact Boolean operations, spacing, width/area, cut, enclosure DRC, and fix suggestions exists, and arbitrary-angle polygons are integrated into document storage, hit testing, SVG, movement, exact area checks, same-layer spacing DRC, dedicated API/browser creation, unconstrained vertex editing, exact two-polygon union/intersection/difference, direct two-pin routing, bounded visibility-graph maze routing, and negotiated arbitrary-angle rip-up/congestion refinement; the candidate graph remains bounded and complete foundry rule decks remain |
| P2 | Native/production-grade GUI shell | Browser hosts drive schematic and layout sessions, including zoom/pan, property grids, rectangle/polygon/via editing, polygon vertices, and DRC; a native shell and broader production workflow integration remain |
| P2 | Full daemon/RPC method surface | Foundational JSON/TCP RPC and job manager exist with cooperative RTL/PDK/SAT/BMC/equivalence/k-induction/router/AMS-loop cancellation including optimizers, but every toolchain API is not yet remotely callable and cancellation is not yet supported inside every nested solver |
| P2 | Distributed/cloud simulation backend | Daemon-shared bounded LRU regression caching, a local multi-worker pool, owner-checked priority/label-routed RPC remote-worker registration/claim/heartbeat/completion, timestamped lease timeout/reassignment, bounded remote retry budgets, and an optional restart-safe local artifact store are implemented; concurrent network object storage and an automatic worker fleet are not bundled |

## Verification gates closed in this iteration

The complete Rust workspace is now formatted and warning-clean under the
current stable Clippy pedantry-sensitive default lint set:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p eda-tests --test suite -- --nocapture
```

Authoritative results on 2026-10-10 were formatting clean, Clippy clean, and
all 15,026 named circuit/signal cases passing. CI now runs the same gates (and
all workspace unit/integration tests) on every push and pull request.

The workspace goal remains active until these gaps are implemented and verified
rather than merely documented.
