//! Doctrine-weighted scoring, shared by every planner.
//!
//! The numbers a planner cares about — is this tile worth holding, is this
//! position winning — live here rather than inside one planner, so the
//! utility planner's per-unit choice and MCTS's search heuristic can never
//! drift apart, and so a doctrine file changes both at once.

use super::{best_weapon_against, visible_enemies};
use crate::battle::{BattleState, UnitId};
use crate::data::{DataRegistry, DoctrineDef};
use crate::map::ObjectiveKind;
use hexx::Hex;

/// What holding a tile is worth, and the shot that comes with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileScore {
    pub score: f32,
    /// The best attack available from that tile: target and weapon index.
    pub attack: Option<(UnitId, usize)>,
}

/// Scores positions the way one doctrine sees them. Deterministic: any
/// difficulty noise is the planner's business, added on top.
pub struct Evaluator {
    pub doctrine: DoctrineDef,
}

impl Evaluator {
    pub fn new(doctrine: DoctrineDef) -> Self {
        Self { doctrine }
    }

    /// Score `unit` standing on `tile`, together with the best attack it
    /// could make from there.
    pub fn score_tile(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        tile: Hex,
    ) -> TileScore {
        let Some(me) = state.unit(unit) else {
            return TileScore {
                score: 0.0,
                attack: None,
            };
        };
        let doctrine = &self.doctrine;
        let enemies = visible_enemies(state, me.side);

        // Offense: the best shot available from this tile. Indirect appetite
        // scales what artillery is worth, so a doctrine that hoards shells
        // values a howitzer opportunity less than one that spends them.
        let mut best_attack: Option<(UnitId, usize, f32)> = None;
        for enemy in &enemies {
            let Some((weapon, dmg, kill)) =
                best_weapon_against(registry, state, unit, tile, enemy, enemy.pos)
            else {
                continue;
            };
            let mut value = dmg + if kill { 4.0 } else { 0.0 };
            if self.is_indirect(registry, state, unit, weapon) {
                value *= doctrine.indirect_appetite;
            }
            if best_attack.is_none_or(|(_, _, v)| value > v) {
                best_attack = Some((enemy.id, weapon, value));
            }
        }
        let attack_value = best_attack.map(|(_, _, v)| v).unwrap_or(0.0);

        // Threat: what the visible enemies could put on her if she stood
        // *there*, in the resolver's own arithmetic and nobody else's.
        //
        // [`fire_on`](crate::battle::fire_on)'s summed twin, and the whole
        // of Phase 2's argument in one line. What stood here priced
        // `best_weapon_against(enemy, enemy.pos, me, me.pos)` — the shot at
        // the hex she was *already* standing on — and let the candidate tile
        // in only as a `1/distance` falloff behind a six-hex gate. So
        // driving out from behind a ridge into a gun's arc was priced
        // identically to staying behind it, and cover, elevation, facing,
        // profile, obliquity and range never reached the decision about
        // where to stand at all. Every one of them reaches it now, because
        // this is the same function the shot itself goes through.
        //
        // The gate and the falloff are gone rather than retuned, and that is
        // not tidiness. Both were standing in for the positional terms the
        // arithmetic could not see: a falloff is a guess at "further off is
        // safer" that the range band now states exactly, and a six-hex cut
        // is a guess at "out of reach" that sight and range now answer for
        // each gun on the field. Keeping either would charge the same fact
        // twice and, worse, would cap the term below what a sixteen-hex gun
        // can actually do. Note what that gate cost while it stood: threat
        // was *zero* for most of an approach march, so no amount of
        // repricing it changed a decision, which is why removing it on its
        // own once cost the skill table more than it won.
        //
        // Fog-honest by construction — only enemies this side has found are
        // on the list — so a crew cannot flinch away from a tank nobody has
        // seen and thereby tell the player it is there.
        let threat = crate::battle::incoming(registry, state, unit, tile);
        // ...and what that costs *her*, which is a different question and was
        // not being asked. The sum above is in substance points, an absolute
        // quantity, so five points of expected damage read exactly the same to
        // a fresh heavy tank as to a battle taxi with a platoon in the back
        // and one cadet still on her feet. Every unit on the field weighed
        // danger by the size of the shell rather than by what the shell would
        // take from her.
        //
        // Danger is therefore priced as a *fraction*, out of two ratios, both
        // read off state that already exists. No chassis is named here and
        // none should be — an APC is timid because she is small and soft, not
        // because a table says APCs are timid, and a mod's own vehicles get
        // the same treatment on the day they are written:
        //
        // - **Fragility: how much of her a hit is.** Expected damage over
        //   what she can still absorb, restated against the field's own
        //   reference so it stays in the same units as every other term. A
        //   nine-point taxi feels a five-point shell as most of herself; a
        //   fifteen-point heavy feels it as a third. And because the divisor
        //   is what is *left* rather than her full complement, the same crew
        //   grows more careful as she is worn down — which `caution` below
        //   also does, but does as a doctrine's appetite for withdrawing,
        //   scaled by `withdraw_threshold`. This one is not an opinion: there
        //   is simply less of her, and it applies to the stubbornest side on
        //   the field.
        // - **Stake: what is riding on her.** A loaded carrier gambles her
        //   passengers on every tile she picks, because shared fate is real —
        //   a penetration rolls the platoon in the back through the same
        //   interior pool and a brew-up burns them. An empty taxi risks a
        //   hull; a full one risks the infantry's whole afternoon, and ought
        //   to drive like it. Empty, the factor is exactly one and this costs
        //   nothing.
        //
        // Capped, because the fragility ratio diverges: a crew down to her
        // last point would weigh a scratch as fifteen times a mortal threat,
        // and someone four times as careful as a fresh crew is already
        // refusing every tile a fresh crew would take. Past that the term
        // stops discriminating and only makes the arithmetic loud.
        //
        // This is the half of the old pricing that survived Phase 2 unchanged,
        // and the reason it did is the line the whole chunk is drawn on: the
        // sum above is what the *rules* say can be put on her, which is the
        // resolver's to answer, and this is what she makes of it, which is
        // hers and her doctrine's. One is arithmetic and one is a preference.
        let exposure = {
            /// Most a crew may multiply danger by for being small, worn down,
            /// or loaded. Four is "refuses what a fresh crew accepts", which
            /// is as far as the distinction still says anything.
            const MAX_EXPOSURE: f32 = 4.0;
            let left = state.substance(registry, me).0.max(1) as f32;
            let riding: u32 = state
                .units
                .iter()
                .filter(|u| u.alive && u.aboard == Some(unit))
                .map(|u| state.substance(registry, u).0)
                .sum();
            let fragility = state.typical_substance(registry) / left;
            let stake = 1.0 + riding as f32 / left;
            (fragility * stake).min(MAX_EXPOSURE)
        };
        // Condition replaces the hit-point fraction: cadets and modules
        // remaining over the full complement. A crew that has taken wounds
        // and lost gear grows cautious by exactly the machinery that used
        // to read a shrinking pool.
        let condition = state.condition(registry, me).clamp(0.0, 1.0);
        let caution =
            (1.5 - doctrine.aggression) * (1.0 + doctrine.withdraw_threshold * (1.0 - condition));

        // Terrain: what cover and high ground are worth *before* anybody has
        // been found, and a doctrine's taste beyond the arithmetic.
        //
        // This used to be an independent model of what cover and elevation
        // do — `cover * 0.03`, `elevation * 0.4` — sitting beside a threat
        // term that could not read either. Since the term above became the
        // resolver's own answer, `balance.cover_against_accuracy` and
        // `balance.downhill_bonus` price both of them exactly, per gun and
        // per bearing, so a flat bonus on top is a second opinion about the
        // same fact and the two can only drift.
        //
        // What it is *not* is redundant, and the reason is the fog gate one
        // term up. Threat is a statement about the enemies this side has
        // found; before the first contact — round 3.5 on the shipped maps,
        // and never for the guns still unspotted at the bell — it is exactly
        // zero, and a crew with nothing on her list would have no reason at
        // all to prefer a wood to a field. That is the prior this term is:
        // *somebody will be shooting from somewhere*, so take the ground that
        // will be worth having when they do. It is also where a doctrine's
        // taste lives, which is why `cover_value` and `elevation_value` are
        // read here and nowhere else: two commanders looking at identical
        // arithmetic may still disagree about how much they like a ridge, and
        // that disagreement is content rather than a rule.
        //
        // So the rule is: **the prior speaks only where the arithmetic is
        // silent.** A tile no gun this side has found can reach is priced by
        // taste; a tile inside a found gun's envelope is priced by the shot
        // that gun would take, cover and elevation included, and the flat
        // bonus stands down rather than being added twice. That is a
        // statement per *tile* rather than per battle, so on a map with one
        // spotted gun the ground under its arc is arithmetic and the rest of
        // the map is still preference — which is what an approach march is
        // made of, first contact falling in round 3.5 of 13.
        //
        // Measured, because the alternative reading (keep the term and shrink
        // its coefficients) was the obvious one and is worse. On the mirrored
        // arena at 4 seeds x 36 battles a side, the skill gap reads 48.8% /
        // 51.6% (5 over 1 / 5 over 3) with the flat term as it stood, 53.0% /
        // 50.7% with both coefficients halved, and 55.0% / 56.3% with them at
        // zero — monotone, and the strongest signal this table has produced.
        // Zero is not available, though: it is the only thing that reads
        // `cover_value` and `elevation_value`, and a doctrine weight nothing
        // consults is exactly the dead field this project keeps finding. The
        // gate buys the same thing without spending them, and it ships at the
        // coefficients the constants gave rather than at retuned ones.
        //
        // The engine-side coefficients are data anyway, for the reason every
        // other evaluator weight is — see `planner.cover_prior`.
        let mut terrain_value = 0.0;
        if threat <= 0.0
            && let Some(t) = state.map.get(tile)
        {
            terrain_value +=
                t.elevation as f32 * registry.planner.elevation_prior * doctrine.elevation_value;
            if let Some(def) = registry.terrain(&t.terrain) {
                terrain_value +=
                    def.cover as f32 * registry.planner.cover_prior * doctrine.cover_value;
            }
        }

        // Mass: a spacing band rather than a pull. The old term was a
        // monotonic attraction to the nearest friend, which is why massed
        // armour clumped into artillery bait — the closer, the better, all
        // the way to adjacency. What drill actually teaches is an interval:
        // close enough for mutual support, far enough that one shell cannot
        // kill two vehicles. So inside two hexes there is a crowding penalty
        // that no doctrine can buy off (that is not a preference, it is
        // survival), inside the supported interval there is nothing to pay,
        // and beyond it the out-of-support penalty scales with
        // `concentration` exactly as the old pull did. Support also demands
        // a *sight line* from the friend's planned position — near but
        // masked is not mutual support, and requiring the check is sectors
        // and interlocking fires in one line. Friends still count from
        // where they are heading, not where they stand, so a formation
        // converges instead of chasing.
        let mass = {
            /// Farthest a friend can stand and still be supporting.
            const SUPPORT: f32 = 4.0;
            let nearest = state
                .side_units(me.side)
                .filter(|other| other.id != unit)
                .map(|other| {
                    let at = other.planned_destination();
                    (at.distance_to(tile), at)
                })
                .min_by_key(|(dist, at)| (*dist, at.x, at.y));
            match nearest {
                None => 0.0,
                Some((dist, at)) => {
                    let crowding = match dist {
                        1 => -0.45,
                        2 => -0.15,
                        _ => 0.0,
                    };
                    let supported = dist as f32 <= SUPPORT && state.sight.clear(at, tile);
                    let apart = if supported {
                        0.0
                    } else {
                        -((dist as f32 - SUPPORT).max(1.0)) * 0.12 * doctrine.concentration
                    };
                    crowding + apart
                }
            }
        };

        // Objectives: the one thing on the map worth something with no enemy
        // attached to it, and the reason this evaluator will leave good cover
        // at all. Measured against the old behaviour it is the whole fix —
        // without it, holding the best ground in sight is unbeatable play,
        // and two sides doing that never meet.
        //
        // A standing mission replaces this term outright rather than adding
        // to it: the commander has decided which ground matters, and a unit
        // under orders stops weighing the whole map for itself. A unit in no
        // formation, or in one that has not been given a mission, scores
        // exactly as it always did — which is the additivity hinge, and why
        // the mission is read from battle state rather than passed in: every
        // planner that scores through here becomes mission-aware at once,
        // and one that never sees a mission is bit-for-bit the old game.
        //
        // A unit who cannot hear her chain of command soldiers on the orders
        // she was carrying when the wire went dead — `mission_for` is that
        // snapshot — rather than going rogue. Standing orders standing is
        // what commander loss *means* in the design doc, and it is also what
        // keeps a command block additive on maps where leaders die early:
        // with the alternative "cut off means unmissioned" model, half the
        // map's missions silently vanished by round two. With no command
        // rules nobody is ever out of contact and `mission_for` is exactly
        // `mission`, bit for bit.
        // A detached unit — under the commander's personal tasking — is
        // excused from the standing mission entirely: her ground is where
        // she was put, and every other term (cover, threat, the drill's
        // judgment) still applies.
        let standing = if me.detached {
            None
        } else {
            state
                .command
                .formation_of(unit)
                .and_then(|f| f.mission_for(unit).map(|m| (m, f)))
        };
        // Contact suspends a movement to contact. An `Advance` is exactly
        // that order — take the ground, fight what you meet on the way — so
        // while somebody is shooting at her where she stands, the pull toward
        // the commander's hex is cut to `planner.pull_under_fire` and her own
        // appetites (the shot in front of her, cover, the threat she is
        // under) decide the round. Nothing is latched: when the enemy is dead or lost the
        // damping evaporates and the march resumes, which is why this is a
        // scale on the term rather than a state anybody has to clear.
        //
        // The playtest that forced it: a delegated advance walked into
        // effective fire at the bridge and was gone by round three, because
        // "go there" outbid every local reason not to. The rule the designer
        // ruled on is that a mission may not override the battle drill, and
        // the real-world shape agrees — movement to contact halts and fights,
        // and pressing on through fire is a *different order*, which is
        // [`Mission::Assault`] and is deliberately absent from this match.
        // Which is also why a mod setting `pull_under_fire` to 1 has not
        // turned a rule off but collapsed two verbs into one: `Advance` would
        // then be `Assault` in every respect.
        // `Recon` damps too: eyes forward is even less of a reason to drive
        // into a gun than ground is.
        //
        // Only asked when there is a mission of that kind to damp, because
        // `threatened` walks the visible enemies again and this runs per
        // candidate tile.
        let contact_scale = match standing {
            Some((
                crate::battle::Mission::Advance { .. } | crate::battle::Mission::Recon { .. },
                _,
            )) if super::threatened(registry, state, unit) => registry.planner.pull_under_fire,
            _ => 1.0,
        };
        let objective = match standing {
            Some((mission, formation)) => {
                self.mission_value(
                    registry,
                    state,
                    tile,
                    mission,
                    formation,
                    formation.latitude_for(unit),
                ) * contact_scale
            }
            None => self.objective_value(registry, state, me.side, tile, condition),
        };

        // A crew ordered out stops valuing the fight. Without this, a shot
        // worth taking outbids any walkable gradient — measured on
        // river_crossing, the chance to shoot a spotted scout was worth ~4.6
        // to a withdrawing tank against the ~0.5 per hex its lane could pull,
        // so the "withdrawal" stood on the best firing line instead. A rear
        // guard still answers what is in front of it (a quarter, not zero),
        // but it does not *seek* — and the advance-toward-contact term is
        // actively arguing with the order, so it goes entirely.
        let withdrawing = matches!(standing, Some((crate::battle::Mission::Withdraw { .. }, _)));
        let attack_scale = if withdrawing { 0.25 } else { 1.0 };

        // Advance: with something to shoot, close on it. With no contact and
        // no objectives, push toward the middle of the map to find some —
        // which is all this could do before objectives existed, and is still
        // what a map that names none gets.
        let advance = match enemies.iter().map(|e| e.pos.distance_to(tile)).min() {
            Some(_) if withdrawing => 0.0,
            Some(nearest) => -(nearest as f32) * 0.3 * doctrine.aggression,
            // Under a mission the slope already says which way to walk, and
            // "inwards" would argue with it on a map with nothing to hold.
            None if standing.is_some() => 0.0,
            // The 0.15 here is deliberately *not* `planner.distance_decay`,
            // despite sharing its magnitude. That number is the slope of a
            // gradient leading to a named piece of ground, and the reason it
            // is one number for objectives and missions alike is that
            // `mission_weight` is quoted in objective-value units. This is
            // the fallback for a map that names no ground at all: "wander
            // towards the middle and find somebody". Folding the two together
            // would mean a designer asking how far an order reaches also
            // changed how a lost crew searches an empty map, which is a
            // different question with a different right answer.
            None if state.map.objectives().is_empty() => {
                -(state.map.center().distance_to(tile) as f32) * 0.15 * doctrine.scouting
            }
            // The objective term is already saying which way to walk, and far
            // more specifically than "inwards" ever did.
            None => 0.0,
        };

        TileScore {
            score: attack_value * attack_scale * 2.0 * (0.5 + doctrine.aggression)
                - threat * caution * exposure
                + terrain_value
                + mass
                + objective
                + advance,
            attack: best_attack.map(|(t, w, _)| (t, w)),
        }
    }

    /// What holding `tile` is worth in objective terms to `side`.
    ///
    /// Every objective is scored and the best taken, so a unit walks toward
    /// the one it can most usefully affect rather than being pulled apart by
    /// all of them at once. Two things fall out of the shape:
    ///
    /// - The reward is for *standing* on it, and the distance term is a slope
    ///   leading there, so a unit twenty hexes away still knows which way to
    ///   drive. That slope is what a greedy one-round planner needs: it can
    ///   only see the tiles it can reach this round, so a gradient that is
    ///   flat until you arrive is a gradient it cannot follow.
    /// - Ground already held is worth half. It is still worth sitting on —
    ///   walking away hands it back for free — but not worth marching across
    ///   the map for when there is unclaimed ground somewhere else.
    ///
    /// The two coefficients — the reward for arriving and
    /// [`distance_decay`](crate::data::PlannerRules::distance_decay) for the
    /// slope — were swept over 24 battles, by hand, before the second of them
    /// was data. Both halves of the curve are bad in different ways: at zero
    /// this is the old game, 11 of 24
    /// decided and 302 shots fired in 16.5 rounds because nobody advances; at
    /// double these values it is 21 of 24 decided but only 517 shots in 8.1
    /// rounds, because units drive at the objective through fire and are
    /// destroyed before a firefight develops. The peak of *fighting* — 652
    /// shots, 23 of 24 decided, 9.3 rounds — is here, which is why the
    /// engine-side numbers are half what they first were rather than the
    /// doctrine values being odd fractions.
    /// Exits are the exception, and the reason this is not one formula. Ground
    /// worth *leaving* by cannot be worth walking to in general — a unit that
    /// valued the exit the way it values the bridge would drive off the map on
    /// the first round and call it a victory. The pull toward an exit is
    /// therefore gated on the doctrine's `withdraw_threshold` and the
    /// vehicle's own damage: intact crews cannot see the lane at all, and a
    /// crew that is nearly finished under a doctrine that expects to fall back
    /// will run for it. That is the whole of withdrawal for now; who is
    /// *permitted* to leave belongs to the chain of command, not to the
    /// evaluator.
    fn objective_value(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
        tile: Hex,
        condition: f32,
    ) -> f32 {
        // How badly this crew wants out.
        //
        // `withdraw_threshold` is a fraction of strength *lost* before a
        // doctrine looks for a way out — so a high one is stubborn, which is
        // why massed armour sits at 0.85 and elastic defence at 0.45. The
        // crossing point is therefore `1 - threshold` of remaining condition
        // (cadets and modules over the full complement, now that there are no
        // hit points), and
        // wanting out rises from nothing there to everything at destruction.
        // Getting this the wrong way round makes the stubborn doctrine the
        // first to run, which is what it did on the first attempt.
        let breaking = (1.0 - self.doctrine.withdraw_threshold).clamp(0.01, 1.0);
        let flight = ((breaking - condition) / breaking).clamp(0.0, 1.0);

        let decay = registry.planner.distance_decay;
        let mut best: Option<f32> = None;
        for (objective, held) in state.objectives() {
            // Ground reserved to the other side is somebody else's business.
            if !objective.open_to(side) {
                continue;
            }
            // What an exit *pays* and how badly a broken crew wants it are
            // different quantities, and multiplying by `value` the way ground
            // does conflates them. A retreat lane must be worth almost no
            // points — winning by running away is not winning — while still
            // pulling hard enough to cross a map. So an exit's urgency comes
            // from the crew's condition alone, scaled to be worth about as
            // much to a finished crew as a good objective is to a fresh one.
            let weight = match objective.kind {
                ObjectiveKind::Hold => {
                    // Ground already held is worth half: still worth sitting
                    // on, not worth marching across the map for.
                    let appetite = if held == Some(side) { 0.5 } else { 1.0 };
                    objective.value as f32 * self.doctrine.objective_value * appetite
                }
                ObjectiveKind::Exit => {
                    registry.planner.exit_urgency * self.doctrine.objective_value * flight
                }
            };
            if weight <= 0.0 {
                continue;
            }
            let distance = objective
                .hexes
                .iter()
                .map(|h| h.distance_to(tile))
                .min()
                .unwrap_or(0);
            let reward = if objective.contains(tile) { 1.5 } else { 0.0 };
            let score = weight * (reward - decay * distance as f32);
            if best.is_none_or(|b| score > b) {
                best = Some(score);
            }
        }
        best.unwrap_or(0.0)
    }

    /// What standing on `tile` is worth to a unit whose formation is under
    /// `mission`. The mission counterpart of [`Self::objective_value`], and
    /// the same shape on purpose: a reward for being there and a distance
    /// slope leading there, because a greedy one-round planner can only
    /// follow a gradient it can see from the tiles it can reach.
    ///
    /// The numbers are stated against the map-objective scale so they mean
    /// something, and they are data:
    /// [`PlannerRules::mission_weight`](crate::data::PlannerRules::mission_weight)
    /// ships at 2.0 because that is what a typical piece of ground is worth
    /// in the shipped maps, so "go where you were told" pulls about as hard
    /// as "take the ford". The slope is
    /// [`distance_decay`](crate::data::PlannerRules::distance_decay), shared
    /// with [`Self::objective_value`] precisely so that comparison holds —
    /// two slopes would make the two rewards incommensurable and quietly
    /// change the units `mission_weight` is quoted in. The balance harness's
    /// delegation-tax table is the instrument that judges them, and
    /// `--sweep planner.mission_weight=...` is how to ask.
    ///
    /// `delegation` is the strictness knob, and this is where it is finally
    /// read: a commander who devolves little expects the letter of the order
    /// followed, so the mission term is scaled by `1.5 - delegation` — a
    /// tight doctrine at 0.3 holds its units to the plan half again as hard
    /// as the neutral 0.5, and a loose one at 0.7 leaves room for the local
    /// terms (cover, threat, a good shot) to bend the route. A withdrawal is
    /// exempt on purpose: latitude is about *how* to fight, never about
    /// whether an ordered retreat happens.
    ///
    /// **A binding order raises the floor and nothing else.** The complaint
    /// this answers is that a player's order was quietly worth less because
    /// of who she gave it to: a loose doctrine read "take the ford" at 0.8
    /// where a tight one read it at 1.2, and no commander issues an order
    /// meaning four fifths of it. Under [`Latitude::Binding`] the same
    /// expression is clamped at 1.0 instead of 0.5, which says exactly the
    /// intended thing — **`delegation` may make a subordinate more literal
    /// than she was asked to be, never less.** Taking the doctrine out
    /// altogether was the other candidate and is wrong: it would make a
    /// binding order pull *less* than a delegated one for a tight doctrine,
    /// which is not a thing "I mean it" can be allowed to do.
    fn mission_value(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        tile: Hex,
        mission: &crate::battle::Mission,
        formation: &crate::battle::Formation,
        latitude: crate::battle::Latitude,
    ) -> f32 {
        use crate::battle::Mission;
        // Both read once rather than per arm: every arm below is the same
        // shape — a reward for being on the ground, a slope leading to it —
        // and that is what makes `mission_weight` quotable in objective-value
        // units.
        let weight = registry.planner.mission_weight;
        let decay = registry.planner.distance_decay;
        let doctrine = &self.doctrine;
        let floor = match latitude {
            crate::battle::Latitude::Delegated => 0.5,
            crate::battle::Latitude::Binding => 1.0,
        };
        let strictness = (1.5 - doctrine.delegation).clamp(floor, 1.5);
        match mission {
            // Take the ground and stand on it: reward for arriving, slope
            // for the road there — the objective shape with the commander
            // choosing the objective.
            //
            // An assault is worth exactly what an advance is worth, and one
            // arm says so rather than two copies of the same numbers: the
            // two orders differ in what they will *pay*, not in what the
            // ground is worth or which way it lies. That difference is the
            // contact damping in `score_tile`, which an assault does not
            // get.
            Mission::Advance { to } | Mission::Assault { to } => {
                let dist = to.distance_to(tile);
                let reward = if dist <= 1 { 1.5 } else { 0.0 };
                weight * strictness * doctrine.objective_value * (reward - decay * dist as f32)
            }
            // Stand where told. `None` anchors on the leader rather than a
            // stored hex or a centroid: she is where the formation is, the
            // anchor cannot drift as members wander the way a centroid
            // would, and for the leader herself every move scores worse
            // than staying — which is what holding *is*.
            Mission::Hold { at } => {
                let anchor = at.or_else(|| {
                    formation
                        .leader
                        .and_then(|id| state.unit(id))
                        .map(|u| u.pos)
                });
                match anchor {
                    Some(anchor) => {
                        let dist = anchor.distance_to(tile) as f32;
                        weight * strictness * doctrine.objective_value * (0.75 - decay * dist)
                    }
                    // Nobody left to anchor on: the mission has no ground to
                    // say anything about.
                    None => 0.0,
                }
            }
            // Go and look. Scaled by scouting rather than objective_value,
            // because it is the doctrine's appetite for unscouted ground
            // that says how hard eyes-forward pulls; the smaller reward
            // keeps a scout probing near the target rather than parking on
            // it as if it were a bridge to hold.
            Mission::Recon { toward } => {
                let dist = toward.distance_to(tile);
                let reward = if dist <= 2 { 0.75 } else { 0.0 };
                weight * strictness * doctrine.scouting * (reward - decay * dist as f32)
            }
            // Leave by the named lane. Deliberately ungated by damage: the
            // per-unit flight gate in `objective_value` exists because an
            // exit nobody was ordered to take must not pull, and being
            // ordered is exactly the permission it was standing in for.
            Mission::Withdraw { via } => {
                let lane = state
                    .map
                    .objectives()
                    .iter()
                    .find(|o| o.id == *via && o.kind == ObjectiveKind::Exit);
                match lane {
                    Some(objective) => {
                        let dist = objective
                            .hexes
                            .iter()
                            .map(|h| h.distance_to(tile))
                            .min()
                            .unwrap_or(0);
                        let reward = if objective.contains(tile) { 1.5 } else { 0.0 };
                        registry.planner.exit_urgency
                            * doctrine.objective_value
                            * (reward - decay * dist as f32)
                    }
                    // Validation refuses a mission naming no real exit, so
                    // this only happens if the lane was defined by a mod
                    // that then changed; score nothing rather than panic.
                    None => 0.0,
                }
            }
            // Base of fire. A band at supporting distance rather than a pull
            // toward the people supported: the attack term already pays her
            // for tiles with a shot on them, so what the mission has to say
            // is the *distance* — close enough that her fire lands where
            // theirs is needed, far enough that she is not in the assault
            // she is covering. Hugging them and trailing the map behind them
            // are both wrong and the shape says so, which is why this is
            // `|dist - STANDOFF|` and not a slope.
            //
            // Anchored on the supported formation's leader, falling back to
            // the lowest-id member still on the field — the same rule
            // `Hold`'s anchor uses and for the same reason: she is where the
            // formation is, and a centroid would drift every time a member
            // wandered. Nobody left alive to shoot for scores nothing; the
            // brain will notice at its next review and say something else.
            Mission::Support {
                formation: supported,
            } => {
                /// Supporting distance, in hexes: overwatch range on this
                /// scale (300 m), inside every gun on the field and outside
                /// the fight it is covering.
                const STANDOFF: f32 = 3.0;
                let anchor = state
                    .command
                    .formations()
                    .iter()
                    .find(|f| f.id == *supported)
                    .and_then(|f| {
                        f.leader
                            .and_then(|id| state.unit(id))
                            .or_else(|| f.members.iter().find_map(|id| state.unit(*id)))
                    })
                    .map(|u| u.pos);
                match anchor {
                    Some(anchor) => {
                        let dist = anchor.distance_to(tile) as f32;
                        weight
                            * strictness
                            * doctrine.objective_value
                            * (0.75 - decay * (dist - STANDOFF).abs())
                    }
                    None => 0.0,
                }
            }
        }
    }

    /// How the battle stands for `side`, in [-1, 1]. Aggressive doctrines
    /// count damage dealt for more than damage avoided, so search under those
    /// weights accepts trades a cautious one would refuse.
    pub fn position_value(&self, registry: &DataRegistry, state: &BattleState, side: u8) -> f32 {
        if let Some(result) = state.over {
            return match result.winner {
                Some(w) if w == side => 1.0,
                Some(_) => -1.0,
                None => 0.0,
            };
        }
        let mut ours = 0.0;
        let mut theirs = 0.0;
        for unit in state.alive_units() {
            // Substance points — cadets and module hits still aboard — are
            // the material currency now that hit points are gone. The
            // absolute scale differs from the old pool; only the ratio
            // below ever mattered.
            let weight = state.substance(registry, unit).0 as f32;
            if unit.side == side {
                ours += weight;
            } else {
                theirs += weight;
            }
        }
        let total = ours + theirs;
        if total <= 0.0 {
            return 0.0;
        }
        let weight_theirs = 0.5 + self.doctrine.aggression * 0.5;
        let weight_ours = 1.5 - weight_theirs;
        let material = (ours * weight_ours - theirs * weight_theirs) / total;

        // Ground counts too, or a search planner would happily trade away
        // every objective on the map for a favourable exchange of tanks and
        // then lose on points. Weighted against material rather than added to
        // it so the result stays inside [-1, 1] and the two are comparable.
        // Only ground that can be *held* counts here. An exit is scored by
        // driving off it, which shows up in the score rather than in who
        // stands where, and folding it in would dilute the fraction by ground
        // nobody can ever hold.
        let ground: f32 = state
            .objectives()
            .filter(|(o, _)| o.kind == ObjectiveKind::Hold)
            .map(|(objective, held)| match held {
                Some(s) if s == side => objective.value as f32,
                Some(_) => -(objective.value as f32),
                None => 0.0,
            })
            .sum();
        let at_stake: f32 = state
            .map
            .objectives()
            .iter()
            .filter(|o| o.kind == ObjectiveKind::Hold)
            .map(|o| o.value as f32)
            .sum::<f32>();
        if at_stake <= 0.0 {
            return material;
        }
        0.7 * material + 0.3 * (ground / at_stake)
    }

    fn is_indirect(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        weapon: usize,
    ) -> bool {
        state
            .unit(unit)
            .and_then(|u| registry.vehicle(&u.vehicle))
            .and_then(|v| v.weapons.get(weapon))
            .and_then(|w| registry.weapon(w))
            .is_some_and(|w| w.indirect)
    }
}
