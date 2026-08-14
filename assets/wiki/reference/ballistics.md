# Ballistics — penetration, the girls inside, and the end of hit points

*Written 2026-08-13, after the chain-of-command layer landed. This is the
design record and build order for the combat-model rewrite: velocity and
travel time, penetration and ricochet, behind-armor effects, ammo types and
counts, and the AI that has to understand all of it. It begins the feature
wrap-up before MVP: after this, IFVs and infantry are the only large
mechanical additions planned — everything else should be small additions or
balance work.*

Read `command.md` first if you have not; the two docs share their
architecture commitments and this one leans on machinery that one built
(morale pressure, succession, the battle drill, the crew's clock, formations
and the wire).

## Why, and why now

The current model is `damage × pen/(pen+armor)` against a 10-point HP pool,
with a `.max(1)` floor on every hit. The floor is the single most
consequential defect in the game and the balance instrument states it
plainly: an MG kills a heavy tank in three rounds — the same as a 37 mm gun.
CLAUDE.md has carried "a non-penetrating hit should do nothing, not a chip"
since the instrument first printed that line.

The period makes the floor worse than a nuisance. This roster deliberately
spans twenty years of armor development in service at once, and the entire
meaning of a 1943 gun meeting 1960s armor is that it *cannot get through*.
A damage floor erases the one sentence the vehicle roster is trying to say.

The prerequisites this rewrite waited for are now met: battles resolve
rather than stalemate (objectives), there is a delegation layer whose crews
can be told "your gun cannot hurt that, hold fire or flank", and the balance
harness can bracket any change with fought-out numbers.

## Decisions taken (2026-08-13, with the designer)

- **Hit points are removed, not rescaled.** A vehicle has no HP pool. What
  a penetration does is roll *outcomes* — crew hit, module hit, ammo fire —
  and a vehicle is out of the fight when the state of her crew and modules
  says so, Combat Mission style. Every system that reads `hp` today is
  rebuilt on condition instead. This was chosen over the incremental
  keep-HP options with eyes open.
- **Crew hits strike named girls, by station.** A penetration can wound or
  kill a specific girl mid-battle, and her station degrades immediately:
  the gunner hit means the gun slows or silences, the driver hit means the
  tank stops maneuvering, the commander hit feeds the succession and morale
  machinery that already exists. Permadeath is a campaign default; this is
  where it gets its teeth.
- **Modules are content, not an enum.** Gun, mobility, ammo stowage and
  radio ship in the base mod, and the module list is data so mods (and
  later chunks) add modules without touching Rust. Effect *kinds* are code;
  module *instances* — what a vehicle carries, how big a target each is,
  what it does when it dies — are data.
- **Ammo loadouts are data; editing is stubbed.** Vehicles declare stowage
  (e.g. 45 AP / 30 HE), counts are tracked in battle, nothing resupplies
  mid-battle, and replenishment between battles is automatic. The engine
  exposes a loadout-editing surface from day one, but the screen that calls
  it waits for the UI pass.
- **Deferred by the designer, recorded so it is not lost:** the
  advance/assault *feel* wants a revisit after the ballistics balance
  settles (their words: "a bit wonky — come back after other balance
  changes"), and the one-continuous-map change (vehicles exist on the
  battle layer at all times; the overworld is a zoom level, so retreat,
  pursuit and luring happen on real ground) is noted in TODO.md and comes
  after most other things are sorted.

## The scale, and what velocity means here

A battle hex is 100 m and a tick is 5 s. An 88 crossing its entire 1.6 km
reach takes about two seconds; every direct-fire shot in the game arrives
inside the tick that fired it. So "velocity and travel time" does not mean
watching tracers crawl:

- **Velocity is ammo data.** It exists on the round, in m/s, because it is
  the honest input to both uses below and because a mod adding a rocket or
  a missile should only have to write a number.
- **Kinetic penetration falls with range.** The data carries penetration at
  the weapon's minimum and maximum range and the pipeline interpolates.
  Chemical energy (HEAT-type) does not fall off — that flat line is *why*
  HEAT exists, and the difference between the two curves is a real tactical
  fact the AI can stand on (close the range against a kinetic gun; never
  bother against a HEAT thrower).
- **Only indirect fire takes flight time.** A 105 shell at ~470 m/s needs
  eight or nine seconds to cross 4 km: the shot resolves one to two ticks
  after it is fired, **against the hex it was aimed at**, not against the
  unit. Whoever is standing there when it lands is who it lands on. This
  turns artillery from the hitscan sniper it currently is (top killer in
  the balance tables, 22 kills a campaign) into what it actually was: an
  area weapon that punishes units that stand still, commanded by
  prediction. Direct fire resolves in-tick exactly as today.

## The pipeline

What happens to one fired round, replacing `raw_damage`:

1. **To-hit is kept.** The existing accuracy machinery — base accuracy,
   falloff, gunnery, cover, elevation, facing — survives unchanged. It was
   never the problem.
2. **Impact geometry.** The existing three-arc facing (front/side/rear)
   picks the armor plate. On top of it, *obliquity*: the exact bearing from
   shooter to target versus the plate's normal — geometry we already have —
   scales effective armor up as the impact goes oblique and raises the
   ricochet chance sharply near the arc seams. A shot that arrives nearly
   parallel to the plate glances off however big the gun is. This is what
   makes angling and flanking physical rather than a flat side-armor
   discount.
3. **The penetration roll.** Interpolated penetration for the range, a
   small quality scatter (data — rounds are not clones), against effective
   armor. **All comparisons are ratio-based** (pen/armor), never absolute
   margins, so the abstract armor units can be relabeled to millimetres
   later by editing data alone. Three outcomes:
   - *Clean penetration* — pen comfortably over armor. Full behind-armor
     effect.
   - *Partial penetration* — the marginal band. Spall and fragments: a
     reduced effect roll, no full energy dump.
   - *No penetration* — **nothing structural. Zero. The floor is dead.**
     What a non-pen does instead: shock (morale pressure through the
     existing ladder — being rung like a bell is real even when the armor
     holds) and a chance of *external* module damage where physics allows
     it (tracks under a heavy HE hit, the radio antenna), per module data.
4. **Behind-armor effects.** A penetration converts to an effect budget
   scaled by overmatch ratio and the round's `post_pen` potency, spent as
   rolls against what is physically inside: the crew stations and the
   module list, weighted by their declared sizes. An ammo-stowage hit rolls
   brew-up scaled by the *fraction of ammo still aboard* — an emptied tank
   is measurably harder to torch, which quietly makes ammo counts a
   survival stat as well as an economy.
5. **High explosive.** Against armor a direct HE hit penetrates only the
   thinnest plates but delivers its `blast` externally: mobility and
   antenna damage, heavy shock. Against soft and open vehicles it is
   simply lethal. Against a hex (indirect impact) it affects everyone in
   and adjacent to the tile, scaled by cover — which is the first time
   terrain cover has mattered against artillery, and one more reason crews
   already drilled to scatter and dig in were built first.

## What a vehicle is, without hit points

A unit's fighting state is the conjunction of her crew and her modules:

- **Crew:** each girl aboard is fine, wounded, or gone (dead or
  unconscious — the roster's post-battle fate machinery already
  distinguishes walking home from not). A wounded girl works her station
  at a penalty; an empty station works barely or not at all. Skills
  already flow through `crew_skill`, so degradation is mostly *removing a
  contributor*, not new arithmetic.
- **Modules:** each is intact, damaged, or destroyed, with effects from
  its data: gun (silent or degraded), mobility (immobilized — she fights
  from where she stands), ammo (the brew-up roll lives here), radio (she
  drops off the net, and the entire chain-of-command layer starts caring
  about ballistics for free — this was flagged as a future when radio
  hardware landed, and it lands here).
- **Knocked out** is derived, never stored as a number: brewed up, or no
  crew able to fight, or *abandoned* — bailing out is a morale decision,
  rolled through the existing pressure ladder when wounds and hits stack
  up, so discipline training buys crews that stay with a damaged tank.
  The `alive` flag keeps meaning exactly what it means today ("on the
  battlefield"); wrecks and abandonments reap through the same
  end-of-tick pass as deaths so simultaneity is preserved.

Every current reader of `hp` gets rebuilt on a **condition score** derived
from crew and modules — the AI's withdraw thresholds, the formation
"beaten" metric, the brain's mission review, MCTS rollout scoring, the HUD
(the health bar becomes a status readout: gun, tracks, radio, four small
faces), the save format, and the balance instrument. The inventory of
touch points is long and is exactly why the build order below stages the
demolition.

## Ammo, and the AI that spends it

Ammo types are a content type (`AmmoDef`): class (kinetic / chemical /
explosive / small-arms), penetration near/far, post-pen potency, blast,
velocity. Weapons declare what they can fire; vehicles declare stowage;
units carry counts (BTreeMap — iteration order reaches events).

The AI's shot pricing changes from "expected damage" to **expected
outcome**: P(hit) × P(pen) × expected condition removed, plus the shock
value of near-miss pressure. Consequences that fall out rather than being
coded:

- An MG genuinely cannot threaten a heavy tank, so `threatened` — the one
  predicate the drill, the evaluator and the engine share — stops counting
  it, and crews stop breaking cover to flee machine guns that cannot hurt
  them. The movement-to-contact damping, the battle drill and opportunity
  fire all get more honest for free.
- Ammo selection is the same argmax: AP at armor, HE at soft targets and
  suppression, skip the shot entirely when nothing loaded can do anything
  — holding fire *is* the correct play the floor was hiding.
- A dry or gun-dead vehicle prices every attack at zero, and the existing
  withdraw machinery reads her condition and pulls her out — no new
  "retreat because empty" rule is written, the mission review just sees a
  unit that can no longer contribute.
- Artillery leads its targets: it aims at hexes, so it aims where the
  target will be, and standing still under observation becomes the
  mistake it historically was.

## Difficulty stays a mod

The harsh knobs are data with gentle zeros, same contract as everything
else: crew-hit severity can collapse to "everyone walks away" (the promise
the casualty system already makes), brew-up chance to zero, quality
scatter to zero. **The floor's removal is not a difficulty knob** — it is
a correctness fix; there is no mod that restores chip damage, the same way
there is no mod that makes fog lie.

Determinism discipline is unchanged and load-bearing: every roll through
the battle rng in unit-id order, BTreeMaps where iteration reaches events,
the four-seed baseline regenerated only deliberately with the diff read
first, and the save-fork suite extended to the new state (crew wounds,
module states, ammo counts, shells in flight).

## Build order

Chunks sized for the same routing as the command work: low-judgment chunks
go to Opus subagents on tight specs; the burned-area chunks are Fable's.
Every chunk ends with the full verification loop (suite listed per binary,
baseline discipline, `balance --sim` before/after quoted in the commit,
clippy/fmt/validate-mods).

**B0. Ammo as content, inert (2/5 — Opus).** `AmmoDef`, weapon ammo lists,
vehicle stowage, unit counts, validation and the validate-mods roster
table, the loadout-editing engine stub. Deliberately consumes nothing:
the event-stream baseline must come out byte-identical, which is the
proof it stayed a data chunk.

**B1. The gate: pen-or-nothing (4/5 — Fable).** The floor dies. Hit →
facing + obliquity → interpolated penetration → roll; non-pen does zero
structural damage and feeds shock; consumption turns on (counts decrement,
a dry gun is silent). HP survives this chunk as a temporary severity
ledger so the whole suite keeps meaning something while the gate goes in;
`expected_damage` learns P(pen) so the planners stop plinking the moment
the gate exists. Deliberate baseline regen; balance bracketed before and
after (the MG-grinds-heavies line must die in the same commit).

**B2. Outcomes: the end of hit points (5/5 — Fable, the heart).** Crew
stations and named-girl wounds, modules from data, brew-up on ammo
fraction, bail-out through morale, knocked-out as a derived state, every
hp consumer rebuilt on condition, events that carry who/what/why for each
outcome, save format, HUD status readout. The longest chunk and the one
that cannot be split from itself; everything before exists to make its
diff reviewable.

**B3. Flight time and the artillery rework (3/5 — Opus, tight spec).**
Shells in flight as state, impact against the hex, blast over the tile
and neighbors scaled by cover, the narration for it. Self-contained
enough to spec once B2's event vocabulary exists.

**B4. The AI economy pass (4/5 — Fable).** Ammo choice per target,
conservation, artillery lead, threatened-epsilon and withdraw-condition
tuning, and the doctrine coefficients re-read against the new outcome
space. This is where the delegation-tax and stalemate numbers get
re-baselined and quoted.

**B5. Instruments and presentation (3/5 — Opus).** The balance analytic
pass rewritten around the kill chain (P(pen) tables per gun × armor ×
range, time-to-kill distributions, ammo economy), playthrough narration,
HUD polish, perf additions for the new hot paths.

Order: B0 → B1 → B2 → B3/B5 in parallel → B4 last, because tuning an
economy before the instruments can see it is guessing.

## Open questions, carried deliberately

- **Hit location inside a facing** (hull vs turret, weak points): the
  pipeline's impact-geometry step is where it would slot; not MVP.
- **APCR/HEAT premium rounds as scarce stowage:** the data schema carries
  them from B0 (class + flat curve is all HEAT needs); shipping them in
  the base mod waits for the balance pass to want them.
- **Smoke:** the round type is trivial in this schema, the fog interaction
  is not; after MVP.
- **Field repair and recovery** (mobility kills, abandoned vehicles as
  salvage): ties into the engineer/logistics units already in TODO; the
  wreck state lands in B2 either way.
- **Unbuttoned commanders** (vision vs exposure): wants the girls' trait
  system (girls.md slice 5) so it can be a personality, not a toggle.
- **Ammo units:** penetration stays in today's abstract armor units; the
  ratio-only rule above means a later data-only pass can relabel the whole
  scale to millimetres without touching code.
