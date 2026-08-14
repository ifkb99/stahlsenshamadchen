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
2. **Impact geometry.** The hull is a hexagonal prism: the struck *face*
   is the one whose normal lies nearest the incoming ray (the same
   quantisation the three-arc facing already performs), the arc decides
   which armor value that face wears — three faces of front plate, two of
   side, one of rear — and *obliquity* is the ray's residual angle off
   that face's own normal, at most thirty degrees by construction, worth
   up to ~15% extra effective armor. Angling is therefore about which
   armor class each threat axis meets, exactly the decision turning a real
   hull makes. (The first draft measured obliquity against the arc's
   central normal; since the front arc spans ±90° of incoming ray, its
   edges read as near-parallel strikes on the glacis and became
   impenetrable — the empirical trace that caught it is in the B1 commit.)
   Ricochet is not a separate mechanism: it is penetration failing, and
   the scatter makes marginal shots genuinely marginal.
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
   thinnest plates but delivers its `blast` externally — overpressure and
   fragmentation, per the designer's ruling (2026-08-13): *a huge HE shell
   hitting or landing near a tank definitely does something to it.* What
   it does is roll against what physically lives outside the plate:
   mobility (the classic result of a 105 arriving next to a track), the
   radio antenna, and the crew's nerves through the morale ladder. Blast
   *overmatch* goes further: a large enough shell against thin enough
   armor wrecks the vehicle without consulting the penetration gate at
   all — a recon car under a 105 is not a bounce. Against soft and open
   vehicles HE is simply lethal. Against a hex (indirect impact, B3) it
   affects everyone in and adjacent to the tile, scaled by cover — the
   first time terrain cover has mattered against artillery. Until B2/B3
   land, HE that cannot penetrate only rattles: a known interim, not the
   design.

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

**B1. The gate: pen-or-nothing (4/5 — Fable).** ✅ Done. The floor died.
Hit → hex-face impact geometry → interpolated penetration × integer
scatter → roll; a non-pen does zero structural damage, announces itself
(`ShotBounced`), and rattles the crew through a new `morale.bounced`
pressure rung — never for small arms, which would have rebuilt the defect
in morale. Consumption is on (`WeaponDry` announces the last round; blind
shelling spends shells); the analytic `penetration_chance` and the
resolver's roll enumerate the same finite comparison so the AI can never
be lied to; `expected_damage` = hit × pen × damage, and a zero-value shot
is *not taken* — crews finally hold fire. The obliquity model earned its
correction mid-build: measuring against the armor arc's central normal
made front-arc edges impenetrable (a duel's return fire went silent for a
tick until the hulls turned — caught by the crew's-clock test timing, of
all things); the shipped model strikes the hex face whose normal is
nearest the ray, ±30° residual, and `no_seam_of_the_hull_is_impenetrable`
pins it. Measured at 36 games: flat massed-vs-elastic swung 16-20-0 →
4-29-3 — tank destroyers 87 kills to 5 losses, light tanks 2 to 63,
frontal plate finally meaning what the roster always claimed — and the
instrument's "worth a look" now prints "gun_75 cannot meaningfully hurt
heavy_tank from the front" where the MG absurdities used to be. That
swing is deliberately not chased here: B4 re-baselines after B3 gives HE
its external blast (until then artillery is soft-target-only, a known
interim). Round resolution 1.11 ms.

**B2. Outcomes: the end of hit points (5/5 — Fable, the heart).** Split
into B2a, the module data layer (✅ done, `ea451b6`, Opus, inert by the
same byte-identical-baseline proof B0 gave), and B2b, the outcome engine.
The B2b model, decided before the first line of it is written:

- **The effect budget.** A penetration converts its ledger damage — the
  weapon's weight times the round's `post_pen`, the number B1 already
  computes — into effect rolls: one roll per four points, rounded up, so
  a machine-gun burst that gets into a soft car rolls once and an 88
  through a glacis rolls three or four times. Each roll picks from what
  is physically inside, weighted by size: every girl aboard is a station
  of crew-weight (a `balance` knob), every module its declared `size`.
- **Crew hits strike named girls.** A girl rolled is *wounded* if fine
  and *out* — dead or unconscious, the distinction resolved after the
  battle by the roster's existing fate machinery, worse odds from a
  vehicle that burned — if wounded again; a heavily overmatching round
  can put her out in one. A wounded girl works her station at a penalty;
  an out girl's station is covered by the best remaining crew member
  through the substitution rule that already exists (short-handed crews
  are this game's normal case, and the machinery was built for it).
  Every crew hit is an event that names her.
- **Module hits spend toughness** and announce damaged/destroyed. Gun
  destroyed silences the primary weapon (index 0 — a future data field
  can map modules to mounts when a vehicle needs it); mobility damaged
  halves movement, destroyed stops her where she stands; the radio
  destroyed drops her off the net through the contact machinery that
  already knows what silence means; the ammo rack is the special one —
  every rack hit rolls brew-up at a data-scaled chance times the
  fraction of ammunition still aboard, and a rack destroyed without a
  fire leaves the remaining rounds unusable, announced as the gun
  running dry.
- **Brew-up is the catastrophe:** the vehicle is destroyed at once and
  the girls' fate rolls carry the fire.
- **Bail-out is morale, not arithmetic.** Every penetration slams the
  crew with a data-priced pressure spike on top of the hit pressure that
  exists today, and then rolls the same `holds_together` discipline
  check the refusal system uses. A crew that fails abandons: the vehicle
  is a wreck for scoring, the girls walk home by the existing fate
  machinery. This is what keeps time-to-kill honest — real tanks are
  mostly lost because the crew leaves or dies, not because every box is
  ticked — and it makes discipline training visibly be the thing that
  keeps a damaged tank in the fight.
- **Overpressure (the designer's ruling).** A non-penetrating hit whose
  round carries `blast` rolls against the *exterior* — mobility and
  radio modules — at a chance shaped by blast against plate, and blast
  overmatch (blast comfortably above the plate) wrecks the vehicle
  without consulting the gate at all: a recon car under a 105 is not a
  bounce. Small arms still rattle nobody.
- **Knocked out is derived, never stored:** brewed, abandoned, or no
  girl aboard able to fight. Hit points are deleted; a mission-killed
  vehicle (gun and tracks gone, crew grimly aboard) is *alive*, which is
  what makes recovering her a future campaign story instead of a
  contradiction. Every hp consumer — evaluator threat and withdraw
  thresholds, formation "beaten", MCTS scoring, the HUD bar (becomes the
  status readout: gun, tracks, radio, faces), save, the balance
  instrument's lethality math — is rebuilt on a condition score derived
  from crew and modules.

Everything before B2b exists to make its diff reviewable; it is still
the longest chunk and it lands as one commit, because half a hit-point
system is not a reviewable state.

**B2b ✅ done.** The model above shipped as written, plus three rules the
build discovered it needed:

- **Decisions before triggers.** With outcomes landing mid-tick, the crew
  processed first could shoot the gun out of the second's hands and cancel
  a reply that was already coming — loop order became a rule of the game,
  which is the exact sin the reap-at-end-of-tick bargain exists to
  prevent. `resolve_fire` now runs two phases: every crew decides against
  the tick's opening state, then everything resolves.
- **Bail-out rides the rung, not raw dice.** Rolling discipline on every
  penetration made average crews flee half the time from the first hit.
  The shipped rule mirrors disobedience exactly: the *prospective* rung —
  where this penetration's pressure will land her — decides whether nerve
  is in question, and only a rung that disobeys rolls `holds_together`.
  Base ladder: first pen leaves a steady crew wavering, the second puts
  her at breaking and the dice speak. A one-rung ladder never bails —
  pinned.
- **An empty crew list is an abstracted crew, not a dead one** — and
  placements that name no girls now spawn anonymous ones, one per seat,
  trained to average at exactly what the seat demands. Without hit
  points, dying happens to people, and whether a vehicle is mortal must
  not depend on whether a scenario author wrote a roster.

Measured at 36 games against B1's table: flat massed-vs-elastic 4-29-3 →
9-23-4 (the outcome model softened the tank destroyer's reign: 87 → 76
kills), mean 9.2 rounds, and the four-seed baseline carries 17 named crew
hits, 65 module hits, 14 brew-ups and an abandonment whose story — racks
wrecked, both guns dry, radio dead, crew walks — is the design doc read
back by the engine. The rack is the dominant killer, which is
period-true; whether 60% at full racks is the right base is B4's
question. Round resolution 1.00 ms. `VehicleDef::max_hp` is deprecated,
unread, and optional.

**B3. Flight time and the artillery rework (3/5 — Opus, tight spec).**
Shells in flight as state, impact against the hex, blast over the tile
and neighbors scaled by cover, the narration for it. Self-contained
enough to spec once B2's event vocabulary exists.

**B4. The AI economy pass (4/5 — Fable).** Ammo choice per target,
conservation, artillery lead, threatened-epsilon and withdraw-condition
tuning, and the doctrine coefficients re-read against the new outcome
space. This is where the delegation-tax and stalemate numbers get
re-baselined and quoted.

Its first target was measured on 2026-08-13 by the `skillgap` example
(folded into `balance --sim` by B5, which is where it lives now) after
the designer asked whether a skill gap wins cleanly here the way
it does in military history: **it does not, and difficulty is inverted
in practice.** With identical forces and doctrine, the noiseless
difficulty-5 utility planner loses to a noisy one in both side
orientations, and every pairing's loss ratio sits near 1:1 — no
lopsided victories exist in the current space at all. The greedy argmax
coordinates badly with itself (deterministic clumping, frozen local
optima) and difficulty noise accidentally implements the dispersion and
exploration that good play actually needs. What generalship historically
buys — fighting only unfair fights: local concentration against isolated
enemies, ambush from cover, refusing engagements at bad odds — is
exactly what the evaluator does not yet price, and B4's success metric
is therefore written down now: a difficulty-5 side against difficulty-1
with equal forces should win most battles at a loss ratio visibly
better than 1:2, and skill should buy *cleanliness*, not just wins.

**B5. Instruments and presentation (3/5 — Opus).** The balance analytic
pass rewritten around the kill chain (P(pen) tables per gun × armor ×
range, time-to-kill distributions, ammo economy), playthrough narration,
HUD polish, perf additions for the new hot paths.

**B5 (the balance instrument) ✅ done.** Both passes now speak the kill
chain, and the analytic tables are forced *through* the engine rather
than derived beside it: a duel loads exactly the round under test into
the racks with `set_loadout`, so `chambered` has one choice and the
printed penetration is `preview_attack`'s own — the number the AI plans
on. Analytic: P(pen) per gun × round × target at near/mid/max on the
front plate plus side and rear at mid (square on, obliquity 1.00);
expected shots and rounds to knock out through hit × pen × effect
budget against the substance pool, with the brew-up and blast-overmatch
terms labelled as modeled; shell flight ticks at each band beside how
far the quickest vehicle moves in them; and a "worth a look" that
judges a gun on blast overmatch as well as penetration, flags a vehicle
nothing can hurt, and flags a round no gun can chamber. Simulated: kill
causes by the flag the vehicle died carrying, crew wounded/out per
battle, ammunition gone per round-type, artillery's shells-on-occupied-
ground rate, and the skill-gap table, folded in from the standalone
example, which is deleted — one question about the data, one
instrument. Two things it says immediately that B4 needs: **the 105's
blast (6) overmatches even the Löwe's thinnest plate (3), so a direct
hit wrecks a heavy tank without consulting the gate at all**, and half
of all deaths are ammunition fires. The other half of B5 — playthrough
narration, HUD polish, perf additions — is untouched.

Order: B0 → B1 → B2 → B3/B5 in parallel → B4 last, because tuning an
economy before the instruments can see it is guessing.

## Open questions, carried deliberately

- **Hit location inside a facing** (hull vs turret, weak points): ruled
  with the designer (2026-08-13) as too deep for MVP — at 100 m hexes the
  player commands sections, not gun-laying, so exterior weak points would
  be invisible dice while doubling the armor schema on every vehicle. The
  *angle* story (arcs + hex-face obliquity + angling as armor-class
  selection) carries the MVP, and the interesting half of "what did it
  hit" lives inside anyway: B2's post-pen rolls are weighted by what the
  round passes through, so a driver's-plate penetration finds the driver.
  If post-MVP feel wants more exterior texture, weak points slot into the
  impact-geometry step without disturbing anything above it.
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
