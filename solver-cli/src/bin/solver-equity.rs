// SPDX-License-Identifier: AGPL-3.0-or-later

use postflop_solver::card_from_str;
use postflop_solver::hand::Hand;
use postflop_solver::Card;
use std::process::ExitCode;

#[derive(serde::Deserialize)]
struct Request {
    board: Vec<String>,
    combos: Vec<String>,
    opponent: Vec<String>,
    queries: Vec<Vec<f64>>,
}

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: solver-equity <request.json>");
        return ExitCode::from(10);
    };
    match answer(&path) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(10)
        }
    }
}

fn answer(path: &str) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let req: Request = serde_json::from_str(&text).map_err(|e| format!("bad equity request json: {e}"))?;
    if !(3..=5).contains(&req.board.len()) {
        return Err("board must hold 3, 4 or 5 cards".into());
    }
    if req.queries.iter().any(|q| q.len() != req.opponent.len()) {
        return Err("every query must hold one weight per opponent combo".into());
    }
    let card = |s: &str| card_from_str(s).map_err(|e| format!("bad card {s}: {e}"));
    let combo = |s: &String| -> Result<(Card, Card), String> {
        if s.len() != 4 {
            return Err(format!("bad combo {s}"));
        }
        Ok((card(&s[0..2])?, card(&s[2..4])?))
    };
    let board = req.board.iter().map(|s| card(s)).collect::<Result<Vec<_>, _>>()?;
    let combos = req.combos.iter().map(combo).collect::<Result<Vec<_>, _>>()?;
    let opponent = req.opponent.iter().map(combo).collect::<Result<Vec<_>, _>>()?;
    let equities = range_equities(&board, &combos, &opponent, &req.queries);
    serde_json::to_string(&serde_json::json!({ "equities": equities })).map_err(|e| e.to_string())
}

fn range_equities(
    board: &[Card],
    combos: &[(Card, Card)],
    opponent: &[(Card, Card)],
    queries: &[Vec<f64>],
) -> Vec<Vec<Option<f64>>> {
    let runouts = runouts(board);
    let mut num = vec![vec![0.0f64; combos.len()]; queries.len()];
    let mut den = vec![vec![0.0f64; combos.len()]; queries.len()];

    let by_pair: Vec<Vec<f64>> = queries
        .iter()
        .map(|w| {
            let mut t = vec![0.0f64; 52 * 52];
            for (&(a, b), &x) in opponent.iter().zip(w) {
                t[pair(a, b)] += x;
            }
            t
        })
        .collect();

    for runout in &runouts {
        let mut full: Vec<Card> = board.to_vec();
        full.extend_from_slice(runout);
        let dead = |c: &(Card, Card)| full.contains(&c.0) || full.contains(&c.1);
        let ours: Vec<(usize, u16)> = combos
            .iter()
            .enumerate()
            .filter(|(_, c)| !dead(c))
            .map(|(i, &c)| (i, rank_of(c, &full)))
            .collect();
        let theirs: Vec<(usize, u16)> = opponent
            .iter()
            .enumerate()
            .filter(|(_, c)| !dead(c))
            .map(|(i, &c)| (i, rank_of(c, &full)))
            .collect();

        let mut ours_sorted = ours.clone();
        ours_sorted.sort_by_key(|&(_, r)| r);
        let mut theirs_sorted = theirs.clone();
        theirs_sorted.sort_by_key(|&(_, r)| r);

        for (q, weights) in queries.iter().enumerate() {
            let mut total = 0.0f64;
            let mut card_total = [0.0f64; 52];
            for &(j, _) in &theirs_sorted {
                let w = weights[j];
                if w <= 0.0 {
                    continue;
                }
                let (a, b) = opponent[j];
                total += w;
                card_total[a as usize] += w;
                card_total[b as usize] += w;
            }
            let (mut lt, mut le) = (0.0f64, 0.0f64);
            let (mut lt_card, mut le_card) = ([0.0f64; 52], [0.0f64; 52]);
            let (mut i_lt, mut i_le) = (0usize, 0usize);
            for &(i, r) in &ours_sorted {
                while i_lt < theirs_sorted.len() && theirs_sorted[i_lt].1 < r {
                    let j = theirs_sorted[i_lt].0;
                    let w = weights[j].max(0.0);
                    let (a, b) = opponent[j];
                    lt += w;
                    lt_card[a as usize] += w;
                    lt_card[b as usize] += w;
                    i_lt += 1;
                }
                while i_le < theirs_sorted.len() && theirs_sorted[i_le].1 <= r {
                    let j = theirs_sorted[i_le].0;
                    let w = weights[j].max(0.0);
                    let (a, b) = opponent[j];
                    le += w;
                    le_card[a as usize] += w;
                    le_card[b as usize] += w;
                    i_le += 1;
                }
                let (a, b) = combos[i];
                let same = by_pair[q][pair(a, b)].max(0.0);
                let compat = total - card_total[a as usize] - card_total[b as usize] + same;
                if compat <= 1e-12 {
                    continue;
                }
                let below = lt - lt_card[a as usize] - lt_card[b as usize];
                let at_or_below = le - le_card[a as usize] - le_card[b as usize] + same;
                num[q][i] += below + 0.5 * (at_or_below - below);
                den[q][i] += compat;
            }
        }
    }

    num.into_iter()
        .zip(den)
        .map(|(n, d)| n.into_iter().zip(d).map(|(n, d)| if d > 1e-12 { Some(n / d) } else { None }).collect())
        .collect()
}

fn pair(a: Card, b: Card) -> usize {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    lo as usize * 52 + hi as usize
}

fn rank_of(hole: (Card, Card), full: &[Card]) -> u16 {
    let mut hand = Hand::new();
    for &c in full {
        hand = hand.add_card(c as usize);
    }
    hand.add_card(hole.0 as usize).add_card(hole.1 as usize).evaluate()
}

fn runouts(board: &[Card]) -> Vec<Vec<Card>> {
    let live: Vec<Card> = (0..52u8).filter(|c| !board.contains(c)).collect();
    match board.len() {
        5 => vec![vec![]],
        4 => live.iter().map(|&c| vec![c]).collect(),
        3 => {
            let mut out = Vec::with_capacity(live.len() * (live.len() - 1) / 2);
            for (i, &a) in live.iter().enumerate() {
                for &b in &live[i + 1..] {
                    out.push(vec![a, b]);
                }
            }
            out
        }
        n => panic!("board must hold 3, 4 or 5 cards, got {n}"),
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use postflop_solver::card_from_str;

    fn c(s: &str) -> Card {
        card_from_str(s).unwrap()
    }
    fn combo(s: &str) -> (Card, Card) {
        (c(&s[0..2]), c(&s[2..4]))
    }
    fn board(s: &str) -> Vec<Card> {
        (0..s.len() / 2).map(|i| c(&s[2 * i..2 * i + 2])).collect()
    }

    #[test]
    fn river_overpair_beats_underpair() {
        let e = range_equities(&board("2c7d9hTsJc"), &[combo("AhAd")], &[combo("KhKd")], &[vec![1.0]]);
        assert_eq!(e, vec![vec![Some(1.0)]]);
    }

    #[test]
    fn a_tie_counts_half() {
        let e = range_equities(&board("9h8d7c6s5h"), &[combo("2c3d")], &[combo("2h3s")], &[vec![1.0]]);
        assert_eq!(e, vec![vec![Some(0.5)]]);
    }

    #[test]
    fn card_removal_drops_a_blocked_combo() {
        let e = range_equities(
            &board("Kd7c2s3h4d"),
            &[combo("AhKh")],
            &[combo("AhQh"), combo("QsQd")],
            &[vec![1.0, 1.0]],
        );
        assert_eq!(e, vec![vec![Some(1.0)]]);
    }

    #[test]
    fn all_weight_blocked_is_none() {
        let e = range_equities(&board("Kd7c2s3h4d"), &[combo("AhKh")], &[combo("AhQh")], &[vec![1.0]]);
        assert_eq!(e, vec![vec![None]]);
    }

    #[test]
    fn turn_is_the_mean_over_rivers_by_weight() {
        let turn = board("Kd7c2s3h");
        let ours = combo("QsQd");
        let theirs = [combo("AhKh"), combo("JcJh")];
        let w = vec![1.0, 2.0];
        let got = range_equities(&turn, &[ours], &theirs, &[w.clone()])[0][0].unwrap();
        let (mut num, mut den) = (0.0, 0.0);
        for r in 0..52u8 {
            if turn.contains(&r) || r == ours.0 || r == ours.1 {
                continue;
            }
            let mut full = turn.clone();
            full.push(r);
            for (t, &x) in theirs.iter().zip(&w) {
                if full.contains(&t.0) || full.contains(&t.1) {
                    continue;
                }
                let (a, b) = (rank_of(ours, &full), rank_of(*t, &full));
                num += x * if a > b { 1.0 } else if a == b { 0.5 } else { 0.0 };
                den += x;
            }
        }
        assert!((got - num / den).abs() < 1e-12, "{got} vs {}", num / den);
    }

    #[test]
    fn a_zero_weight_query_part_is_ignored_not_counted() {
        let e = range_equities(
            &board("2c7d9hTsJc"),
            &[combo("AhAd")],
            &[combo("KhKd"), combo("QhQd")],
            &[vec![0.0, 1.0], vec![1.0, 0.0]],
        );
        assert_eq!(e, vec![vec![Some(1.0)], vec![Some(1.0)]]);
    }
}
