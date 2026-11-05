use crate::interface::*;
use crate::mutex_like::*;
use crate::sliceop::*;
use crate::utility::*;
use std::io::{self, Write};
use std::mem::MaybeUninit;

#[cfg(feature = "custom-alloc")]
use crate::alloc::*;

struct DiscountParams {
    alpha_t: f32,
    beta_t: f32,
    gamma_t: f32,
}

impl DiscountParams {
    pub fn new(current_iteration: u32) -> Self {
        // 0, 1, 4, 16, 64, 256, ...
        let nearest_lower_power_of_4 = match current_iteration {
            0 => 0,
            x => 1 << ((x.leading_zeros() ^ 31) & !1),
        };

        let t_alpha = (current_iteration as i32 - 1).max(0) as f64;
        let t_gamma = (current_iteration - nearest_lower_power_of_4) as f64;

        let pow_alpha = t_alpha * t_alpha.sqrt();
        let pow_gamma = (t_gamma / (t_gamma + 1.0)).powi(3);

        Self {
            alpha_t: (pow_alpha / (pow_alpha + 1.0)) as f32,
            beta_t: 0.5,
            gamma_t: pow_gamma as f32,
        }
    }
}

/// Performs Discounted CFR algorithm until the given number of iterations or exploitability is
/// satisfied.
///
/// This method returns the exploitability of the obtained strategy.
pub fn solve<T: Game>(
    game: &mut T,
    max_num_iterations: u32,
    target_exploitability: f32,
    print_progress: bool,
) -> f32 {
    if game.is_solved() {
        panic!("Game is already solved");
    }

    if !game.is_ready() {
        panic!("Game is not ready");
    }

    let mut root = game.root();
    let mut exploitability = compute_exploitability(game);

    if print_progress {
        print!("iteration: 0 / {max_num_iterations} ");
        print!("(exploitability = {exploitability:.4e})");
        io::stdout().flush().unwrap();
    }

    for t in 0..max_num_iterations {
        if exploitability <= target_exploitability {
            break;
        }

        let params = DiscountParams::new(t);

        // alternating updates
        for player in 0..2 {
            let mut result = Vec::with_capacity(game.num_private_hands(player));
            solve_recursive(
                result.spare_capacity_mut(),
                game,
                &mut root,
                player,
                game.initial_weights(player ^ 1),
                &params,
            );
        }

        if (t + 1) % 10 == 0 || t + 1 == max_num_iterations {
            exploitability = compute_exploitability(game);
        }

        if print_progress {
            print!("\riteration: {} / {} ", t + 1, max_num_iterations);
            print!("(exploitability = {exploitability:.4e})");
            io::stdout().flush().unwrap();
        }
    }

    if print_progress {
        println!();
        io::stdout().flush().unwrap();
    }

    finalize(game);

    exploitability
}

/// Proceeds Discounted CFR algorithm for one iteration.
#[inline]
pub fn solve_step<T: Game>(game: &T, current_iteration: u32) {
    if game.is_solved() {
        panic!("Game is already solved");
    }

    if !game.is_ready() {
        panic!("Game is not ready");
    }

    let mut root = game.root();
    let params = DiscountParams::new(current_iteration);

    // alternating updates
    for player in 0..2 {
        let mut result = Vec::with_capacity(game.num_private_hands(player));
        solve_recursive(
            result.spare_capacity_mut(),
            game,
            &mut root,
            player,
            game.initial_weights(player ^ 1),
            &params,
        );
    }
}

/// Recursively solves the counterfactual values.
fn solve_recursive<T: Game>(
    result: &mut [MaybeUninit<f32>],
    game: &T,
    node: &mut T::Node,
    player: usize,
    cfreach: &[f32],
    params: &DiscountParams,
) {
    // return the counterfactual values when the `node` is terminal
    if node.is_terminal() {
        game.evaluate(result, node, player, cfreach);
        return;
    }

    #[cfg(not(feature = "custom-alloc"))]
    if game.is_chance_free(node) {
        solve_planned(result, game, node, player, cfreach, &Cfr(params));
        return;
    }

    let num_actions = node.num_actions();
    let num_hands = result.len();

    // simply recurse when the number of actions is one
    if num_actions == 1 && !node.is_chance() {
        let child = &mut node.play(0);
        solve_recursive(result, game, child, player, cfreach, params);
        return;
    }

    // allocate memory for storing the counterfactual values
    #[cfg(feature = "custom-alloc")]
    let cfv_actions = MutexLike::new(Vec::with_capacity_in(num_actions * num_hands, StackAlloc));
    #[cfg(not(feature = "custom-alloc"))]
    let cfv_actions = MutexLike::new(Vec::with_capacity(num_actions * num_hands));

    // if the `node` is chance
    if node.is_chance() {
        // update the reach probabilities
        #[cfg(feature = "custom-alloc")]
        let mut cfreach_updated = Vec::with_capacity_in(cfreach.len(), StackAlloc);
        #[cfg(not(feature = "custom-alloc"))]
        let mut cfreach_updated = Vec::with_capacity(cfreach.len());
        mul_slice_scalar_uninit(
            cfreach_updated.spare_capacity_mut(),
            cfreach,
            1.0 / game.chance_factor(node) as f32,
        );
        unsafe { cfreach_updated.set_len(cfreach.len()) };

        // compute the counterfactual values of each action
        for_each_child(node, |action| {
            solve_recursive(
                row_mut(cfv_actions.lock().spare_capacity_mut(), action, num_hands),
                game,
                &mut node.play(action),
                player,
                &cfreach_updated,
                params,
            );
        });

        // use 64-bit floating point values
        #[cfg(feature = "custom-alloc")]
        let mut result_f64 = Vec::with_capacity_in(num_hands, StackAlloc);
        #[cfg(not(feature = "custom-alloc"))]
        let mut result_f64 = Vec::with_capacity(num_hands);

        // sum up the counterfactual values
        let mut cfv_actions = cfv_actions.lock();
        unsafe { cfv_actions.set_len(num_actions * num_hands) };
        sum_slices_f64_uninit(result_f64.spare_capacity_mut(), &cfv_actions);
        unsafe { result_f64.set_len(num_hands) };

        // get information about isomorphic chances
        let isomorphic_chances = game.isomorphic_chances(node);

        // process isomorphic chances
        for (i, &isomorphic_index) in isomorphic_chances.iter().enumerate() {
            let swap_list = &game.isomorphic_swap(node, i)[player];
            let tmp = row_mut(&mut cfv_actions, isomorphic_index as usize, num_hands);

            apply_swap(tmp, swap_list);

            result_f64.iter_mut().zip(&*tmp).for_each(|(r, &v)| {
                *r += v as f64;
            });

            apply_swap(tmp, swap_list);
        }

        result.iter_mut().zip(&result_f64).for_each(|(r, &v)| {
            r.write(v as f32);
        });
    }
    // if the current player is `player`
    else if node.player() == player {
        // compute the counterfactual values of each action
        for_each_child(node, |action| {
            solve_recursive(
                row_mut(cfv_actions.lock().spare_capacity_mut(), action, num_hands),
                game,
                &mut node.play(action),
                player,
                cfreach,
                params,
            );
        });

        let mut cfv_actions = cfv_actions.lock();
        unsafe { cfv_actions.set_len(num_actions * num_hands) };
        update_player_node(result, game, node, &mut cfv_actions, params);
    }
    // if the current player is not `player`
    else {
        let row_size = cfreach.len();

        let cfreach_actions = if cfreach.iter().all(|&x| x == 0.0) {
            zeros(num_actions * row_size)
        } else {
            reach_by_action(game, node, cfreach)
        };

        // compute the counterfactual values of each action
        for_each_child(node, |action| {
            solve_recursive(
                row_mut(cfv_actions.lock().spare_capacity_mut(), action, num_hands),
                game,
                &mut node.play(action),
                player,
                row(&cfreach_actions, action, row_size),
                params,
            );
        });

        // sum up the counterfactual values
        let mut cfv_actions = cfv_actions.lock();
        unsafe { cfv_actions.set_len(num_actions * num_hands) };
        sum_slices_uninit(result, &cfv_actions);
    }
}

#[inline]
fn update_player_node<T: Game>(
    result: &mut [MaybeUninit<f32>],
    game: &T,
    node: &mut T::Node,
    cfv_actions: &mut [f32],
    params: &DiscountParams,
) {
    let num_actions = node.num_actions();
    let num_hands = result.len();

    // compute the strategy by regret-maching algorithm
    let mut strategy = if game.is_compression_enabled() {
        regret_matching_compressed(node.regrets_compressed(), num_actions)
    } else {
        regret_matching(node.regrets(), num_actions)
    };

    // node-locking
    let locking = game.locking_strategy(node);
    apply_locking_strategy(&mut strategy, locking);

    // sum up the counterfactual values
    let result = fma_slices_uninit(result, &strategy, cfv_actions);

    if game.is_compression_enabled() {
        // update the cumulative strategy
        let scale = node.strategy_scale();
        let decoder = params.gamma_t * scale / u16::MAX as f32;
        let cum_strategy = node.strategy_compressed_mut();

        strategy.iter_mut().zip(&*cum_strategy).for_each(|(x, y)| {
            *x += (*y as f32) * decoder;
        });

        if !locking.is_empty() {
            strategy.iter_mut().zip(locking).for_each(|(d, s)| {
                if s.is_sign_positive() {
                    *d = 0.0;
                }
            })
        }

        let new_scale = encode_unsigned_slice(cum_strategy, &strategy);
        node.set_strategy_scale(new_scale);

        // update the cumulative regret
        let scale = node.regret_scale();
        let alpha_decoder = params.alpha_t * scale / i16::MAX as f32;
        let beta_decoder = params.beta_t * scale / i16::MAX as f32;
        let cum_regret = node.regrets_compressed_mut();

        cfv_actions.iter_mut().zip(&*cum_regret).for_each(|(x, y)| {
            *x += *y as f32 * if *y >= 0 { alpha_decoder } else { beta_decoder };
        });

        cfv_actions.chunks_exact_mut(num_hands).for_each(|row| {
            sub_slice(row, result);
        });

        if !locking.is_empty() {
            cfv_actions.iter_mut().zip(locking).for_each(|(d, s)| {
                if s.is_sign_positive() {
                    *d = 0.0;
                }
            })
        }

        let new_scale = encode_signed_slice(cum_regret, &cfv_actions);
        node.set_regret_scale(new_scale);
    } else {
        // update the cumulative strategy
        let gamma = params.gamma_t;
        let cum_strategy = node.strategy_mut();
        cum_strategy.iter_mut().zip(&strategy).for_each(|(x, y)| {
            *x = *x * gamma + *y;
        });

        // update the cumulative regret
        let (alpha, beta) = (params.alpha_t, params.beta_t);
        let cum_regret = node.regrets_mut();
        cum_regret.iter_mut().zip(&*cfv_actions).for_each(|(x, y)| {
            let coef = if x.is_sign_positive() { alpha } else { beta };
            *x = *x * coef + *y;
        });
        cum_regret.chunks_exact_mut(num_hands).for_each(|row| {
            sub_slice(row, result);
        });
    }
}

#[inline]
fn reach_by_action<T: Game>(game: &T, node: &T::Node, cfreach: &[f32]) -> ActionVec {
    let num_actions = node.num_actions();
    let row_size = cfreach.len();

    // compute the strategy by regret-matching algorithm
    let mut cfreach_actions = if game.is_compression_enabled() {
        regret_matching_compressed(node.regrets_compressed(), num_actions)
    } else {
        regret_matching(node.regrets(), num_actions)
    };

    // node-locking
    let locking = game.locking_strategy(node);
    apply_locking_strategy(&mut cfreach_actions, locking);

    // update the reach probabilities
    cfreach_actions.chunks_exact_mut(row_size).for_each(|row| {
        mul_slice(row, cfreach);
    });
    cfreach_actions
}

#[derive(Clone, Copy)]
enum Slot {
    Root,
    Row { values: usize, action: usize },
}

#[derive(Clone, Copy)]
enum Reach {
    Root,
    Zero,
    Row { reach: usize, action: usize },
}

struct Planned<N> {
    node: *mut N,
    slot: Slot,
    reach: Reach,
    values: Option<usize>,
}

struct Plan<N> {
    nodes: Vec<Planned<N>>,
    values_len: usize,
    reaches: Vec<ActionVec>,
    num_hands: usize,
    row_size: usize,
}

#[cfg(not(feature = "custom-alloc"))]
thread_local! {
    static ARENA: std::cell::RefCell<Vec<MaybeUninit<f32>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(not(feature = "custom-alloc"))]
pub(crate) trait PlannedWalk<T: Game> {
    fn reach_by_action(&self, game: &T, node: &T::Node, cfreach: &[f32]) -> Vec<f32>;

    ///
    unsafe fn finish(
        &self,
        result: &mut [MaybeUninit<f32>],
        game: &T,
        node: *mut T::Node,
        cfv_actions: &mut [f32],
    );
}

#[cfg(not(feature = "custom-alloc"))]
struct Cfr<'a>(&'a DiscountParams);

#[cfg(not(feature = "custom-alloc"))]
impl<T: Game> PlannedWalk<T> for Cfr<'_> {
    #[inline]
    fn reach_by_action(&self, game: &T, node: &T::Node, cfreach: &[f32]) -> Vec<f32> {
        reach_by_action(game, node, cfreach)
    }

    #[inline]
    unsafe fn finish(
        &self,
        result: &mut [MaybeUninit<f32>],
        game: &T,
        node: *mut T::Node,
        cfv_actions: &mut [f32],
    ) {
        update_player_node(result, game, &mut *node, cfv_actions, self.0);
    }
}

///
///
#[cfg(not(feature = "custom-alloc"))]
pub(crate) fn solve_planned<T: Game, W: PlannedWalk<T>>(
    result: &mut [MaybeUninit<f32>],
    game: &T,
    node: *mut T::Node,
    player: usize,
    cfreach: &[f32],
    walk: &W,
) {
    let num_hands = result.len();
    let row_size = cfreach.len();
    let mut plan = Plan {
        nodes: Vec::with_capacity(32),
        values_len: 0,
        reaches: Vec::with_capacity(8),
        num_hands,
        row_size,
    };
    plan_node(&mut plan, game, walk, node, player, Slot::Root, Reach::Root, cfreach);

    ARENA.with(|arena| {
        let mut arena = arena.borrow_mut();
        arena.clear();
        arena.reserve(plan.values_len);
        unsafe { arena.set_len(plan.values_len) };
        let base = arena.as_mut_ptr();
        let root = result.as_mut_ptr();
        let slot = |slot: Slot| -> &mut [MaybeUninit<f32>] {
            let ptr = match slot {
                Slot::Root => root,
                Slot::Row { values, action } => unsafe { base.add(values + action * num_hands) },
            };
            unsafe { std::slice::from_raw_parts_mut(ptr, num_hands) }
        };

        {
            let zero_row = if plan.nodes.iter().any(|p| matches!(p.reach, Reach::Zero)) {
                vec![0.0; row_size]
            } else {
                Vec::new()
            };
            let mut results = Vec::with_capacity(16);
            let mut nodes = Vec::with_capacity(16);
            let mut reach = Vec::with_capacity(16);
            for planned in plan.nodes.iter().filter(|p| p.values.is_none()) {
                results.push(slot(planned.slot));
                nodes.push(unsafe { &*planned.node });
                reach.push(match planned.reach {
                    Reach::Zero => zero_row.as_slice(),
                    Reach::Root => cfreach,
                    Reach::Row { reach: r, action } => {
                        &plan.reaches[r][action * row_size..(action + 1) * row_size]
                    }
                });
            }
            game.evaluate_many(&mut results, &nodes, player, &reach);
        }

        for planned in plan.nodes.iter().rev() {
            let Some(values) = planned.values else {
                continue;
            };
            let out = slot(planned.slot);
            let node = unsafe { &*planned.node };
            let values = unsafe {
                std::slice::from_raw_parts_mut(
                    base.add(values) as *mut f32,
                    node.num_actions() * num_hands,
                )
            };
            if node.player() == player {
                unsafe { walk.finish(out, game, planned.node, values) };
            } else {
                sum_slices_uninit(out, values);
            }
        }
    });
}

#[cfg(not(feature = "custom-alloc"))]
fn plan_node<T: Game, W: PlannedWalk<T>>(
    plan: &mut Plan<T::Node>,
    game: &T,
    walk: &W,
    ptr: *mut T::Node,
    player: usize,
    slot: Slot,
    reach: Reach,
    root_reach: &[f32],
) {
    let node = unsafe { &*ptr };
    if node.is_terminal() {
        plan.nodes.push(Planned {
            node: ptr,
            slot,
            reach,
            values: None,
        });
        return;
    }

    let num_actions = node.num_actions();
    if num_actions == 1 {
        let child: *mut T::Node = &mut *node.play(0);
        plan_node(plan, game, walk, child, player, slot, reach, root_reach);
        return;
    }

    let values = plan.values_len;
    plan.values_len += num_actions * plan.num_hands;
    plan.nodes.push(Planned {
        node: ptr,
        slot,
        reach,
        values: Some(values),
    });

    let split = if node.player() == player {
        None
    } else {
        let rs = plan.row_size;
        let by_action = {
            let parent = match reach {
                Reach::Zero => None,
                Reach::Root => Some(root_reach),
                Reach::Row { reach: r, action } => {
                    Some(&plan.reaches[r][action * rs..(action + 1) * rs])
                }
            };
            match parent {
                Some(parent) if !parent.iter().all(|&x| x == 0.0) => {
                    Some(walk.reach_by_action(game, node, parent))
                }
                _ => None,
            }
        };
        Some(by_action.map(|v| {
            plan.reaches.push(v);
            plan.reaches.len() - 1
        }))
    };

    for action in 0..num_actions {
        let child_reach = match split {
            None => reach,
            Some(None) => Reach::Zero,
            Some(Some(r)) => Reach::Row { reach: r, action },
        };
        let child_slot = Slot::Row { values, action };
        let child: *mut T::Node = &mut *node.play(action);
        plan_node(plan, game, walk, child, player, child_slot, child_reach, root_reach);
    }
}

#[cfg(feature = "custom-alloc")]
type ActionVec = Vec<f32, StackAlloc>;
#[cfg(not(feature = "custom-alloc"))]
type ActionVec = Vec<f32>;

#[cfg(feature = "custom-alloc")]
#[inline]
fn zeros(len: usize) -> ActionVec {
    let mut v = Vec::with_capacity_in(len, StackAlloc);
    v.resize(len, 0.0);
    v
}

#[cfg(not(feature = "custom-alloc"))]
#[inline]
fn zeros(len: usize) -> ActionVec {
    vec![0.0; len]
}

/// Computes the strategy by regret-matching algorithm.
#[cfg(feature = "custom-alloc")]
#[inline]
fn regret_matching(regret: &[f32], num_actions: usize) -> Vec<f32, StackAlloc> {
    let mut strategy = Vec::with_capacity_in(regret.len(), StackAlloc);
    let uninit = strategy.spare_capacity_mut();
    uninit.iter_mut().zip(regret).for_each(|(s, r)| {
        s.write(max(*r, 0.0));
    });
    unsafe { strategy.set_len(regret.len()) };

    let row_size = regret.len() / num_actions;
    let mut denom = Vec::with_capacity_in(row_size, StackAlloc);
    sum_slices_uninit(denom.spare_capacity_mut(), &strategy);
    unsafe { denom.set_len(row_size) };

    let default = 1.0 / num_actions as f32;
    strategy.chunks_exact_mut(row_size).for_each(|row| {
        div_slice(row, &denom, default);
    });

    strategy
}

#[cfg(not(feature = "custom-alloc"))]
thread_local! {
    static DENOM: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Computes the strategy by regret-matching algorithm.
///
#[cfg(not(feature = "custom-alloc"))]
#[inline]
fn regret_matching(regret: &[f32], num_actions: usize) -> Vec<f32> {
    let row_size = regret.len() / num_actions;
    let default = 1.0 / num_actions as f32;
    let mut strategy = Vec::with_capacity(regret.len());
    DENOM.with(|denom| {
        let mut denom = denom.borrow_mut();
        denom.clear();
        denom.extend(regret[..row_size].iter().map(|&r| max(r, 0.0)));
        for row in regret[row_size..].chunks_exact(row_size) {
            denom.iter_mut().zip(row).for_each(|(d, &r)| *d += max(r, 0.0));
        }
        let uninit = strategy.spare_capacity_mut();
        for (out, row) in uninit.chunks_exact_mut(row_size).zip(regret.chunks_exact(row_size)) {
            out.iter_mut().zip(row).zip(denom.iter()).for_each(|((s, &r), &d)| {
                s.write(if is_zero(d) { default } else { max(r, 0.0) / d });
            });
        }
    });
    unsafe { strategy.set_len(regret.len()) };
    strategy
}

/// Computes the strategy by regret-matching algorithm.
#[cfg(feature = "custom-alloc")]
#[inline]
fn regret_matching_compressed(regret: &[i16], num_actions: usize) -> Vec<f32, StackAlloc> {
    let mut strategy = Vec::with_capacity_in(regret.len(), StackAlloc);
    strategy.extend(regret.iter().map(|&r| r.max(0) as f32));

    let row_size = strategy.len() / num_actions;
    let mut denom = Vec::with_capacity_in(row_size, StackAlloc);
    sum_slices_uninit(denom.spare_capacity_mut(), &strategy);
    unsafe { denom.set_len(row_size) };

    let default = 1.0 / num_actions as f32;
    strategy.chunks_exact_mut(row_size).for_each(|row| {
        div_slice(row, &denom, default);
    });

    strategy
}

/// Computes the strategy by regret-matching algorithm.
#[cfg(not(feature = "custom-alloc"))]
#[inline]
fn regret_matching_compressed(regret: &[i16], num_actions: usize) -> Vec<f32> {
    let mut strategy = Vec::with_capacity(regret.len());
    strategy.extend(regret.iter().map(|&r| r.max(0) as f32));

    let row_size = strategy.len() / num_actions;
    let mut denom = Vec::with_capacity(row_size);
    sum_slices_uninit(denom.spare_capacity_mut(), &strategy);
    unsafe { denom.set_len(row_size) };

    let default = 1.0 / num_actions as f32;
    strategy.chunks_exact_mut(row_size).for_each(|row| {
        div_slice(row, &denom, default);
    });

    strategy
}
