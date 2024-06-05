use crate::bundler::{ArbBot, SignerProvider};
use crate::database::Database;
use crate::simulation::simulate_path_node;
use crate::trading_graph::{ArbHop, PriceGraph};
use crate::utils::{gwei_to_eth_f, wei_to_eth_f};
use arrayvec::ArrayVec;
use dashmap::DashMap;
use ethers::types::{H160, I256, U256};
use log::{debug, error, info, warn};
use petgraph::graph::NodeIndex;
use petgraph::prelude::*;
use rayon::prelude::*;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};
use tokio::runtime::{Handle, Runtime};

// this number is not really max depth
pub const MAX_DEPTH: usize = 5;

pub type PathT = ArrayVec<(NodeIndex, Option<EdgeIndex>), MAX_DEPTH>;

pub fn find_path(
    max_time: Duration,
    g: &PriceGraph,
    db: Database,
    source: NodeIndex,
    gas_price: f64,
    memo: Arc<DashMap<PathT, f64>>,
    bot: &ArbBot<SignerProvider>,
    path_sender: tokio::sync::mpsc::Sender<(PathT, f64)>,
    handle: Handle,
) -> u64 {
    // TODO: fix bad variable naming in this function

    let stop = Arc::new(AtomicBool::new(false));
    debug!("current gas price: {gas_price}");

    let stop_clone = stop.clone();
    thread::spawn(move || {
        sleep(max_time);
        stop_clone.store(true, Ordering::Relaxed);
        info!("Stopping search");
    });

    let edges: Vec<_> = {
        g.0.edges_directed(source, Direction::Outgoing)
            .map(|edge| (edge, stop.clone(), memo.clone(), path_sender.clone()))
            .collect()
    };

    info!("Searching to a depth of {}", MAX_DEPTH - 2);
    let paths_searched: u64 = edges
        .par_iter()
        .map(move |(edge, stop, memo, path_sender)| {
            let target = edge.target();
            let edge_weight = edge.weight();

            let mut path = ArrayVec::<_, MAX_DEPTH>::new();
            path.push((source, None));
            path.push((target, Some(edge.id())));
            search(
                g,
                &db,
                stop.clone(),
                memo.clone(),
                1.0 * edge_weight,
                target,
                source,
                &mut path,
                MAX_DEPTH as u8 - 2,
                gas_price,
                bot,
                path_sender.clone(),
                handle.clone(),
            )
        })
        .sum();

    paths_searched
}

fn search(
    g: &PriceGraph,
    db: &Database,
    stop: Arc<AtomicBool>,
    memo: Arc<DashMap<PathT, f64>>,
    weight: f64,
    node: NodeIndex,
    source: NodeIndex,
    path: &mut PathT,
    depth: u8,
    gas_price: f64,
    bot: &ArbBot<SignerProvider>,
    path_sender: tokio::sync::mpsc::Sender<(PathT, f64)>,
    handle: Handle,
) -> u64 {
    if stop.load(Ordering::Relaxed) {
        return 0;
    }

    let mut prev_edges = ArrayVec::<EdgeIndex, MAX_DEPTH>::new();
    for (_, edge) in &path[1..] {
        let edge = edge.unwrap();
        if prev_edges.contains(&edge) {
            return 0;
        }
        prev_edges.push(edge);
    }

    if node == source {
        if weight < 1.0 || weight > 1.5 {
            return 1;
        }

        match memo.get(path) {
            Some(_) => {
                return 1;
            }
            None => {
                let hops = (path.len() / 2).try_into().expect("Hops longer than 512");
                let score = handle.block_on(calculate_score(db, path, hops, gas_price, bot));
                memo.insert(path.clone(), score);
                if score > -0.0 {
                    info!("Path weight: {weight}");
                    path_sender.blocking_send((path.clone(), score)).unwrap();
                }

                return 1;
            }
        }
    }

    if depth == 0 {
        return 0;
    }

    let mut paths_searched = 0;

    for edge in g.0.edges(node) {
        let target = edge.target();
        let edge_weight = edge.weight();

        path.push((target, Some(edge.id())));
        paths_searched += search(
            g,
            db,
            stop.clone(),
            memo.clone(),
            weight * edge_weight,
            target,
            source,
            path,
            depth - 1,
            gas_price,
            bot,
            path_sender.clone(),
            handle.clone(),
        );
        path.pop();
    }

    paths_searched
}

pub fn calculate_gas(hops: u8) -> f64 {
    // TODO: get a more accurate gas limit estimation.
    hops as f64 * 100_000.0 + 100_000.0
}

pub async fn newton_optimizer(
    path: &PathT,
    db: &Database,
    first_guess: U256,
    max_iterations: usize,
    bias: f64,
    bot: &ArbBot<SignerProvider>,
) -> Option<(U256, U256)> {
    // Understanding this algo is left as an exercise for the reader.
    let mut amount_in = first_guess;
    let mut amount_out = U256::from(0);
    let delta = first_guess.as_u128() / 100;
    let val = U256::from(1);

    for _ in 0..max_iterations {
        let amount_in_prime = amount_in + delta;
        amount_out = simulate_path_node(path, amount_in, val, bot, db).await;
        let amount_out_prime = simulate_path_node(path, amount_in_prime, val, bot, db).await;
        let profit = amount_out.as_u128() as f64;
        if (profit - bias) < 1000000000.0 && (profit - bias) > -1000000000.0 {
            return Some((amount_in, amount_out));
        }
        let profit_prime = amount_out_prime.as_u128() as f64;
        // k = (y_1 - y_0) / (x_1 - x_0)
        let slope = (profit_prime - profit) / delta as f64;
        let new_in = amount_in.as_u128() as f64 - ((profit - bias) / slope);
        if new_in < 0.0 {
            return None;
        }
        amount_in = U256::from(new_in as i128);
    }

    Some((amount_in, amount_out))
}

async fn calculate_score(
    db: &Database,
    path: &PathT,
    hops: u8,
    gas_price: f64,
    bot: &ArbBot<SignerProvider>,
) -> f64 {
    let opt = bettermizer(db, path, bot).await;
    match opt {
        Some((_, o)) => {
            wei_to_eth_f(o.as_u128() as f64) - gwei_to_eth_f(calculate_gas(hops) * gas_price)
        }
        None => f64::MIN,
    }
}

pub async fn bettermizer(
    db: &Database,
    path: &PathT,
    bot: &ArbBot<SignerProvider>,
) -> Option<(U256, U256)> {
    let val = U256::from(128);

    let mut best_profit = U256::from(0);
    let mut best_amount_in = U256::from(0);
    let mut amount_in = U256::from(10).pow(U256::from(17));
    let mut profit = simulate_path_node(path, amount_in, val, bot, db).await;
    if profit <= U256::from(0) {
        return None;
    }

    for _ in 0..500 {
        profit = simulate_path_node(path, amount_in, val, bot, db).await;
        if profit >= best_profit {
            best_profit = profit;
            best_amount_in = amount_in;
            amount_in += U256::from(10).pow(U256::from(16));
        } else {
            break;
        }
    }

    Some((best_amount_in, best_profit))
}
