//! Cores and skills: what a girl is made of, declared in mod data.
//!
//! A girl's abilities are never written down. What is stored is her
//! temperament (cores) and what she has been taught (skills); everything she
//! can *do* is worked out at the point of use. See
//! `assets/wiki/reference/girls.md` for why.
//!
//! # Everything is a skill check
//!
//! There are no derived stats — no stored "reactivity" or "perception". Each
//! [`SkillDef`] names the cores it draws on, and asking how well someone can do
//! something is [`SkillDef::level_for`]. Firing a gun, spotting a tank, working
//! a radio and holding a formation together are all skill checks with different
//! core mixes.
//!
//! Two reasons, one design and one practical. A check happens at a place and a
//! time, so terrain, suppression and traits are modifiers arriving where they
//! are needed rather than corrections bolted onto a cached number. And adding a
//! weapon that rewards a steady hand, or a whole new kind of order, is data.
//!
//! # Untrained is not zero
//!
//! Borrowed from GURPS: a skill nobody has taught you *defaults* to its
//! controlling cores at a penalty. An average girl who has never been near a
//! gun still shoots, at `hands - 4`, which is bad but not helpless. The effect
//! is that cores dominate when training is thin and fade as it grows, which is
//! the relationship between talent and experience the campaign wants — and it
//! falls out of one subtraction.
//!
//! # Scale
//!
//! Centred on 10. 8-12 is ordinary, 14 and above is remarkable, 6 is a real
//! weakness. The untrained penalties only mean anything on a scale with room
//! to subtract from, which is why this is not the old 0-5.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The average value of any core. Definitions and generated recruits are
/// written relative to this.
pub const AVERAGE: i32 = 10;

/// One axis of temperament, declared by a mod.
///
/// Which cores exist is content, not Rust: another game on this engine wants a
/// different set, and even this one may grow one. They are resolved to indices
/// when the registry loads, so a check costs an array index rather than a
/// string hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// A trainable skill, and the cores it leans on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDef {
    pub id: String,
    pub name: String,
    /// Core id to weight. Weights are relative: `{"hands": 2, "wits": 1}` is
    /// two parts coordination to one part quickness, and the weighted mean is
    /// what an untrained girl falls back on.
    pub cores: HashMap<String, i32>,
    /// How far below her cores an untrained girl performs. Higher for skills
    /// that are genuinely specialised — you can flail at a gun, but you cannot
    /// improvise field surgery.
    #[serde(default = "default_untrained_penalty")]
    pub untrained_penalty: i32,
}

fn default_untrained_penalty() -> i32 {
    4
}

impl SkillDef {
    /// The weighted mean of this skill's controlling cores.
    ///
    /// `cores` is indexed the way the registry orders them; a core this skill
    /// names but the mod does not define contributes nothing, which is the
    /// same forgiving behaviour the rest of the loader has.
    pub fn core_base(&self, index: &CoreIndex, cores: &[i32]) -> i32 {
        let mut total = 0;
        let mut weight = 0;
        for (id, w) in &self.cores {
            if let Some(value) = index.get(id).and_then(|i| cores.get(i)) {
                total += value * w;
                weight += w;
            }
        }
        if weight == 0 {
            return AVERAGE;
        }
        // Rounded rather than truncated: truncation would quietly bias every
        // untrained check downward.
        (total as f32 / weight as f32).round() as i32
    }

    /// How well someone with these cores and this much training performs.
    ///
    /// `trained` is `None` for a skill she has never been taught, which is
    /// where the default applies.
    pub fn level_for(&self, index: &CoreIndex, cores: &[i32], trained: Option<i32>) -> i32 {
        match trained {
            Some(level) => level,
            None => self.core_base(index, cores) - self.untrained_penalty,
        }
    }
}

/// Core ids resolved to positions, so a check is an array index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreIndex {
    order: Vec<String>,
    lookup: HashMap<String, usize>,
}

impl CoreIndex {
    pub fn build(cores: &[CoreDef]) -> Self {
        let order: Vec<String> = cores.iter().map(|c| c.id.clone()).collect();
        let lookup = order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        Self { order, lookup }
    }

    pub fn get(&self, id: &str) -> Option<usize> {
        self.lookup.get(id).copied()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Core ids in registry order, which is the order a girl's values are
    /// stored in.
    pub fn ids(&self) -> &[String] {
        &self.order
    }

    /// Turn an id-keyed map from a definition file into positional values.
    /// Anything the mod does not name sits at [`AVERAGE`], so a character can
    /// state only what makes her unusual.
    pub fn values_from(&self, named: &HashMap<String, i32>) -> Vec<i32> {
        let mut values = vec![AVERAGE; self.order.len()];
        for (id, value) in named {
            if let Some(i) = self.get(id) {
                values[i] = *value;
            }
        }
        values
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> CoreIndex {
        CoreIndex::build(&[
            CoreDef {
                id: "hands".into(),
                name: "Hands".into(),
                description: String::new(),
            },
            CoreDef {
                id: "wits".into(),
                name: "Wits".into(),
                description: String::new(),
            },
        ])
    }

    fn gunnery() -> SkillDef {
        SkillDef {
            id: "gunnery".into(),
            name: "Gunnery".into(),
            cores: HashMap::from([("hands".into(), 2), ("wits".into(), 1)]),
            untrained_penalty: 4,
        }
    }

    #[test]
    fn an_untrained_skill_falls_back_to_its_cores_at_a_penalty() {
        let (index, skill) = (index(), gunnery());
        // Hands 12, Wits 9 -> weighted mean 11, less the penalty.
        let cores = vec![12, 9];
        assert_eq!(skill.core_base(&index, &cores), 11);
        assert_eq!(skill.level_for(&index, &cores, None), 7);
    }

    #[test]
    fn training_replaces_the_default_rather_than_adding_to_it() {
        // The whole point of the model: cores dominate when training is thin
        // and stop mattering once she has actually been taught.
        let (index, skill) = (index(), gunnery());
        let gifted = vec![16, 16];
        let ordinary = vec![10, 10];
        assert_eq!(skill.level_for(&index, &gifted, Some(13)), 13);
        assert_eq!(skill.level_for(&index, &ordinary, Some(13)), 13);
        // Untrained, the gap between them is the whole difference.
        assert!(skill.level_for(&index, &gifted, None) > skill.level_for(&index, &ordinary, None));
    }

    #[test]
    fn a_character_only_states_what_makes_her_unusual() {
        let index = index();
        let values = index.values_from(&HashMap::from([("wits".into(), 15)]));
        assert_eq!(values, vec![AVERAGE, 15], "unnamed cores sit at average");
    }

    #[test]
    fn a_skill_naming_a_core_the_mod_does_not_define_still_works() {
        let index = index();
        let mut skill = gunnery();
        skill.cores.insert("telepathy".into(), 5);
        // The unknown core contributes nothing rather than poisoning the mean.
        assert_eq!(skill.core_base(&index, &[12, 9]), 11);
    }
}
