// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::build::build_game_with_ranges;
use crate::spec::*;
use crate::{CliError, EXIT_INVALID_SPEC};
use postflop_solver::*;
use sha2::{Digest, Sha256};

const ENGINE_SHA: &str = env!("ENGINE_SHA");

pub fn engine_sha() -> &'static str {
    ENGINE_SHA
}

pub fn execute(path: &str) -> Result<String, CliError> {
    execute_reporting(path, false)
}

pub fn execute_reporting(path: &str, progress: bool) -> Result<String, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("cannot read {path}: {e}")))?;
    let spec: SolveSpec = serde_json::from_str(&text)
        .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad spec json: {e}")))?;

    let spec_hash = hash_spec(&text);
    let mut passes = Vec::with_capacity(1 + spec.locks.len());

    if spec.skip_baseline_pass && spec.freeze_unlocked_villain {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            "skip_baseline_pass with freeze_unlocked_villain would freeze villain \
             to nothing; pass 0 is where the baseline rows are harvested",
        ));
    }

    let frozen = if spec.skip_baseline_pass {
        None
    } else {
        let (baseline, frozen) = solve_one(&spec, None, None, progress)?;
        passes.push(baseline);
        frozen
    };

    for lock in &spec.locks {
        passes.push(solve_one(&spec, Some(lock), frozen.as_ref(), progress)?.0);
    }

    let result = SolveResult {
        spec_version: 3,
        engine_sha: ENGINE_SHA.to_string(),
        spec_hash,
        passes,
    };

    serde_json::to_string(&result)
        .map_err(|e| CliError::new(crate::EXIT_INTERNAL, format!("cannot serialize: {e}")))
}

fn hash_spec(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update(ENGINE_SHA.as_bytes());
    format!("{:x}", h.finalize())
}

type VillainBaseline = std::collections::BTreeMap<Vec<usize>, Vec<f32>>;

fn villain_player(game: &mut PostFlopGame, spec: &SolveSpec) -> usize {
    if let Some(player) = spec.villain_player {
        return player;
    }
    game.back_to_root();
    1 - game.current_player()
}

fn solve_one(
    spec: &SolveSpec,
    lock: Option<&LockSet>,
    frozen: Option<&VillainBaseline>,
    progress: bool,
) -> Result<(PassResult, Option<VillainBaseline>), CliError> {
    let (oop_override, ip_override) = match lock {
        Some(l) if l.kind == LockKind::RangeRestrict => {
            if !l.nodes.is_empty() {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!(
                        "lock set {:?} has kind range_restrict and {} node lock(s); \
                         range_restrict substitutes a range and solves freely, so \
                         node locks cannot also apply -- use kind range_zero instead",
                        l.name, l.nodes.len()
                    ),
                ));
            }
            if l.oop_range.is_none() && l.ip_range.is_none() {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!(
                        "lock set {:?} has kind range_restrict but supplies neither \
                         oop_range nor ip_range, so it would re-solve the baseline",
                        l.name
                    ),
                ));
            }
            (l.oop_range.as_deref(), l.ip_range.as_deref())
        }
        _ => (None, None),
    };

    let mut game = build_game_with_ranges(spec, oop_override, ip_override)?;
    game.allocate_memory(spec.compression);

    if game.current_board().len() < 5 && spec.emit.is_empty() && !spec.emit_full_walk {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            "spec.emit is required for a turn-or-deeper board (the full node walk is too large to default to)",
        ));
    }

    if let Some(lock) = lock.filter(|l| l.kind != LockKind::RangeRestrict) {
        for nl in &lock.nodes {
            game.back_to_root();
            game.apply_history(&nl.at);

            if game.is_terminal_node() {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!("lock '{}': at {:?} is a terminal node", lock.name, nl.at),
                ));
            }
            if game.is_chance_node() {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!("lock '{}': at {:?} is a chance node", lock.name, nl.at),
                ));
            }
            if game.current_player() != nl.player {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!(
                        "lock '{}': targets player {} but player {} acts at {:?}",
                        lock.name, nl.player, game.current_player(), nl.at
                    ),
                ));
            }

            let expected =
                game.available_actions().len() * game.private_cards(nl.player).len();
            if nl.strategy.len() != expected {
                return Err(CliError::new(
                    EXIT_INVALID_SPEC,
                    format!(
                        "lock '{}' at {:?} has {} entries, expected {} (n_actions * n_combos)",
                        lock.name, nl.at, nl.strategy.len(), expected
                    ),
                ));
            }

            if lock.kind != LockKind::Partial {
                let n_actions = game.available_actions().len();
                let n_combos = nl.strategy.len() / n_actions;
                for c in 0..n_combos {
                    if (0..n_actions).all(|a| nl.strategy[a * n_combos + c] <= 0.0) {
                        return Err(CliError::new(
                            EXIT_INVALID_SPEC,
                            format!(
                                "lock set {:?} at node {:?}: combo {} has zero weight on \
                                 every action, which the engine reads as UNLOCKED (that \
                                 combo would converge freely). Kind {:?} requires every \
                                 combo to be locked; use kind partial if leaving it free \
                                 is intended.",
                                lock.name, nl.at, c, lock.kind
                            ),
                        ));
                    }
                }
            }

            game.lock_current_strategy(&nl.strategy);
        }
        game.back_to_root();

        if spec.freeze_unlocked_villain {
            let Some(frozen) = frozen else {
                return Err(CliError::new(
                    crate::EXIT_INTERNAL,
                    "freeze_unlocked_villain is set but no baseline strategies were \
                     harvested; the baseline pass must run first",
                ));
            };
            let addressed: std::collections::BTreeSet<&Vec<usize>> =
                lock.nodes.iter().map(|nl| &nl.at).collect();

            for (at, strategy) in frozen {
                if addressed.contains(at) {
                    continue;
                }
                game.back_to_root();
                game.apply_history(at);
                if game.is_terminal_node() || game.is_chance_node() {
                    continue;
                }
                game.lock_current_strategy(strategy);
            }
            game.back_to_root();
        }
    }

    let name = lock.map(|l| l.name.clone()).unwrap_or_else(|| "baseline".to_string());
    let outcome = crate::progress::solve_reporting(
        &mut game,
        spec.max_iterations,
        match lock {
            Some(_) => spec
                .locked_target_exploitability
                .unwrap_or(spec.target_exploitability),
            None => spec.target_exploitability,
        },
        &name,
        progress,
    );
    let mes_ev = outcome.mes_ev.to_vec();
    let current_ev = compute_current_ev(&game).to_vec();
    let mut nodes = collect_nodes(
        &mut game,
        &spec.emit,
        &spec.emit_descendants_of,
        spec.emit_full_walk,
        spec.emit_depth,
    )?;
    if spec.river_equity {
        fill_weighted_equity(&mut nodes);
        fill_continue_equity(&mut nodes);
    }

    let semantics = match lock {
        None => "equilibrium",
        Some(l) => match l.kind {
            LockKind::Frequency | LockKind::RangeZero => "best_response",
            LockKind::RangeRestrict => "equilibrium_vs_narrowed_range",
            LockKind::Partial => "diagnostic_partial",
        },
    };

    let harvest = if lock.is_none() && spec.freeze_unlocked_villain {
        Some(harvest_villain_baseline(&mut game, &spec)?)
    } else {
        None
    };

    let (root_equity, root_combos) = if spec.emit_root_equity {
        game.back_to_root();
        game.cache_normalized_weights();
        let mut equity = Vec::with_capacity(2);
        let mut combos = Vec::with_capacity(2);
        for player in 0..2 {
            equity.push(game.equity(player));
            combos.push(
                holes_to_strings(game.private_cards(player))
                    .map_err(|e| CliError::new(crate::EXIT_INTERNAL, e))?,
            );
        }
        (equity, combos)
    } else {
        (Vec::new(), Vec::new())
    };

    Ok((
        PassResult {
            name,
            semantics: semantics.to_string(),
            iterations: outcome.iterations,
            exploitability: outcome.exploitability,
            nodes,
            root_equity,
            root_combos,
            mes_ev,
            current_ev,
        },
        harvest,
    ))
}

fn harvest_villain_baseline(
    game: &mut PostFlopGame,
    spec: &SolveSpec,
) -> Result<VillainBaseline, CliError> {
    let villain = villain_player(game, spec);
    let mut out = VillainBaseline::new();
    let mut stack = vec![Vec::<usize>::new()];

    while let Some(history) = stack.pop() {
        game.back_to_root();
        game.apply_history(&history);

        if game.is_terminal_node() {
            continue;
        }
        if game.is_chance_node() {
            let mask = game.possible_cards();
            for card in 0..52usize {
                if (mask >> card) & 1 == 1 {
                    let mut child = history.clone();
                    child.push(card);
                    stack.push(child);
                }
            }
            continue;
        }

        let n_actions = game.available_actions().len();
        if game.current_player() == villain {
            let strategy = game.strategy();

            let n_combos = game.private_cards(villain).len();
            for c in 0..n_combos {
                if (0..n_actions).all(|a| strategy[a * n_combos + c] <= 0.0) {
                    return Err(CliError::new(
                        crate::EXIT_INTERNAL,
                        format!(
                            "baseline strategy at node {history:?} has combo {c} at zero \
                             on every action; the engine would read that row as UNLOCKED \
                             and leave the combo free to adapt, which is the opposite of \
                             freezing it"
                        ),
                    ));
                }
            }
            out.insert(history.clone(), strategy);
        }

        for i in 0..n_actions {
            let mut child = history.clone();
            child.push(i);
            stack.push(child);
        }
    }

    Ok(out)
}

fn collect_nodes(
    game: &mut PostFlopGame,
    emit: &[EmitTarget],
    emit_descendants_of: &[Vec<usize>],
    emit_full_walk: bool,
    emit_depth: usize,
) -> Result<Vec<NodeResult>, CliError> {
    let mut out = if emit_full_walk || emit.is_empty() {
        collect_nodes_full_walk(game)?
    } else {
        let mut out = Vec::new();
        for target in emit {
            if let Some(node) = collect_one_node(game, &target.at)? {
                out.push(node);
            }
        }
        out
    };

    for at in emit_descendants_of {
        out.extend(collect_descendants(game, at)?);
    }

    out.sort_by(|a, b| a.history.cmp(&b.history));
    out.dedup_by(|a, b| a.history == b.history);

    for _ in 0..emit_depth {
        let known: Vec<Vec<usize>> = out.iter().map(|n| n.history.clone()).collect();
        let before = out.len();
        for history in &known {
            game.back_to_root();
            game.apply_history(history);
            if game.is_terminal_node() {
                continue;
            }
            out.extend(collect_descendants(game, history)?);
        }
        out.sort_by(|a, b| a.history.cmp(&b.history));
        out.dedup_by(|a, b| a.history == b.history);
        if out.len() == before {
            break;
        }
    }

    Ok(out)
}

fn collect_nodes_full_walk(game: &mut PostFlopGame) -> Result<Vec<NodeResult>, CliError> {
    let mut out = Vec::new();
    let mut stack = vec![Vec::<usize>::new()];

    while let Some(history) = stack.pop() {
        game.back_to_root();
        game.apply_history(&history);

        if game.is_terminal_node() {
            continue;
        }

        if game.is_chance_node() {
            let mask = game.possible_cards();
            for card in 0..52usize {
                if (mask >> card) & 1 == 1 {
                    let mut child = history.clone();
                    child.push(card);
                    stack.push(child);
                }
            }
            continue;
        }

        let n_actions = game.available_actions().len();
        out.push(build_node_result(game, history.clone())?);

        for i in 0..n_actions {
            let mut child = history.clone();
            child.push(i);
            stack.push(child);
        }
    }

    Ok(out)
}

fn build_node_result(game: &mut PostFlopGame, history: Vec<usize>) -> Result<NodeResult, CliError> {
    let player = game.current_player();
    game.cache_normalized_weights();
    Ok(NodeResult {
        history,
        player,
        board: current_board_strings(game),
        actions: game.available_actions().iter().map(|a| format!("{a:?}")).collect(),
        combos: holes_to_strings(game.private_cards(player))
            .map_err(|e| CliError::new(crate::EXIT_INTERNAL, e))?,
        strategy: game.strategy(),
        ev_detail: game.expected_values_detail(player),
        equity: game.equity(player),
        weights: (0..2).map(|p| game.normalized_weights(p).to_vec()).collect(),
        reach_weights: (0..2).map(|p| game.weights(p).to_vec()).collect(),
        weighted_equity: Vec::new(),
        continue_equity: std::collections::BTreeMap::new(),
    })
}

fn collect_one_node(game: &mut PostFlopGame, at: &[usize]) -> Result<Option<NodeResult>, CliError> {
    game.back_to_root();
    game.apply_history(at);
    if game.is_terminal_node() || game.is_chance_node() {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            format!("emit target {:?} is not a decision node", at),
        ));
    }
    Ok(Some(build_node_result(game, at.to_vec())?))
}

fn collect_descendants(game: &mut PostFlopGame, at: &[usize]) -> Result<Vec<NodeResult>, CliError> {
    game.back_to_root();
    game.apply_history(at);
    if game.is_terminal_node() {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            format!("emit_descendants_of target {:?} is a terminal node", at),
        ));
    }

    let mut out = Vec::new();
    let mut frontier = vec![at.to_vec()];

    loop {
        let mut next_frontier = Vec::new();
        let mut all_decisions = true;

        for history in &frontier {
            game.back_to_root();
            game.apply_history(history);

            if history == at {
                if game.is_chance_node() {
                    let mask = game.possible_cards();
                    for card in 0..52usize {
                        if (mask >> card) & 1 == 1 {
                            let mut child = history.clone();
                            child.push(card);
                            next_frontier.push(child);
                        }
                    }
                } else {
                    let n_actions = game.available_actions().len();
                    for i in 0..n_actions {
                        let mut child = history.clone();
                        child.push(i);
                        next_frontier.push(child);
                    }
                }
                all_decisions = false;
                continue;
            }

            if game.is_chance_node() {
                all_decisions = false;
                let mask = game.possible_cards();
                for card in 0..52usize {
                    if (mask >> card) & 1 == 1 {
                        let mut child = history.clone();
                        child.push(card);
                        next_frontier.push(child);
                    }
                }
            } else if !game.is_terminal_node() {
                out.push(build_node_result(game, history.clone())?);
            }
        }

        frontier = next_frontier;
        if all_decisions || frontier.is_empty() {
            break;
        }
    }

    Ok(out)
}

fn fill_weighted_equity(nodes: &mut Vec<NodeResult>) {
    let snapshot = nodes.clone();

    for i in 0..nodes.len() {
        let node = &snapshot[i];
        if node.board.len() != 5 {
            continue;
        }
        let opponent = 1 - node.player;

        let ancestor = snapshot
            .iter()
            .filter(|a| {
                a.player == opponent
                    && a.history.len() < node.history.len()
                    && node.history.starts_with(&a.history)
            })
            .max_by_key(|a| a.history.len());

        let Some(ancestor) = ancestor else { continue };

        let n = ancestor.combos.len();
        let n_actions = ancestor.actions.len();
        let weights: Vec<f32> = (0..n)
            .map(|c| (1..n_actions).map(|a| ancestor.strategy[a * n + c]).sum())
            .collect();

        let opp_combos: Vec<(Card, Card)> = ancestor
            .combos
            .iter()
            .map(|s| parse_combo(s))
            .collect();

        let our_combos: Vec<(Card, Card)> =
            node.combos.iter().map(|s| parse_combo(s)).collect();

        let board = parse_board_strings(&node.board);
        nodes[i].weighted_equity = our_combos
            .iter()
            .map(|&c| crate::equity::weighted_equity(c, &opp_combos, &weights, &board))
            .collect();
    }
}

fn fill_continue_equity(nodes: &mut Vec<NodeResult>) {
    let snapshot = nodes.clone();

    for i in 0..nodes.len() {
        let node = &snapshot[i];
        if node.board.len() != 5 {
            continue;
        }

        for bet in 1..node.actions.len() {
            let mut child_history = node.history.clone();
            child_history.push(bet);

            let Some(child) = snapshot.iter().find(|c| c.history == child_history) else {
                continue;
            };

            let Some(call_index) = child.actions.iter().position(|a| a == "Call") else {
                continue;
            };

            let n = child.combos.len();
            let weights: Vec<f32> = (0..n)
                .map(|c| child.strategy[call_index * n + c])
                .collect();

            let opp_combos: Vec<(Card, Card)> =
                child.combos.iter().map(|s| parse_combo(s)).collect();
            let our_combos: Vec<(Card, Card)> =
                node.combos.iter().map(|s| parse_combo(s)).collect();

            let board = parse_board_strings(&node.board);
            let equities: Vec<f64> = our_combos
                .iter()
                .map(|&c| crate::equity::weighted_equity(c, &opp_combos, &weights, &board))
                .collect();

            nodes[i].continue_equity.insert(bet, equities);
        }
    }
}

fn parse_combo(s: &str) -> (Card, Card) {
    let c: Vec<char> = s.chars().collect();
    let a: String = c[0..2].iter().collect();
    let b: String = c[2..4].iter().collect();
    (
        card_from_str(&a).expect("engine-produced combo must parse"),
        card_from_str(&b).expect("engine-produced combo must parse"),
    )
}

fn current_board_strings(game: &PostFlopGame) -> Vec<String> {
    game.current_board()
        .iter()
        .map(|&c| card_to_string(c).expect("engine-produced card must format"))
        .collect()
}

fn parse_board_strings(board: &[String]) -> [Card; 5] {
    let mut out = [0; 5];
    for (i, s) in board.iter().enumerate() {
        out[i] = card_from_str(s).expect("engine-produced board card must parse");
    }
    out
}
