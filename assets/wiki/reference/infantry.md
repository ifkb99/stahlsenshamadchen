# Infantry and the ride to battle — the last large mechanics before MVP

*Written 2026-08-14, after the ballistics rewrite closed. This is the design
record and build order for infantry and the armoured transports that carry
them: the last two large mechanical additions planned before the MVP wrap-up.
Read `ballistics.md` first — almost everything here is that machinery reused,
which is the point.*

## Decisions taken (2026-08-14, with the designer)

- **A platoon is one piece, led by named cadets.** Managing thirty soldiers
  would be tiresome (designer's word); an infantry unit is a platoon on one
  hex, with 2–3 cadets as its leadership — seats in the existing crew model —
  and the rest abstracted. Splitting into squads is deferred (the design
  leaves the door open: each cadet is a squad leader in waiting).
- **Full mount and dismount, this arc.** The APC/IFV pairing only means
  something if troops actually ride. Passengers share the carrier's fate
  when she is penetrated — riding a halftrack under fire *should* be scary,
  which is period truth — and dismounting is an order, not an animation.
- **AT is the RPG and the recoilless rifle, not the ATGM.** Shaped charges
  at one to three hexes: infantry's tank-killing is an ambush, deadly in
  timber and towns and helpless in the open. Wire-guided missiles are the
  60s revolution and get their own pass later (the shell-flight machinery
  is waiting for them).
- **Leaders are weighted like everything else.** Casualty rolls pick from
  the cadets and the abstracted rifles by weight, through the same interior
  machinery as tank crews: leaders are rarely hit while the platoon is
  strong and increasingly exposed as it thins. "Leaders last" emerges from
  arithmetic; a lucky burst can still find her early, which is where the
  drama lives.
- **The period is the early-to-mid 1960s.** Halftracks are the 40s legacy
  still in second-line service; the M113-generation battle taxi is current;
  the true IFV (HS.30, the BMP's precursors) is just arriving with an
  autocannon and the doctrine argument attached. The roster carries all
  three so the campaign can say something about equipment generations, the
  same sentence the tank roster already speaks.
- **Everything is mod data.** No new content kind is invented: infantry are
  chassis, their inventory is modules, their AT is ammunition. A modder who
  can add a tank can add a paratroop platoon.

## The model: infantry are a chassis

An infantry platoon is a `VehicleDef` — deliberately. `MovementClass::Foot`
has existed in the data since the beginning; armor is 0/0/0; `crew_slots`
name the leadership seats (new `platoon_leader` / `section_leader` roles in
data, skilled in command, fieldcraft, small arms); `weapons` are the
platoon's rifles-and-MGs (small-arms class) and its AT tube (chemical
ammunition: flat penetration, one to three hexes); `stowage` counts the
rounds; `modules` are the inventory, exactly as the designer asked —
including the abstracted troops themselves:

- **`ModuleEffect::Troops`** is the one new effect kind (code), and the
  platoon's rifle strength is a module instance (data): big `size` (the
  troops are most of what a burst can find), `toughness` counted in
  sections-worth of casualties. Its three meanings:
  - *Interior weight*: casualty rolls find troops far more often than the
    cadets, by the same weighted roll a tank uses — this is the ruled
    leader-risk model with no extra machinery.
  - *Firepower*: every weapon the unit fires scales its damage by the
    troops fraction (hits remaining over toughness). A platoon at half
    strength shoots half as hard; the cadets alone are nearly harmless.
  - *Combat effectiveness*: at zero the platoon is a remnant — alive, the
    cadets still aboard the battle, pulled hard toward withdrawal by the
    condition score it already wrecks. Like a mission-killed tank, a
    shattered platoon is a story, not a deletion.
- **`field_radio`** is a radio-effect module: the man-pack set, short send
  range in data, and a platoon whose set dies falls back to runners and
  flags — the contact machinery already knows what that means.

**Squishiness falls out of armor 0** — every hit penetrates, blast is
lethal — with one deliberate carve-out built in the same pass: the
overpressure *overmatch* rule (blast ≥ twice the plate wrecks the vehicle)
must not apply to plate-zero units, or one shell splashing a neighbouring
hex would delete a dispersed platoon outright. For soft, dispersed things,
splash converts to casualty rolls instead: artillery against infantry is
attrition — brutal attrition — never a single-event erasure. A *direct*
hit already penetrates and rolls its full budget through the ordinary
pipeline, which is devastating enough to be honest.

**Concealment is a chassis field.** `concealment` (percent, default 0)
scales every spotter's effective vision range against this unit; standing
in real cover (tile cover ≥ 30) counts it again. Infantry at ~40 are hard
to see in the open and nearly invisible in timber until they move or
shoot; the scout section at ~55 is the recon instrument the designer
asked for. Vehicles default to 0 and nothing changes for them — the
additivity rule, as always. Firing still reveals (`reveal_to_all` is
untouched): an ambush is spent by springing it.

## The ride: mount and dismount

- `VehicleDef.capacity: u32` — how many units she lifts (0 for everything
  that exists today; 1 for the new transports). A transport *unit*
  abstractly represents the platoon's lift, the same one-piece abstraction
  as the platoon itself.
- `Unit.aboard: Option<UnitId>` — the passenger state. Aboard: she is off
  the map for spotting and targeting, her position mirrors the carrier's,
  she neither moves nor shoots (firing ports are an IFV-doctrine argument
  for a later pass, not MVP).
- **Orders**: `Order::Mount { unit, into }` (drive/walk to the carrier,
  board on arrival — the walk uses the ordinary movement machinery) and
  `Order::Dismount { unit }` (she steps off onto the carrier's hex at the
  top of the next tick, spotted or not — dismounting into an ambush is a
  thing that happens). Both travel the order stream like everything else:
  saves, replays and external brains get them for free.
- **Shared fate**: a penetration into a loaded carrier adds every
  passenger's cadets and troops to the interior roll pool, weighted like
  everything else. A brew-up rolls the passengers through the same fire.
  This is the ruled decision, and it is what makes "dismount before the
  gun line" a real tactical sentence rather than flavour.
- **The AI rides before it plans rides**: MVP AI mounts nothing on its own
  initiative but understands mounted starts (scenario data may spawn
  passengers aboard), dismounts when its carrier is under effective AT
  threat, and dismounts to take ground a vehicle cannot hold. Planning a
  taxi run end-to-end is the willingness work's business, later.

## The roster (all data)

| id | what it is |
| --- | --- |
| `halftrack` | The 40s legacy lift: open-topped, thin, an MG, cheap. High `safety` (everyone can jump clear), zero dignity. |
| `apc` | The M113-generation battle taxi: enclosed aluminum box, MG, amphibious pretensions left out. Taxi doctrine: arrive, unload, get out. |
| `ifv` | The HS.30-generation argument: a 20 mm autocannon (new weapon, fast cadence, kinetic AP + HE belts), armor that stops rifle fire, and the temptation to fight mounted that the period had not yet resolved. |
| `rifle_platoon` | 2 cadets + troops module (the mass of the platoon), rifles/MG, an RPG with a handful of rounds, field radio. Concealment ~40. |
| `scout_section` | 2 cadets + a small troops module, high concealment (~55), binoculars-grade vision, next to no teeth. The recon instrument. |

## Build order

Same routing discipline as every arc: tight-specced data chunks to Opus,
engine and judgment chunks to Fable, every chunk through the full
verification loop with numbers quoted in commits.

**N0. The data layer, inert (2/5 — Opus).** `ModuleEffect::Troops` variant
(one enum arm so the data loads), `concealment` + `capacity` fields
(serde-defaulted, unread), the new roles, weapons, ammunition, module
defs and all five chassis in the base mod, plus validation (a troops
module on a vehicle with crew slots but no capacity is fine; capacity on
a foot chassis is a warning; an AT tube with no stowage is the existing
warning). Byte-identical baseline is the proof of inertness, as always.

**N1. Soft targets and hidden ones (4/5 — Fable).** Concealment in the
spotting pass (per-target effective range where units-on-visible-tiles
become spotted — the tile-vision cache is untouched), troops-module
mechanics (interior weight is free; firepower scaling; remnant
behaviour), the plate-zero splash carve-out, and foot-movement sanity
against the terrain data. The balance instrument grows a concealment
column and infantry rows.

**N2. The ride (5/5 — Fable).** Mount/dismount orders, the aboard state,
shared fate on penetration and brew-up, fog behaviour, save format, HUD
(a loaded carrier shows her passengers; a passenger panel says where she
is), and the three AI dismount reflexes. The chunk that cannot be split
from itself.

**N3. The field (3/5 — Opus).** Infantry and transports placed into all
three battle maps' orders of battle, the campaign's `frontier` armies
learning the new vehicles exist, playthrough/HUD narration polish, and
the full instrument re-quote that becomes the infantry-era baseline.

## The arc as built (all four chunks ✅, 2026-08-14)

- **N0** (`ed59c5d`): the five chassis and their kit, inert, with the
  autocannon-vs-taxi and RPG-vs-everything-but-the-heavy pairings
  printing straight from the analytic tables.
- **N1** (`ad1b358`): concealment (the recon car that reads twenty hexes
  spots a treeline platoon at four), the troops fraction, the plate-zero
  splash carve-out. Byte-identical baseline — everything additive.
- **Ambush discipline** (`deae95c`, mid-arc, designer-ratified for every
  unit): an unseen crew holds opportunity fire below a quarter of the
  target's remaining substance. Let them close. Rules-of-engagement as
  leader-assignable postures recorded in TODO as the designed future.
- **N2** (`6c0bf0b`): the ride — standing mount marches, dismounts onto
  the ground beside, passengers sharing a penetrated carrier's fate and
  a brew's fire, exits taking everyone home, and the dismount reflex.
- **N3** (`4b5480c`): fielded. Both new maps carry a mounted platoon and
  a foot scout section per side (APCs on the plains, halftracks in the
  forest), frontier's armies motorise, the player gets M/U and passenger
  panels, and the instruments speak infantry. N3 also fixed an N2 gap:
  `from_map` never honoured `aboard_at` — mounted starts worked only on
  the campaign path until `board_mounted_starts` was shared by both.

**What the first 36-game infantry-era baseline said** (reported, not yet
tuned): infantry are all-or-nothing — 27 of 48 platoons die outright,
mostly aboard their taxis (22/24 APCs and 21/24 halftracks are lost),
while surviving foot units end battles with 98% of their rifles and
almost no ammunition spent. The dismount reflex fired 47 of 48 times it
was owed. The two named tuning numbers for the next balance pass:
**transports as coffins** (the AI drives taxis like tanks) and
**infantry that never shoots** (rifles too short-ranged and platoons too
hidden to ever engage). Both are AI-employment questions more than data
questions, and they wait on the pool-era doctrine re-read.

## The employment pass (2026-08-14, after the baseline was read)

The baseline's two named numbers — *transports as coffins* and *infantry
that never shoots* — were AI-employment questions, and the designer
rejected the obvious cures ("scaling the objective value up… feels
manual rather than emergent"). Three changes went in instead, none of
them a multiplier and none of them naming a chassis in Rust.

- **Leaders claim gunnery** (`d525a1e`). The leadership seats were
  written with the skills a leader obviously has and gunnery was not
  among them, so no seat was responsible for it, `crew_skill` fell
  through to "best aboard", and every cadet read gunnery through her
  cores at the untrained penalty. The platoon laid every weapon it owns
  four levels below ordinary — a silent −12 percentage points on every
  infantry weapon in the game. Expected shots to knock a target out:
  RPG 9.1 → 6.7, rifles 28.9 → 23.6.
- **Danger priced as a fraction** (`4ffbfd2`). The evaluator's threat
  term was absolute, so five points of expected damage read identically
  to a fresh heavy tank and to a loaded taxi with one cadet left. It is
  now divided by what she can still absorb — against
  `BattleState::typical_substance`, the field's own mean complement, so
  the reference is data rather than a constant — and multiplied by what
  is riding on her. Honest measurement: **this barely moves the pool
  tables**, because threat reaches six hexes with a 1/distance falloff
  while the guns reach sixteen, so for most of an approach march it is
  zero. Repricing zero changes nothing. Removing the six-hex gate was
  tried and cost the skill-gap table more than it won.
- **The commander reads her infantry** (`7d812aa`). A formation is an
  infantry element because somebody in it walks — recognised off the
  chassis, exactly as a base of fire is recognised by somebody laying an
  indirect weapon — and such a formation is given the *covered* ground
  and told to hold it rather than drawing a slot in the tank rotation.
  The maps had to earn it: the taxi and her platoon shipped inside the
  armoured formation, so each side now fields three formations — line,
  guns, grenadiers. Massed armour's delegation tax falls from 8 wins to
  5, and side 0's foot troops fire 9 rounds a pairing against the flat
  control's 2. Draws rise 2 → 5, which is what infantry sitting on
  objectives costs.

**Still open, and now understood rather than guessed:** 21 of 24 taxis
die whether the side fights flat or under command, because a mission
belongs to a formation and the carrier holds the ground her passengers
were sent to hold. "Deliver them and get out" is a sentence about one
unit. `Unit.detached` is the seam; the work is logged in TODO under
Chain of Command beside the assignable postures.

## Open questions, carried deliberately

- **Squad splitting** — deferred per the designer ("unless it is easy":
  it is not; it is mid-battle unit spawning). The cadets-as-squad-leaders
  seating means the split falls out naturally when wanted.
- **ATGM teams** — deferred by ruling; the shell-flight machinery is
  their landing pad.
- **Firing ports / fighting mounted** — the IFV doctrine argument, after
  the taxi works.
- **Entrenchment** — infantry that digs in belongs with the engineer
  vehicle in TODO, not here.
- **Buildings as fighting positions** — town tiles already give cover;
  giving interiors capacity is a post-MVP idea that would reuse the
  mount machinery.
