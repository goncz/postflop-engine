// SPDX-License-Identifier: AGPL-3.0-or-later

use postflop_solver::*;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outcome {
    pub exploitability: f32,
    pub mes_ev: [f32; 2],
    pub iterations: u32,
}

fn check(game: &PostFlopGame) -> (f32, [f32; 2]) {
    let mes_ev = compute_mes_ev(game);
    let exploitability = if !game.is_raked() {
        (mes_ev[0] + mes_ev[1]) * 0.5
    } else {
        let current_ev = compute_current_ev(game);
        ((mes_ev[0] - current_ev[0]) + (mes_ev[1] - current_ev[1])) * 0.5
    };
    (exploitability, mes_ev)
}

pub fn check_gap(ran: u32, exploitability: f32, target: f32) -> u32 {
    let far = if exploitability > 4.0 * target {
        50
    } else if exploitability > 2.0 * target {
        20
    } else {
        10
    };
    far.min(ran.max(10))
}

pub fn solve_reporting(
    game: &mut PostFlopGame,
    max_iterations: u32,
    target: f32,
    label: &str,
    report: bool,
) -> Outcome {
    let (mut exploitability, mut mes_ev) = check(game);
    if report {
        emit(label, 0, max_iterations, exploitability, target);
    }

    let mut next_check = check_gap(0, exploitability, target);
    let mut iterations = 0;
    while iterations < max_iterations && exploitability > target {
        solve_step(game, iterations);
        iterations += 1;
        if iterations == next_check || iterations == max_iterations {
            (exploitability, mes_ev) = check(game);
            next_check = iterations + check_gap(iterations, exploitability, target);
        }
        if report && (iterations % 10 == 0 || iterations == max_iterations) {
            emit(label, iterations, max_iterations, exploitability, target);
        }
    }

    finalize(game);
    Outcome {
        exploitability,
        mes_ev,
        iterations,
    }
}

fn emit(label: &str, iteration: u32, max: u32, exploitability: f32, target: f32) {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(
        err,
        "{{\"progress\":{{\"pass\":\"{label}\",\"iteration\":{iteration},\
         \"max_iterations\":{max},\"exploitability\":{exploitability:.6e},\
         \"target\":{target:.6e}}}}}"
    );
    let _ = err.flush();
}
