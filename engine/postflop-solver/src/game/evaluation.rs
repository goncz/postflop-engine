use super::*;
use crate::interface::*;
use crate::sliceop::*;
use std::cell::RefCell;
use std::mem::MaybeUninit;

#[inline]
fn min(x: f64, y: f64) -> f64 {
    if x < y {
        x
    } else {
        y
    }
}

impl PostFlopGame {
    pub(super) fn evaluate_internal(
        &self,
        result: &mut [MaybeUninit<f32>],
        node: &PostFlopNode,
        player: usize,
        cfreach: &[f32],
    ) {
        let pot = (self.tree_config.starting_pot + 2 * node.amount) as f64;
        let half_pot = 0.5 * pot;
        let rake = min(pot * self.tree_config.rake_rate, self.tree_config.rake_cap);
        let amount_win = (half_pot - rake) / self.num_combinations;
        let amount_lose = -half_pot / self.num_combinations;

        let player_cards = &self.private_cards[player];
        let opponent_cards = &self.private_cards[player ^ 1];

        let mut cfreach_sum = 0.0;
        let mut cfreach_minus = [0.0; 52];

        result.iter_mut().for_each(|v| {
            v.write(0.0);
        });

        let result = unsafe { &mut *(result as *mut _ as *mut [f32]) };

        // someone folded
        if node.player & PLAYER_FOLD_FLAG == PLAYER_FOLD_FLAG {
            let folded_player = node.player & PLAYER_MASK;
            let payoff = if folded_player as usize != player {
                amount_win
            } else {
                amount_lose
            };

            let valid_indices = if node.river != NOT_DEALT {
                &self.valid_indices_river[card_pair_to_index(node.turn, node.river)]
            } else if node.turn != NOT_DEALT {
                &self.valid_indices_turn[node.turn as usize]
            } else {
                &self.valid_indices_flop
            };

            let opponent_indices = &valid_indices[player ^ 1];
            for &i in opponent_indices {
                unsafe {
                    let cfreach_i = *cfreach.get_unchecked(i as usize);
                    if cfreach_i != 0.0 {
                        let (c1, c2) = *opponent_cards.get_unchecked(i as usize);
                        let cfreach_i_f64 = cfreach_i as f64;
                        cfreach_sum += cfreach_i_f64;
                        *cfreach_minus.get_unchecked_mut(c1 as usize) += cfreach_i_f64;
                        *cfreach_minus.get_unchecked_mut(c2 as usize) += cfreach_i_f64;
                    }
                }
            }

            if cfreach_sum == 0.0 {
                return;
            }

            let player_indices = &valid_indices[player];
            let same_hand_index = &self.same_hand_index[player];
            for &i in player_indices {
                unsafe {
                    let (c1, c2) = *player_cards.get_unchecked(i as usize);
                    let same_i = *same_hand_index.get_unchecked(i as usize);
                    let cfreach_same = if same_i == u16::MAX {
                        0.0
                    } else {
                        *cfreach.get_unchecked(same_i as usize) as f64
                    };
                    // inclusion-exclusion principle
                    let cfreach = cfreach_sum + cfreach_same
                        - *cfreach_minus.get_unchecked(c1 as usize)
                        - *cfreach_minus.get_unchecked(c2 as usize);
                    *result.get_unchecked_mut(i as usize) = (payoff * cfreach) as f32;
                }
            }
        }
        // showdown (optimized for no rake; 2-pass)
        else if rake == 0.0 {
            let pair_index = card_pair_to_index(node.turn, node.river);
            let hand_strength = &self.hand_strength[pair_index];
            let player_strength = &hand_strength[player];
            let opponent_strength = &hand_strength[player ^ 1];

            let valid_player_strength = &player_strength[1..player_strength.len() - 1];
            let mut i = 1;

            for &StrengthItem { strength, index } in valid_player_strength {
                unsafe {
                    while opponent_strength.get_unchecked(i).strength < strength {
                        let opponent_index = opponent_strength.get_unchecked(i).index as usize;
                        let cfreach_i = *cfreach.get_unchecked(opponent_index);
                        if cfreach_i != 0.0 {
                            let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                            let cfreach_i_f64 = cfreach_i as f64;
                            cfreach_sum += cfreach_i_f64;
                            *cfreach_minus.get_unchecked_mut(c1 as usize) += cfreach_i_f64;
                            *cfreach_minus.get_unchecked_mut(c2 as usize) += cfreach_i_f64;
                        }
                        i += 1;
                    }
                    let (c1, c2) = *player_cards.get_unchecked(index as usize);
                    let cfreach = cfreach_sum
                        - cfreach_minus.get_unchecked(c1 as usize)
                        - cfreach_minus.get_unchecked(c2 as usize);
                    *result.get_unchecked_mut(index as usize) = (amount_win * cfreach) as f32;
                }
            }

            cfreach_sum = 0.0;
            cfreach_minus.fill(0.0);
            i = opponent_strength.len() - 2;

            for &StrengthItem { strength, index } in valid_player_strength.iter().rev() {
                unsafe {
                    while opponent_strength.get_unchecked(i).strength > strength {
                        let opponent_index = opponent_strength.get_unchecked(i).index as usize;
                        let cfreach_i = *cfreach.get_unchecked(opponent_index);
                        if cfreach_i != 0.0 {
                            let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                            let cfreach_i_f64 = cfreach_i as f64;
                            cfreach_sum += cfreach_i_f64;
                            *cfreach_minus.get_unchecked_mut(c1 as usize) += cfreach_i_f64;
                            *cfreach_minus.get_unchecked_mut(c2 as usize) += cfreach_i_f64;
                        }
                        i -= 1;
                    }
                    let (c1, c2) = *player_cards.get_unchecked(index as usize);
                    let cfreach = cfreach_sum
                        - cfreach_minus.get_unchecked(c1 as usize)
                        - cfreach_minus.get_unchecked(c2 as usize);
                    *result.get_unchecked_mut(index as usize) += (amount_lose * cfreach) as f32;
                }
            }
        }
        // showdown (raked; 3-pass)
        else {
            let amount_tie = -0.5 * rake / self.num_combinations;
            let same_hand_index = &self.same_hand_index[player];

            let pair_index = card_pair_to_index(node.turn, node.river);
            let hand_strength = &self.hand_strength[pair_index];
            let player_strength = &hand_strength[player];
            let opponent_strength = &hand_strength[player ^ 1];

            let valid_player_strength = &player_strength[1..player_strength.len() - 1];
            let valid_opponent_strength = &opponent_strength[1..opponent_strength.len() - 1];

            for &StrengthItem { index, .. } in valid_opponent_strength {
                unsafe {
                    let cfreach_i = *cfreach.get_unchecked(index as usize);
                    if cfreach_i != 0.0 {
                        let (c1, c2) = *opponent_cards.get_unchecked(index as usize);
                        let cfreach_i_f64 = cfreach_i as f64;
                        cfreach_sum += cfreach_i_f64;
                        *cfreach_minus.get_unchecked_mut(c1 as usize) += cfreach_i_f64;
                        *cfreach_minus.get_unchecked_mut(c2 as usize) += cfreach_i_f64;
                    }
                }
            }

            if cfreach_sum == 0.0 {
                return;
            }

            let mut cfreach_sum_win = 0.0;
            let mut cfreach_sum_tie = 0.0;
            let mut cfreach_minus_win = [0.0; 52];
            let mut cfreach_minus_tie = [0.0; 52];

            let mut i = 1;
            let mut j = 1;
            let mut prev_strength = 0; // strength is always > 0

            for &StrengthItem { strength, index } in valid_player_strength {
                unsafe {
                    if strength > prev_strength {
                        prev_strength = strength;

                        if i < j {
                            cfreach_sum_win = cfreach_sum_tie;
                            cfreach_minus_win = cfreach_minus_tie;
                            i = j;
                        }

                        while opponent_strength.get_unchecked(i).strength < strength {
                            let opponent_index = opponent_strength.get_unchecked(i).index as usize;
                            let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                            let cfreach_i = *cfreach.get_unchecked(opponent_index) as f64;
                            cfreach_sum_win += cfreach_i;
                            *cfreach_minus_win.get_unchecked_mut(c1 as usize) += cfreach_i;
                            *cfreach_minus_win.get_unchecked_mut(c2 as usize) += cfreach_i;
                            i += 1;
                        }

                        if j < i {
                            cfreach_sum_tie = cfreach_sum_win;
                            cfreach_minus_tie = cfreach_minus_win;
                            j = i;
                        }

                        while opponent_strength.get_unchecked(j).strength == strength {
                            let opponent_index = opponent_strength.get_unchecked(j).index as usize;
                            let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                            let cfreach_j = *cfreach.get_unchecked(opponent_index) as f64;
                            cfreach_sum_tie += cfreach_j;
                            *cfreach_minus_tie.get_unchecked_mut(c1 as usize) += cfreach_j;
                            *cfreach_minus_tie.get_unchecked_mut(c2 as usize) += cfreach_j;
                            j += 1;
                        }
                    }

                    let (c1, c2) = *player_cards.get_unchecked(index as usize);
                    let cfreach_total = cfreach_sum
                        - cfreach_minus.get_unchecked(c1 as usize)
                        - cfreach_minus.get_unchecked(c2 as usize);
                    let cfreach_win = cfreach_sum_win
                        - cfreach_minus_win.get_unchecked(c1 as usize)
                        - cfreach_minus_win.get_unchecked(c2 as usize);
                    let cfreach_tie = cfreach_sum_tie
                        - cfreach_minus_tie.get_unchecked(c1 as usize)
                        - cfreach_minus_tie.get_unchecked(c2 as usize);
                    let same_i = *same_hand_index.get_unchecked(index as usize);
                    let cfreach_same = if same_i == u16::MAX {
                        0.0
                    } else {
                        *cfreach.get_unchecked(same_i as usize) as f64
                    };

                    let cfvalue = amount_win * cfreach_win
                        + amount_tie * (cfreach_tie - cfreach_win + cfreach_same)
                        + amount_lose * (cfreach_total - cfreach_tie);
                    *result.get_unchecked_mut(index as usize) = cfvalue as f32;
                }
            }
        }
    }

    pub(super) fn evaluate_internal_bunching(
        &self,
        result: &mut [MaybeUninit<f32>],
        node: &PostFlopNode,
        player: usize,
        cfreach: &[f32],
    ) {
        let pot = (self.tree_config.starting_pot + 2 * node.amount) as f64;
        let half_pot = 0.5 * pot;
        let rake = min(pot * self.tree_config.rake_rate, self.tree_config.rake_cap);
        let amount_win = ((half_pot - rake) / self.bunching_num_combinations) as f32;
        let amount_lose = (-half_pot / self.bunching_num_combinations) as f32;
        let amount_tie = (-0.5 * rake / self.bunching_num_combinations) as f32;
        let opponent_len = self.private_cards[player ^ 1].len();

        // someone folded
        if node.player & PLAYER_FOLD_FLAG == PLAYER_FOLD_FLAG {
            let folded_player = node.player & PLAYER_MASK;
            let payoff = if folded_player as usize != player {
                amount_win
            } else {
                amount_lose
            };

            let indices = if node.river != NOT_DEALT {
                &self.bunching_num_river[player][card_pair_to_index(node.turn, node.river)]
            } else if node.turn != NOT_DEALT {
                &self.bunching_num_turn[player][node.turn as usize]
            } else {
                &self.bunching_num_flop[player]
            };

            result.iter_mut().zip(indices).for_each(|(r, &index)| {
                if index != 0 {
                    let slice = &self.bunching_arena[index..index + opponent_len];
                    r.write(payoff * inner_product(cfreach, slice));
                } else {
                    r.write(0.0);
                }
            });
        }
        // showdown
        else {
            let pair_index = card_pair_to_index(node.turn, node.river);
            let indices = &self.bunching_num_river[player][pair_index];
            let player_strength = &self.bunching_strength[pair_index][player];
            let opponent_strength = &self.bunching_strength[pair_index][player ^ 1];

            result
                .iter_mut()
                .zip(indices)
                .zip(player_strength)
                .for_each(|((r, &index), &strength)| {
                    if index != 0 {
                        r.write(inner_product_cond(
                            cfreach,
                            &self.bunching_arena[index..index + opponent_len],
                            opponent_strength,
                            strength,
                            amount_win,
                            amount_lose,
                            amount_tie,
                        ));
                    } else {
                        r.write(0.0);
                    }
                });
        }
    }
}

pub(crate) const LANES: usize = 4;

thread_local! {
    static SCRATCH: RefCell<(Vec<f32>, Vec<f32>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

struct Lanes<'a, const N: usize, const PACKED: bool> {
    reach: [&'a [f32]; N],
    packed: &'a [[f32; N]],
    result: [*mut f32; N],
}

impl<'a, const N: usize, const PACKED: bool> Lanes<'a, N, PACKED> {
    #[inline(always)]
    unsafe fn reach(&self, i: usize) -> [f64; N] {
        let mut x = [0.0; N];
        if PACKED {
            let packed = self.packed.get_unchecked(i);
            for k in 0..N {
                x[k] = packed[k] as f64;
            }
        } else {
            for k in 0..N {
                x[k] = *self.reach[k].get_unchecked(i) as f64;
            }
        }
        x
    }

    #[inline(always)]
    unsafe fn write(&self, i: usize, v: [f32; N]) {
        for k in 0..N {
            *self.result[k].add(i) = v[k];
        }
    }

    #[inline(always)]
    unsafe fn add(&self, i: usize, v: [f32; N]) {
        let mut old = [0.0; N];
        for k in 0..N {
            old[k] = *self.result[k].add(i);
        }
        for k in 0..N {
            *self.result[k].add(i) = old[k] + v[k];
        }
    }
}

#[inline(always)]
fn add_lanes<const N: usize>(acc: &mut [f64; N], x: &[f64; N]) {
    for k in 0..N {
        acc[k] += x[k];
    }
}

impl PostFlopGame {
    pub(super) fn evaluate_many_internal(
        &self,
        results: &mut [&mut [MaybeUninit<f32>]],
        nodes: &[&PostFlopNode],
        player: usize,
        cfreach: &[&[f32]],
    ) {
        let batch = self.bunching_num_dead_cards == 0 && !self.is_raked();
        let mut showdowns = [0; LANES];
        let mut folds = [0; LANES];
        let (mut num_showdowns, mut num_folds) = (0, 0);
        for i in 0..nodes.len() {
            if !batch || cfreach[i].iter().all(|&x| x == 0.0) {
                self.evaluate(results[i], nodes[i], player, cfreach[i]);
            } else if nodes[i].player & PLAYER_FOLD_FLAG == PLAYER_FOLD_FLAG {
                folds[num_folds] = i;
                num_folds += 1;
                if num_folds == LANES {
                    self.evaluate_lanes(results, nodes, player, cfreach, &folds, true);
                    num_folds = 0;
                }
            } else {
                showdowns[num_showdowns] = i;
                num_showdowns += 1;
                if num_showdowns == LANES {
                    self.evaluate_lanes(results, nodes, player, cfreach, &showdowns, false);
                    num_showdowns = 0;
                }
            }
        }
        let folds = &folds[..num_folds];
        let showdowns = &showdowns[..num_showdowns];
        self.evaluate_lanes(results, nodes, player, cfreach, folds, true);
        self.evaluate_lanes(results, nodes, player, cfreach, showdowns, false);
    }

    fn evaluate_lanes(
        &self,
        results: &mut [&mut [MaybeUninit<f32>]],
        nodes: &[&PostFlopNode],
        player: usize,
        cfreach: &[&[f32]],
        lanes: &[usize],
        fold: bool,
    ) {
        match (lanes.len(), fold) {
            (0, _) => {}
            (1, _) => {
                let i = lanes[0];
                self.evaluate_internal(results[i], nodes[i], player, cfreach[i]);
            }
            (2, true) => self.evaluate_fold_lanes::<2>(results, nodes, player, cfreach, lanes),
            (_, true) => self.evaluate_fold_lanes::<4>(results, nodes, player, cfreach, lanes),
            (2, false) => self.evaluate_showdown_lanes::<2>(results, nodes, player, cfreach, lanes),
            (_, false) => self.evaluate_showdown_lanes::<4>(results, nodes, player, cfreach, lanes),
        }
    }

    fn with_lanes<const N: usize, const PACKED: bool>(
        &self,
        results: &mut [&mut [MaybeUninit<f32>]],
        cfreach: &[&[f32]],
        lanes: &[usize],
        player: usize,
        f: impl FnOnce(&Lanes<N, PACKED>) -> [bool; N],
    ) {
        debug_assert!(lanes.len() <= N);
        let num_player = self.private_cards[player].len();
        let num_opponent = self.private_cards[player ^ 1].len();
        SCRATCH.with(|scratch| {
            let (spare, packed) = &mut *scratch.borrow_mut();
            spare.resize(num_player, 0.0);
            let packed: &[[f32; N]] = if PACKED {
                packed.clear();
                packed.resize(num_opponent * N, 0.0);
                let packed = unsafe {
                    let ptr = packed.as_mut_ptr() as *mut [f32; N];
                    std::slice::from_raw_parts_mut(ptr, num_opponent)
                };
                for (k, &i) in lanes.iter().enumerate() {
                    packed.iter_mut().zip(cfreach[i]).for_each(|(p, &x)| p[k] = x);
                }
                packed
            } else {
                &[]
            };
            let mut view = Lanes {
                reach: [cfreach[lanes[0]]; N],
                packed,
                result: [spare.as_mut_ptr(); N],
            };
            for (k, &i) in lanes.iter().enumerate() {
                results[i].iter_mut().for_each(|r| {
                    r.write(0.0);
                });
                view.reach[k] = cfreach[i];
                view.result[k] = results[i].as_mut_ptr() as *mut f32;
            }
            let zero = f(&view);
            for (k, &i) in lanes.iter().enumerate() {
                if zero[k] {
                    results[i].iter_mut().for_each(|r| {
                        r.write(0.0);
                    });
                }
            }
        });
    }

    #[inline]
    fn amounts(&self, node: &PostFlopNode) -> (f64, f64) {
        let pot = (self.tree_config.starting_pot + 2 * node.amount) as f64;
        let half_pot = 0.5 * pot;
        let rake = min(pot * self.tree_config.rake_rate, self.tree_config.rake_cap);
        debug_assert!(rake == 0.0);
        let amount_win = (half_pot - rake) / self.num_combinations;
        let amount_lose = -half_pot / self.num_combinations;
        (amount_win, amount_lose)
    }

    fn evaluate_fold_lanes<const N: usize>(
        &self,
        results: &mut [&mut [MaybeUninit<f32>]],
        nodes: &[&PostFlopNode],
        player: usize,
        cfreach: &[&[f32]],
        lanes: &[usize],
    ) {
        let mut payoff = [0.0; N];
        for (k, &i) in lanes.iter().enumerate() {
            let (amount_win, amount_lose) = self.amounts(nodes[i]);
            let folded_player = nodes[i].player & PLAYER_MASK;
            payoff[k] = if folded_player as usize != player {
                amount_win
            } else {
                amount_lose
            };
        }
        let first = nodes[lanes[0]];
        debug_assert!(lanes.iter().all(|&i| nodes[i].turn == first.turn
            && nodes[i].river == first.river));
        let valid_indices = &self.valid_indices_river[card_pair_to_index(first.turn, first.river)];
        let player_cards = &self.private_cards[player];
        let opponent_cards = &self.private_cards[player ^ 1];
        let same_hand_index = &self.same_hand_index[player];

        self.with_lanes::<N, false>(results, cfreach, lanes, player, |lane| {
            let mut cfreach_sum = [0.0; N];
            let mut cfreach_minus = [[0.0; N]; 52];
            for &i in &valid_indices[player ^ 1] {
                unsafe {
                    let x = lane.reach(i as usize);
                    let (c1, c2) = *opponent_cards.get_unchecked(i as usize);
                    add_lanes(&mut cfreach_sum, &x);
                    add_lanes(cfreach_minus.get_unchecked_mut(c1 as usize), &x);
                    add_lanes(cfreach_minus.get_unchecked_mut(c2 as usize), &x);
                }
            }

            for &i in &valid_indices[player] {
                unsafe {
                    let (c1, c2) = *player_cards.get_unchecked(i as usize);
                    let same_i = *same_hand_index.get_unchecked(i as usize);
                    let cfreach_same = if same_i == u16::MAX {
                        [0.0; N]
                    } else {
                        lane.reach(same_i as usize)
                    };
                    let m1 = cfreach_minus.get_unchecked(c1 as usize);
                    let m2 = cfreach_minus.get_unchecked(c2 as usize);
                    let mut v = [0.0; N];
                    for k in 0..N {
                        // inclusion-exclusion principle
                        let cfreach = cfreach_sum[k] + cfreach_same[k] - m1[k] - m2[k];
                        v[k] = (payoff[k] * cfreach) as f32;
                    }
                    lane.write(i as usize, v);
                }
            }

            cfreach_sum.map(|s| s == 0.0)
        });
    }

    fn evaluate_showdown_lanes<const N: usize>(
        &self,
        results: &mut [&mut [MaybeUninit<f32>]],
        nodes: &[&PostFlopNode],
        player: usize,
        cfreach: &[&[f32]],
        lanes: &[usize],
    ) {
        let mut amount_win = [0.0; N];
        let mut amount_lose = [0.0; N];
        for (k, &i) in lanes.iter().enumerate() {
            (amount_win[k], amount_lose[k]) = self.amounts(nodes[i]);
        }
        let first = nodes[lanes[0]];
        debug_assert!(lanes.iter().all(|&i| nodes[i].turn == first.turn
            && nodes[i].river == first.river));
        let hand_strength = &self.hand_strength[card_pair_to_index(first.turn, first.river)];
        let player_strength = &hand_strength[player];
        let opponent_strength = &hand_strength[player ^ 1];
        let valid_player_strength = &player_strength[1..player_strength.len() - 1];
        let player_cards = &self.private_cards[player];
        let opponent_cards = &self.private_cards[player ^ 1];

        self.with_lanes::<N, true>(results, cfreach, lanes, player, |lane| {
            let mut cfreach_sum = [0.0; N];
            let mut cfreach_minus = [[0.0; N]; 52];
            let mut i = 1;

            for &StrengthItem { strength, index } in valid_player_strength {
                unsafe {
                    while opponent_strength.get_unchecked(i).strength < strength {
                        let opponent_index = opponent_strength.get_unchecked(i).index as usize;
                        let x = lane.reach(opponent_index);
                        let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                        add_lanes(&mut cfreach_sum, &x);
                        add_lanes(cfreach_minus.get_unchecked_mut(c1 as usize), &x);
                        add_lanes(cfreach_minus.get_unchecked_mut(c2 as usize), &x);
                        i += 1;
                    }
                    let (c1, c2) = *player_cards.get_unchecked(index as usize);
                    let m1 = cfreach_minus.get_unchecked(c1 as usize);
                    let m2 = cfreach_minus.get_unchecked(c2 as usize);
                    let mut v = [0.0; N];
                    for k in 0..N {
                        let cfreach = cfreach_sum[k] - m1[k] - m2[k];
                        v[k] = (amount_win[k] * cfreach) as f32;
                    }
                    lane.write(index as usize, v);
                }
            }

            cfreach_sum = [0.0; N];
            cfreach_minus = [[0.0; N]; 52];
            i = opponent_strength.len() - 2;

            for &StrengthItem { strength, index } in valid_player_strength.iter().rev() {
                unsafe {
                    while opponent_strength.get_unchecked(i).strength > strength {
                        let opponent_index = opponent_strength.get_unchecked(i).index as usize;
                        let x = lane.reach(opponent_index);
                        let (c1, c2) = *opponent_cards.get_unchecked(opponent_index);
                        add_lanes(&mut cfreach_sum, &x);
                        add_lanes(cfreach_minus.get_unchecked_mut(c1 as usize), &x);
                        add_lanes(cfreach_minus.get_unchecked_mut(c2 as usize), &x);
                        i -= 1;
                    }
                    let (c1, c2) = *player_cards.get_unchecked(index as usize);
                    let m1 = cfreach_minus.get_unchecked(c1 as usize);
                    let m2 = cfreach_minus.get_unchecked(c2 as usize);
                    let mut v = [0.0; N];
                    for k in 0..N {
                        let cfreach = cfreach_sum[k] - m1[k] - m2[k];
                        v[k] = (amount_lose[k] * cfreach) as f32;
                    }
                    lane.add(index as usize, v);
                }
            }

            [false; N]
        });
    }
}
