# Detection: the difference between looking and seeing

*Written 2026-08-26. The record of the first half of TODO's "improve line of
sight system, should be easier to hide while seeing enemy" — the detection
roll, which is additive and cheap. The second half, a shadowcasting field of
view, changes **which** tiles are visible and is deliberately not in here:
doing both at once would leave nothing attributable.*

## The complaint

`vision_range` was doing detection's job. A crew saw every tile inside a hard
radius and nothing outside it, and a unit standing on a tile she could see was
a unit she had found — instantly, whatever the ground, whatever the target was
doing. At 100 m to the hex that reads badly the moment the ranges grow: a
commander with twenty hexes of vision is claiming two kilometres, which she can
genuinely see across, and certainty at 1,999 m with nothing at all at 2,001 m
is not eyesight. It is a cutoff, and it was the only thing on the battlefield
keeping anything hidden.

## What was built

Spotting keeps the geometry it had — line of sight through `SightGrid`, range,
and `VehicleDef::concealment` shortening a spotter's reach against a
particular target — and gains one thing on top: **finding somebody inside your
own field of view costs a roll, once per tick, until you find her.**

    chance = detection_base
           - (terrain.concealment + detection_at_range_percent) × fade
           + detection_per_hex_moved × hexes she crossed this round

`fade` runs from 0 at `detection_certain_percent` of the spotter's reach to 1
at the limit of it. Everything is clamped to 0..=100 and rolled by the
best-placed crew who can see her ground — one roll, not one per pair of eyes.

Four rules hold it together, and each of them was a wrong first draft:

- **A search buys an acquisition, never the watching afterwards.** A contact
  this side already holds is not re-rolled; nor is a crew who has just fired
  (`revealed` is checked before any die). Without the first clause every found
  enemy flickers in and out of the picture tick by tick.
- **One look per target per tick**, recorded in `SideFog::searched`. The fog is
  recomputed after movement, after fire, and again after every individual shot;
  rolling per call would make finding somebody a function of how much shooting
  happened to be going on nearby. The look is spent even when the chance was
  zero, so a crew who becomes conspicuous mid-tick is found on the next one.
- **The best spotter rolls, not each of them.** A roll per pair of eyes reads
  well and makes the printed chance a lie by however many crews happen to be
  looking: at four spotters a nominal 2% is 8%, and the entire usable range of
  the knob collapses into single digits. More eyes still pay — they watch more
  *ground*, and somebody is closer to it.
- **The near band is not optional.** The first draft had no
  `detection_certain_percent` and gave two tanks three hexes apart on open
  grass an 82% chance of noticing each other per tick. Nobody searches for the
  tank 300 m away in an open field. It cost two dozen failing tests to find
  out, which is the tests doing their job.

`detection_at_range_percent` reads the spotter's **own** reach and not the
concealment-shortened one. The two answer different questions —
`VehicleDef::concealment` has already priced how close somebody must be to
find *her* at all — and compounding them put a platoon at three hexes on 75%
of "reach", taking the full far-band penalty on top of the shortening.

## The numbers, and what they say

`cargo run --release -p tactics_core --example balance -- --only detect`
prints the whole table from the engine's own `Balance::detection_chance`.
As shipped (`detection_base` 100, `certain` 40%, `at_range` 70,
`per_hex_moved` 20):

| ground | conceal | close | half | 3/4 | far | far +1h | far +3h | ticks at far |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| grass | – | 100 | 89 | 60 | 30 | 50 | 90 | 3.3 |
| forest / town | 25% | 100 | 85 | 45 | 5 | 25 | 65 | 20.0 |
| mountains | 15% | 100 | 87 | 51 | 15 | 35 | 75 | 6.7 |
| deep_forest | 35% | 100 | 84 | 40 | 0 | 15 | 55 | never |

A round is twelve ticks, so `20.0` is 1.7 rounds and `never` means a crew
sitting still in deep timber at the limit of somebody's sight is not found by
looking at all — she is found when the looker closes, or the moment she drives
or fires.

## What it did to the game: not much, and that is the finding

Measured 2026-08-26, 36 battles per row, four seeds, against the same run with
`detection_certain_percent` at 100 (its neutral value, which is the rule
switched off). **Re-measured after the goal chooser learned to price a road**,
because the first draft of this table was taken before it and every number in
it moved a little:

| | wins (side A of 36) | rounds | stalemates | contact range | shots on the move |
| --- | --- | --- | --- | --- | --- |
| rule off | 14 / 19 / 21 / 24 | 12.5–13.2 | 0 | 10.7–11.1 hexes | 51–53% |
| rule on | 19 / 22 / 20 / 22 | 12.7–14.1 | 0 | 11.0–11.1 hexes | 48–53% |

At 36 battles a genuinely level pairing lands anywhere from 12–24 to 24–12
nineteen times in twenty, so the win column says nothing either way — which is
the point. The four seeds are the same four in both rows.

Every column is inside the seed noise floor. Re-measure with:

```sh
cargo run --release -p tactics_core --example balance -- --sim --games 36 \
    --only sim --absolute --sweep seed=0,1000,2000,3000
cargo run --release -p tactics_core --example balance -- --sim --games 36 \
    --only sim --sweep balance.detection_certain_percent=100,40
```

Two things are worth knowing about why the effect is so small, because both
are answers to "what should be built next":

- **Line of sight already binds harder than vision range does.** Contact on
  these maps is made at 11 hexes mean while vision ranges run 8–20, so the
  ground is doing most of the hiding and a rule about the outer band of a
  sight radius has less to bite on than the complaint assumed. That is an
  argument *for* the shadowcasting work rather than against this.
- **Nothing in the evaluator wants to be unseen.** This is the same shape as
  stacking: the mechanism is live and measured, and the AI never chooses to
  sit still to stay hidden, never prefers concealing ground *for its
  concealment*, and never times an advance around what the enemy can resolve.
  So detection cannot pay off in AI-versus-AI battles, and every number above
  is a floor rather than a measurement of what the rule is worth. It is a
  player-facing rule until the goal chooser learns to want it.

## Costs

Round resolution on `river_crossing` measures **1.62 ms with the rule off and
1.38 ms with it on** (`cargo run --release -p tactics_core --example perf`).

That is the right way round and it was not always: measured on the day this
landed, before the goal chooser could price a road, the same comparison read
1.30 off against 1.87 *on* — and the spotting pass was demonstrably not where
that went, since the rule *lowers* the number of fog recomputes (545 → 454)
and of cold field-of-view computations (322 → 305) over the same benchmark,
with `search` running about 2.4 times a round. The standing guess was that the
cost was behavioural: contact comes later, so crews spend longer manoeuvring.
The chooser then made them manoeuvre better and the sign flipped, which is
about as much confirmation as that guess is going to get.

The lesson for the next person is not about detection. It is that **round
resolution on this benchmark moves with how well the AI plays**, so a change
to the planners will move it without anything in the tick loop getting slower
or faster. Read it alongside `reachable()`, `roads()` and `unit_vision`, which
measure work rather than behaviour.

## Additivity

The neutral value is `detection_certain_percent: 100` — a crew's whole reach is
the near band, nothing is ever faded in, and **not one die is thrown**.
`a_mod_that_asks_for_no_search_spots_exactly_as_it_always_did` pins both
halves, including the rng stream position, and it is pinned separately from the
determinism snapshot on purpose: the base mod now declares detection numbers,
so that snapshot is a record of the rule being *on*.

Tests that stage two crews in plain sight of each other in order to test
something else go through `seen(registry())`, the twin of `registry_wireless()`
and there for the same reason.
