//! A trained network, as content.
//!
//! REIMAGINED.md L0.5. A network is data the way a doctrine is: it is what a
//! commander makes of the ground, written down by a trainer instead of by
//! hand, and a mod can ship one or none. This is the file `training/`
//! writes — the feature names in order, the standardisation, and the layers
//! — and the forward pass over it, which is a few dozen lines of arithmetic
//! and so needs no dependency.
//!
//! **The names are a contract.** A network trained on one list of features
//! and fed another reads the wrong numbers silently, so [`GoalNet::check`]
//! compares the names it was trained on against the ones the engine
//! describes a decision with and refuses on any difference. A feature added
//! to `ai::features` therefore stops every old network loading, which is
//! the retraining REIMAGINED.md accepted, said out loud rather than
//! discovered as bad play.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One dense layer: `out = x · w + b`, `w` stored input-major as the trainer
/// writes it (`w[i][j]` is input `i` to output `j`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub w: Vec<Vec<f32>>,
    pub b: Vec<f32>,
}

/// A goal-scoring network: scores `concat(context, candidate)` for one
/// candidate goal. ReLU between layers and a single linear output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalNet {
    /// The context feature names it was trained on, in order.
    pub context: Vec<String>,
    /// The candidate feature names it was trained on, in order.
    pub candidate: Vec<String>,
    /// Per-input mean and standard deviation, applied before the first
    /// layer.
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    pub layers: Vec<Layer>,
    /// Whatever the trainer recorded about where it came from. Kept so a
    /// shipped network says what it was trained on; never read by a rule.
    #[serde(default)]
    pub trained_on: serde_json::Value,
    /// Inputs with the score the trainer's own forward pass gave them, so
    /// [`Self::check`] can prove this file's arithmetic is the trainer's.
    #[serde(default)]
    pub probe: Vec<Probe>,
}

/// One input the trainer scored, raw (before standardisation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    pub context: Vec<f32>,
    pub candidate: Vec<f32>,
    pub score: f32,
}

/// Why a network cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(
        "the network was trained on {which} features {trained:?}, the engine describes {engine:?}"
    )]
    Features {
        which: &'static str,
        trained: Vec<String>,
        engine: Vec<String>,
    },
    #[error("the network's shapes do not chain: {0}")]
    Shape(String),
    #[error("probe {index}: the engine scores {engine}, the trainer scored {trained}")]
    Probe {
        index: usize,
        engine: f32,
        trained: f32,
    },
}

impl GoalNet {
    /// Read a network from the JSON `training/` writes.
    pub fn from_path(path: &Path) -> Result<Self, NetError> {
        let text = std::fs::read_to_string(path).map_err(|source| NetError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| NetError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Refuse a network trained on different features from the ones the
    /// engine describes, or whose layers do not fit together.
    pub fn check(&self, context: &[String], candidate: &[&str]) -> Result<(), NetError> {
        if self.context != context {
            return Err(NetError::Features {
                which: "context",
                trained: self.context.clone(),
                engine: context.to_vec(),
            });
        }
        if self
            .candidate
            .iter()
            .map(String::as_str)
            .ne(candidate.iter().copied())
        {
            return Err(NetError::Features {
                which: "candidate",
                trained: self.candidate.clone(),
                engine: candidate.iter().map(|s| s.to_string()).collect(),
            });
        }
        let mut width = context.len() + candidate.len();
        if self.mean.len() != width || self.std.len() != width {
            return Err(NetError::Shape(format!(
                "standardisation is {} and {} wide, the input is {width}",
                self.mean.len(),
                self.std.len()
            )));
        }
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.w.len() != width {
                return Err(NetError::Shape(format!(
                    "layer {i} takes {} inputs, is given {width}",
                    layer.w.len()
                )));
            }
            let out = layer.b.len();
            if layer.w.iter().any(|row| row.len() != out) {
                return Err(NetError::Shape(format!(
                    "layer {i}'s rows are not {out} wide"
                )));
            }
            width = out;
        }
        if width != 1 {
            return Err(NetError::Shape(format!(
                "the output is {width} wide, not one score"
            )));
        }
        for (index, probe) in self.probe.iter().enumerate() {
            let engine = self.score(&probe.context, &probe.candidate);
            if (engine - probe.score).abs() > 1e-3 * (1.0 + probe.score.abs()) {
                return Err(NetError::Probe {
                    index,
                    engine,
                    trained: probe.score,
                });
            }
        }
        Ok(())
    }

    /// Score one candidate: `context` and `candidate` as the engine describes
    /// them. Assumes [`Self::check`] passed.
    pub fn score(&self, context: &[f32], candidate: &[f32]) -> f32 {
        let mut h: Vec<f32> = context
            .iter()
            .chain(candidate)
            .zip(self.mean.iter().zip(&self.std))
            .map(|(x, (m, s))| (x - m) / s)
            .collect();
        let last = self.layers.len().saturating_sub(1);
        for (i, layer) in self.layers.iter().enumerate() {
            let mut out = layer.b.clone();
            for (x, row) in h.iter().zip(&layer.w) {
                if *x == 0.0 {
                    continue;
                }
                for (o, w) in out.iter_mut().zip(row) {
                    *o += x * w;
                }
            }
            if i < last {
                for o in &mut out {
                    *o = o.max(0.0);
                }
            }
            h = out;
        }
        h.first().copied().unwrap_or(0.0)
    }
}
