// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::spec::{KeepOnly, SolveSpec};
use crate::{CliError, EXIT_CARD_COLLISION, EXIT_INVALID_SPEC};
use postflop_solver::*;

fn parse_board(board: &str) -> Result<([Card; 3], Card, Card), CliError> {
    let chars: Vec<char> = board.chars().collect();
    let n = chars.len();

    if n != 6 && n != 8 && n != 10 {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            format!(
                "board must be 6 chars (flop), 8 (turn) or 10 (river), got {:?}",
                board
            ),
        ));
    }

    let mut cards = Vec::with_capacity(n / 2);
    for pair in chars.chunks(2) {
        let s: String = pair.iter().collect();
        let card = card_from_str(&s)
            .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad card {s:?}: {e}")))?;
        cards.push(card);
    }

    let flop = [cards[0], cards[1], cards[2]];
    let turn = if cards.len() >= 4 { cards[3] } else { NOT_DEALT };
    let river = if cards.len() == 5 { cards[4] } else { NOT_DEALT };
    Ok((flop, turn, river))
}

pub fn build_game(spec: &SolveSpec) -> Result<PostFlopGame, CliError> {
    build_game_with_ranges(spec, None, None)
}

pub fn build_game_with_ranges<'a>(
    spec: &'a SolveSpec,
    oop_override: Option<&str>,
    ip_override: Option<&str>,
) -> Result<PostFlopGame, CliError> {
    if spec.spec_version != 3 {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            format!("unsupported spec_version {}", spec.spec_version),
        ));
    }
    if spec.bet_sizes.len() != 2 {
        return Err(CliError::new(
            EXIT_INVALID_SPEC,
            "bet_sizes must have exactly 2 entries (OOP, IP)",
        ));
    }

    let (flop, turn, river) = parse_board(&spec.board)?;
    let initial_state = if turn == NOT_DEALT {
        BoardState::Flop
    } else if river == NOT_DEALT {
        BoardState::Turn
    } else {
        BoardState::River
    };

    let oop_text = oop_override.unwrap_or(&spec.oop_range);
    let ip_text = ip_override.unwrap_or(&spec.ip_range);

    let oop = oop_text.parse::<Range>()
        .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad oop_range: {e}")))?;
    let ip = ip_text.parse::<Range>()
        .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad ip_range: {e}")))?;

    let card_config = CardConfig { range: [oop, ip], flop, turn, river };

    let raises = |street: &'a Vec<String>, label: &str, i: usize| -> Result<&'a str, CliError> {
        let (source, named) = if street.len() == 2 || !street.is_empty() {
            (street, label)
        } else {
            (&spec.raise_sizes, "raise_sizes")
        };
        match source.len() {
            0 => Ok(""),
            2 => Ok(source[i].as_str()),
            n => Err(CliError::new(
                EXIT_INVALID_SPEC,
                format!("{named} must be empty or [oop, ip]; got {n} entries"),
            )),
        }
    };

    let menu = |street: &'a Vec<String>,
                street_raises: &'a Vec<String>,
                label: &str,
                i: usize|
     -> Result<BetSizeOptions, CliError> {
        let source = if street.len() == 2 { street } else { &spec.bet_sizes };
        if !street.is_empty() && street.len() != 2 {
            return Err(CliError::new(
                EXIT_INVALID_SPEC,
                format!("a per-street bet size list must be [oop, ip]; got {} entries", street.len()),
            ));
        }
        (source[i].as_str(), raises(street_raises, label, i)?)
            .try_into()
            .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad bet size: {e}")))
    };

    let tree_config = TreeConfig {
        initial_state,
        starting_pot: spec.starting_pot,
        effective_stack: spec.effective_stack,
        flop_bet_sizes: [
            menu(&spec.flop_bet_sizes, &spec.flop_raise_sizes, "flop_raise_sizes", 0)?,
            menu(&spec.flop_bet_sizes, &spec.flop_raise_sizes, "flop_raise_sizes", 1)?,
        ],
        turn_bet_sizes: [
            menu(&spec.turn_bet_sizes, &spec.turn_raise_sizes, "turn_raise_sizes", 0)?,
            menu(&spec.turn_bet_sizes, &spec.turn_raise_sizes, "turn_raise_sizes", 1)?,
        ],
        river_bet_sizes: [
            menu(&spec.river_bet_sizes, &spec.river_raise_sizes, "river_raise_sizes", 0)?,
            menu(&spec.river_bet_sizes, &spec.river_raise_sizes, "river_raise_sizes", 1)?,
        ],
        force_allin_threshold: spec.force_allin_threshold,
        add_allin_threshold: spec.add_allin_threshold,
        merging_threshold: spec.merging_threshold,
        ..Default::default()
    };

    let mut action_tree = ActionTree::new(tree_config)
        .map_err(|e| CliError::new(EXIT_INVALID_SPEC, format!("bad tree: {e}")))?;

    for keep in &spec.keep_only {
        apply_keep_only(&mut action_tree, keep)?;
    }
    action_tree.back_to_root();

    PostFlopGame::with_config(card_config, action_tree)
        .map_err(|e| CliError::new(EXIT_CARD_COLLISION, format!("invalid card config: {e}")))
}

fn label(action: &Action) -> String {
    format!("{action:?}")
}

fn is_aggressive(action: &Action) -> bool {
    matches!(action, Action::Bet(_) | Action::Raise(_) | Action::AllIn(_))
}

fn apply_keep_only(tree: &mut ActionTree, keep: &KeepOnly) -> Result<(), CliError> {
    let bad = |msg: String| CliError::new(EXIT_INVALID_SPEC, format!("keep_only: {msg}"));
    tree.back_to_root();
    for step in &keep.at {
        let next = tree
            .available_actions()
            .iter()
            .copied()
            .find(|a| &label(a) == step)
            .ok_or_else(|| bad(format!("no action {step:?} on the path {:?}", keep.at)))?;
        tree.play(next).map_err(bad)?;
    }
    let offered: Vec<Action> = tree.available_actions().to_vec();
    let kept = offered
        .iter()
        .find(|a| label(a) == keep.action)
        .ok_or_else(|| bad(format!("{:?} is not offered at {:?}", keep.action, keep.at)))?;
    if !is_aggressive(kept) {
        return Err(bad(format!("{:?} is not a bet or raise; only a size can be forced", keep.action)));
    }
    for other in offered.iter().filter(|a| is_aggressive(a) && label(a) != keep.action) {
        tree.remove_action(*other).map_err(bad)?;
    }
    Ok(())
}
