// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolveSpec {
    pub spec_version: u32,
    pub board: String,
    pub oop_range: String,
    pub ip_range: String,
    pub starting_pot: i32,
    pub effective_stack: i32,
    pub bet_sizes: Vec<String>,
    #[serde(default)]
    pub flop_bet_sizes: Vec<String>,
    #[serde(default)]
    pub turn_bet_sizes: Vec<String>,
    #[serde(default)]
    pub river_bet_sizes: Vec<String>,
    #[serde(default)]
    pub compression: bool,
    pub max_iterations: u32,
    pub target_exploitability: f32,
    #[serde(default)]
    pub locked_target_exploitability: Option<f32>,
    #[serde(default)]
    pub locks: Vec<LockSet>,
    #[serde(default)]
    pub emit: Vec<EmitTarget>,
    #[serde(default)]
    pub emit_descendants_of: Vec<Vec<usize>>,
    #[serde(default)]
    pub emit_full_walk: bool,
    #[serde(default)]
    pub emit_depth: usize,
    #[serde(default)]
    pub emit_root_equity: bool,
    #[serde(default = "default_true")]
    pub river_equity: bool,
    #[serde(default)]
    pub raise_sizes: Vec<String>,
    #[serde(default)]
    pub flop_raise_sizes: Vec<String>,
    #[serde(default)]
    pub turn_raise_sizes: Vec<String>,
    #[serde(default)]
    pub river_raise_sizes: Vec<String>,
    #[serde(default)]
    pub freeze_unlocked_villain: bool,
    #[serde(default)]
    pub villain_player: Option<usize>,
    #[serde(default)]
    pub skip_baseline_pass: bool,
    #[serde(default)]
    pub force_allin_threshold: f64,
    #[serde(default)]
    pub add_allin_threshold: f64,
    #[serde(default)]
    pub merging_threshold: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep_only: Vec<KeepOnly>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KeepOnly {
    pub at: Vec<String>,
    pub action: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitTarget {
    pub at: Vec<usize>,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockKind {
    Frequency,
    Partial,
    RangeRestrict,
    RangeZero,
}

impl Default for LockKind {
    fn default() -> Self {
        LockKind::Frequency
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockSet {
    pub name: String,
    #[serde(default)]
    pub kind: LockKind,
    #[serde(default)]
    pub oop_range: Option<String>,
    #[serde(default)]
    pub ip_range: Option<String>,
    pub nodes: Vec<NodeLock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLock {
    pub at: Vec<usize>,
    pub player: usize,
    pub strategy: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SolveResult {
    pub spec_version: u32,
    pub engine_sha: String,
    pub spec_hash: String,
    pub passes: Vec<PassResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PassResult {
    pub name: String,
    pub semantics: String,
    pub iterations: u32,
    pub exploitability: f32,
    pub nodes: Vec<NodeResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_equity: Vec<Vec<f32>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_combos: Vec<Vec<String>>,
    pub mes_ev: Vec<f32>,
    pub current_ev: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeResult {
    pub history: Vec<usize>,
    pub player: usize,
    pub board: Vec<String>,
    pub actions: Vec<String>,
    pub combos: Vec<String>,
    pub strategy: Vec<f32>,
    pub ev_detail: Vec<f32>,
    pub equity: Vec<f32>,
    pub weights: Vec<Vec<f32>>,
    pub reach_weights: Vec<Vec<f32>>,
    pub weighted_equity: Vec<f64>,
    pub continue_equity: BTreeMap<usize, Vec<f64>>,
}
