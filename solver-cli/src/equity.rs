// SPDX-License-Identifier: AGPL-3.0-or-later

use postflop_solver::hand::Hand;
use postflop_solver::Card;

fn rank(hole: (Card, Card), board: &[Card; 5]) -> u16 {
    let mut hand = Hand::new();
    for &c in board {
        hand = hand.add_card(c as usize);
    }
    hand = hand.add_card(hole.0 as usize).add_card(hole.1 as usize);
    hand.evaluate()
}

fn blocks(a: (Card, Card), b: (Card, Card)) -> bool {
    a.0 == b.0 || a.0 == b.1 || a.1 == b.0 || a.1 == b.1
}

pub fn weighted_equity(
    hero_combo: (Card, Card),
    opp_combos: &[(Card, Card)],
    opp_weights: &[f32],
    board: &[Card; 5],
) -> f64 {
    assert_eq!(
        opp_combos.len(),
        opp_weights.len(),
        "combo and weight arrays must be the same length"
    );

    let ours = rank(hero_combo, board);
    let mut won = 0.0f64;
    let mut total = 0.0f64;

    for (opp, &w) in opp_combos.iter().zip(opp_weights) {
        if w <= 0.0 || blocks(hero_combo, *opp) {
            continue;
        }
        let theirs = rank(*opp, board);
        let share = if ours > theirs {
            1.0
        } else if ours < theirs {
            0.0
        } else {
            0.5
        };
        won += w as f64 * share;
        total += w as f64;
    }

    if total == 0.0 {
        return 0.5;
    }
    won / total
}
