# Multi-Scale Simulation: Level Transition Design

**Purpose of this document.** Context brief for an implementation session. Describes how to move
state between levels of abstraction in a hierarchical simulation (person → household → town).
Deliberately not tied to a specific domain — the domain enters only through the choice of coarse
variables and the error indicator.

---

## 1. The two operators

Borrow the multigrid vocabulary:

- **Restriction** `R : Fine → Coarse` — aggregation, "step up"
- **Prolongation** `P : Coarse → Fine` — disaggregation, "step down"

**The one invariant worth enforcing in code:**

```
R(P(c)) == c        for all coarse states c      # consistency, enforceable, test this
P(R(x)) != x        in general                   # information loss, unavoidable, do not fight it
```

Every design decision below is downstream of that asymmetry. The engineering question is not
"how do we avoid losing information" but "which information are we licensed to lose."

---

## 2. Choosing the coarse state

The coarse state should be a **sufficient statistic** for the coarse dynamics: the minimal summary
such that town-level evolution never needs to inspect individual persons.

**Design rule: make `R` a monoid homomorphism.** If `R` is associative and order-independent,
then:

- aggregation parallelizes trivially (fold in any order, any partition)
- `person → household → town` composes without special-casing intermediate levels
- incremental/streaming updates work (merge a delta instead of recomputing)

Safe (mergeable) aggregates:

| Aggregate | Merge | Notes |
|---|---|---|
| sum, count | `+` | conserved quantities live here |
| min / max | `min` / `max` | idempotent |
| histogram / bucketed counts | elementwise `+` | keeps distribution shape, not just mean |
| t-digest | digest merge | approximate quantiles, mergeable |
| HyperLogLog | register-wise `max` | approximate cardinality |
| moments (Σx, Σx², Σx³) | `+` | derive mean/var/skew after the fold |

Unsafe: medians, exact percentiles, "does any member satisfy P" over non-idempotent predicates,
anything whose value depends on partition boundaries. These do not commute with re-partitioning
and are the usual source of level-inconsistency bugs. If one is unavoidable, define it on a
mergeable proxy (histogram → approximate median) rather than on raw membership.

**Conservation.** Anything physically conserved (headcount, currency) must be conserved by both
operators. Structural enforcement: if `R` is a sum, then `P` must be a *redistribution of exactly
that sum*, never an independent draw that happens to have the right mean. Assert conservation at
every level boundary.

---

## 3. The closure problem

The central difficulty. In general:

```
F_coarse(R(x)) != R(F_fine(x))
```

The coarse dynamics are **not closed** — town-level evolution genuinely depends on detail that
`R` discarded. The residual is the *closure term*. Three honest options:

**(a) Analytic closure.** Mean-field, or moment closure with an explicit truncation assumption
(e.g. assume third-order cumulants vanish). Cheap, fast, and the assumption is usually
unfalsifiable within the sim.

**(b) Learned closure.** Fit a correction term `Δ(c)` from paired fine/coarse trajectories so
that `F_coarse(c) + Δ(c) ≈ R(F_fine(P(c)))`. Requires fine-grained training runs; generalizes
poorly outside the sampled region of state space.

**(c) Refuse to close it — equation-free / HMM.** *Usually the right default.* Never write down
town-level laws at all. Instead build a **coarse timestepper**:

```
coarse_step(c, Δt):
    x  = P(c)                       # lift  (§4)
    x' = run_fine(x, δt * k)        # short burst, k steps, δt << Δt
    c' = R(x')
    dc = (c' - c) / (k * δt)        # estimate coarse derivative
    return c + dc * Δt              # project forward over the large step
```

This is heterogeneous multiscale modeling. You pay for short fine bursts but skip the long
stretches between them, and you never need a theory of towns. The gain is the ratio
`Δt / (k·δt)`. Tune `k` upward until the derivative estimate stabilizes (variance of `dc` across
independent lifts falls below tolerance).

---

## 4. Prolongation is a sampler, not a function

`P` is not a map. It is a conditional distribution `p(fine | coarse)`, and you need a generative
model with `R(sample) == coarse` as a **hard** constraint, not a soft penalty.

Standard approaches:

- **Maximum entropy subject to moment constraints** — least-committal distribution consistent
  with the coarse state. Principled default.
- **Iterative proportional fitting (raking)** — fit a joint table to known marginals. This is
  exactly what synthetic population generation in agent-based models already does; reuse that
  literature rather than inventing.

**Cache the fine state.** The single largest practical win. Re-instantiating a household from
scratch destroys identity, history, and continuity — persons pop in and out of existence with
new attributes. Instead: keep the last known fine state, and *reconcile* it against the new
coarse constraints (adjust the minimum number of agents needed to satisfy `R(x) == c`). Cheaper
than resampling and it preserves agent identity across level transitions, which matters if
anything downstream tracks individuals over time.

Non-uniqueness of `P` is not a bug to be eliminated. It is the honest representation of what the
coarse state failed to determine. If results are sensitive to the choice of lift, that
sensitivity is a finding: the coarse variables are insufficient. Consider measuring it directly —
run `n` independent lifts, compare coarse trajectories, treat the spread as a closure-error
estimate.

---

## 5. Adaptive level switching

Run coarse by default; refine only where an **error indicator** fires. This is AMR logic and the
indicator is the real API surface of the system — get it right and everything else is mechanical.

Indicators worth implementing:

1. **Within-cell variance** exceeds threshold → the aggregate has stopped being representative
   of its members.
2. **Curvature / nonlinearity** in the coarse response over the step → averaging is no longer
   commuting with the dynamics.
3. **Proximity to a discrete threshold** — any place where an individual crossing a boundary
   produces a non-smooth coarse effect. Aggregates are worst exactly here.
4. **Explicit query** — user or downstream consumer asks about a specific individual.

Coarsen back when the indicator falls below a *lower* threshold than the one that triggered
refinement. Hysteresis, otherwise cells thrash at the boundary.

---

## 6. Time

Levels have different natural timesteps; the coarse level takes larger ones. Two workable
structures:

- **Nesting** — one coarse step contains `k` fine steps, with a synchronization barrier at the
  coarse boundary. Simple, and the barrier is where you assert consistency and conservation.
- **Operator splitting** — advance levels independently within a window, then reconcile.
  Cheaper, but reconciliation is where ordering hazards appear.

Hazard to watch: a fine-grained event that must influence coarse state *mid-step*. Either
promote it to a coarse-level event (and take the smaller step), or accept that it lands at the
next barrier and document the resulting latency. Do not silently apply it out of order.

---

## 7. Quantum: what is and isn't real here

**Does not work.** Preparing a superposition over 2ⁿ population configurations is easy and
useless — measurement yields one sample, and amplitude estimation buys only a quadratic speedup
on expectation values, swamped in practice by state-preparation cost. Agent dynamics are also
strongly nonlinear while Schrödinger evolution is linear; Carleman linearization handles weak
nonlinearity and then blows up in the truncation order.

**Elegant but not a speedup.** Amplitude-encode the joint distribution over agent states. Then
**restriction is exactly the partial trace** over the qubits carrying fine detail: the reduced
density matrix *is* the coarse state, correlations included. The non-uniqueness of purification
is precisely the non-uniqueness of disaggregation (§4). Good conceptual model, no computational
advantage.

**The transferable idea — Pauli propagation as a coarse-graining scheme.** This is the part that
actually pays, and it does not require a quantum computer.

Pauli propagation is Heisenberg-picture: rather than evolving the state, evolve the *observable*
backward through the circuit, and truncate Pauli strings below a weight or coefficient-magnitude
threshold. Port the structure:

| Quantum | Classical multi-scale analogue |
|---|---|
| Pauli string weight | order of joint correlation between agents |
| Backward observable evolution | propagate target observables back through the dynamics |
| Truncate high-weight strings | drop terms depending on 3rd+ order agent correlations |
| Truncation error bound | quantitative closure error bound |

Concretely: instead of maintaining and evolving a full town state, take the handful of
observables you actually care about, propagate them backward through the dynamics, and truncate
terms that depend on high-order joint correlations below a coefficient threshold. The argument
that high-weight strings contribute negligibly under noise or scrambling is structurally the same
argument that high-order agent correlations wash out under mixing.

The payoff over §3(a): you get a **truncation error bound** with a tunable knob, rather than an
unfalsifiable mean-field assumption. The truncation threshold becomes the accuracy/cost dial for
the whole simulation.

---

## 8. Implementation order

1. Define coarse state as a mergeable summary (§2). Write the merge, property-test associativity
   and commutativity.
2. Implement `R`. Test `R(P(c)) == c` and conservation as invariants.
3. Implement `P` as a constrained sampler with fine-state caching (§4).
4. Build the coarse timestepper via short fine bursts (§3c) before attempting any analytic
   closure. Get correctness first; the closure is an optimization.
5. Add the error indicator and adaptive refinement with hysteresis (§5).
6. Only then consider observable-propagation with truncation (§7) as the performance path.

## 9. Open questions to resolve against the actual domain

- Which coarse variables are genuinely sufficient? Test empirically: does a coarse trajectory
  seeded from `n` different lifts of the same coarse state stay converged?
- What are the discrete thresholds in the domain? Those set where refinement is mandatory.
- Is agent identity required to persist across level transitions? Determines whether the fine
  cache in §4 is a nicety or a hard requirement.
- What is the acceptable coarse-level error, and does it have a natural unit? Needed to set the
  truncation threshold in §7.
