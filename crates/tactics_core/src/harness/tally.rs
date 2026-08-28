//! One run's accounting, and the rule for folding two of them together.

use std::collections::{BTreeMap, HashMap};

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Tally {
    pub wins: HashMap<String, usize>,
    pub draws: usize,
    pub stalemates: usize,
    pub rounds: Vec<u32>,
    pub kills: HashMap<String, usize>,
    pub deaths: HashMap<String, usize>,
    pub shots: u32,
    /// Of those shots, the ones laid from a vehicle that had driven this
    /// round. The arc that added the motion terms is a claim about how much
    /// of the shooting happens under way, and this is the number that
    /// settles it: a resolver term nobody's guns ever meet is a term that
    /// changed nothing.
    pub shots_on_the_move: u32,
    pub hits: u32,
    pub bounces: u32,
    pub misses: u32,
    /// Misses that found somebody else standing on the target's hex. Stacking
    /// is worth nothing unless crews actually share ground, and the stray rule
    /// is worth nothing unless misses land among them — these three numbers are
    /// the check that neither is a term nobody in the game ever meets, which is
    /// what `balance.blind_penalty` turned out to be.
    pub strays: u32,
    /// Rounds that opened with somebody sharing a hex, and the largest stack
    /// seen anywhere in the run.
    pub stacked_rounds: u32,
    pub deepest_stack: usize,
    pub hits_by_arc: HashMap<String, u32>,
    /// What actually ended each vehicle, by the flag she died carrying.
    pub causes: BTreeMap<&'static str, usize>,
    pub girls_wounded: usize,
    pub girls_out: usize,
    /// Rounds that drove out, and rounds still aboard at the end, by ammo
    /// id. The difference is what the battle cost in ammunition.
    pub ammo_aboard: BTreeMap<String, u32>,
    pub ammo_left: BTreeMap<String, u32>,
    pub racks_destroyed: usize,
    pub shells: u32,
    pub shells_on_target: u32,
    pub shells_bounced: u32,
    /// Infantry, which the kills/losses table already reports per chassis —
    /// the rows appear on their own the moment a map fields them. What that
    /// table cannot say is the thing that makes a platoon different from a
    /// tank: she is worn down rather than killed, so a run in which no foot
    /// unit dies can still be one in which every platoon was shot to pieces.
    /// These four numbers are that story: how many took the field, what
    /// fraction of their rifles the survivors still had at the bell, and how
    /// much of the riding actually happened.
    pub foot_fielded: usize,
    pub troops_left: Vec<f32>,
    pub mounts: usize,
    pub dismounts: usize,
    /// Contacts made, and the round each crew was first found in.
    ///
    /// A detection roll does not change what a crew can *see*, it changes how
    /// long she takes to pick somebody out of it — so the thing to watch is
    /// when each vehicle stopped being hidden, not when the battle's first
    /// contact happened. One easy spot on round one would mask every other
    /// crew on the field, and the first draft of this measured exactly that
    /// and reported no difference at any setting.
    ///
    /// `spots` counts every acquisition, re-acquisitions included, which is
    /// the other half: contact broken and remade is what a screen is for.
    pub spots: u32,
    pub found_at: Vec<u32>,
    /// How far the nearest crew of the finding side was when each contact was
    /// made, in hexes.
    ///
    /// This is the headline number for the detection rules, and the reason
    /// the two above it move so little: delaying a spot does not usually
    /// delay the *battle*, because both sides are closing anyway. What it
    /// does is let them close. A contact made at eight hexes instead of
    /// sixteen is a different fight — the same crews, a kilometre nearer —
    /// even when the round it happens in barely moves.
    ///
    /// Deliberately not split by whether the crew who was found had driven,
    /// which the first draft did and which measured the wrong thing entirely:
    /// `moved` is zeroed at the top of every round, so "halted" at tick three
    /// means "has not driven yet", and a *delayed* contact therefore moves
    /// out of the halted bucket by construction. It reported that detection
    /// rolls made stationary crews easier to find.
    pub contact_range: Vec<u32>,
    /// Shots laid at a map reference rather than at somebody anybody could
    /// see. The direct twin of the above on the shooting side, and the one
    /// number that says whether hiding is worth anything: a gun that can
    /// always see its target never pays `balance.blind_penalty`.
    ///
    /// **It reads zero in every run so far, and it is kept anyway.** Nothing
    /// in the AI ever chooses to shell ground it cannot see, so this is
    /// currently measuring the absence of a decision rather than the outcome
    /// of one — which is the same thing `blind_penalty` itself turned out to
    /// be. The reason to keep the column is that the decision is coming:
    /// suppressing fire is fire at a place rather than at a crew, and the
    /// moment a planner learns to lay it, this is the number that will say so
    /// without anybody having to add an instrument in the same breath as the
    /// feature it is meant to judge.
    pub shots_blind: u32,
}

impl Tally {
    /// Fold one battle's accounting into a run's.
    ///
    /// **Every field must fold associatively**: `(a + b) + c` has to equal
    /// `a + (b + c)`, because the batch is split across the machine and the
    /// only thing guaranteed is that results are folded in *job order*, not
    /// how they were divided into chunks. Sums, concatenations, unions of sums
    /// and maxima all qualify. What does not is a ratio, or a last-value-wins:
    /// either would make a printed number depend on which core finished first,
    /// which is the one kind of wrong this harness must not be, because it
    /// looks exactly like noise.
    ///
    /// This comment used to say every field was "a sum, a concatenation or a
    /// union of sums" and listed *a maximum* among the things that would break
    /// it. `deepest_stack` has been a maximum all along and is perfectly safe;
    /// the rule was stated one notch too tight. Associativity is the real
    /// requirement, and it is checked in `tests/harness.rs` rather than
    /// asserted here.
    ///
    /// Note what is deliberately **not** required: commutativity. The sample
    /// vectors concatenate, so `a + b` and `b + a` differ in the order of
    /// `rounds` — which is exactly why `run_all` guarantees job order, and why
    /// that guarantee has a test of its own.
    pub fn merge(&mut self, other: &Tally) {
        for (k, v) in &other.wins {
            *self.wins.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.kills {
            *self.kills.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.deaths {
            *self.deaths.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.hits_by_arc {
            *self.hits_by_arc.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.causes {
            *self.causes.entry(k).or_default() += v;
        }
        for (k, v) in &other.ammo_aboard {
            *self.ammo_aboard.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &other.ammo_left {
            *self.ammo_left.entry(k.clone()).or_default() += v;
        }
        self.draws += other.draws;
        self.stalemates += other.stalemates;
        self.rounds.extend_from_slice(&other.rounds);
        self.shots += other.shots;
        self.shots_on_the_move += other.shots_on_the_move;
        self.hits += other.hits;
        self.bounces += other.bounces;
        self.misses += other.misses;
        self.strays += other.strays;
        self.stacked_rounds += other.stacked_rounds;
        self.deepest_stack = self.deepest_stack.max(other.deepest_stack);
        self.girls_wounded += other.girls_wounded;
        self.girls_out += other.girls_out;
        self.racks_destroyed += other.racks_destroyed;
        self.shells += other.shells;
        self.shells_on_target += other.shells_on_target;
        self.shells_bounced += other.shells_bounced;
        self.foot_fielded += other.foot_fielded;
        self.troops_left.extend_from_slice(&other.troops_left);
        self.mounts += other.mounts;
        self.dismounts += other.dismounts;
        self.spots += other.spots;
        self.found_at.extend_from_slice(&other.found_at);
        self.contact_range.extend_from_slice(&other.contact_range);
        self.shots_blind += other.shots_blind;
    }
}
