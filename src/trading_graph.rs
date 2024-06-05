use amms::amm::AutomatedMarketMaker;
use amms::amm::AMM;

use ethers::types::H160;
use hashbrown::{HashMap, HashSet};

use log::{debug, info, warn};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::prelude::*;
use std::f64::consts::E;
use std::fs;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct ArbHop {
    pub router_address: H160,
    pub token_in: H160,
    pub token_out: H160,
    pub fee: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PriceGraph(pub DiGraph<H160, f64>);

impl PriceGraph {
    pub fn new() -> Self {
        PriceGraph { 0: DiGraph::new() }
    }

    pub fn initialize(
        &mut self,
        pools: Vec<AMM>,
    ) -> (
        Option<NodeIndex>,
        HashMap<EdgeIndex, AMM>,
        HashMap<NodeIndex, H160>,
    ) {
        info!("Reading blacklist");
        let whitelist = match fs::read_to_string("whitelist.txt") {
            Ok(s) => {
                let mut set = HashSet::new();
                for line in s.lines() {
                    match H160::from_str(line) {
                        std::result::Result::Ok(a) => {
                            set.insert(a);
                        }
                        _ => (), // Skips line if it is not valid addresses
                    }
                }

                info!("Successfully read whitelist. Blocked {} tokens", set.len());
                set
            }
            Err(e) => {
                warn!("Unable to open whitelist: {e:?}");
                HashSet::new()
            }
        };

        let (eth_idx, edge_to_pool, node_to_token) = self.build_graph(pools.clone(), &whitelist);
        info!(
            "graph edges: {} graph: nodes: {}",
            self.0.edge_count(),
            self.0.node_count()
        );

        (eth_idx, edge_to_pool, node_to_token)
    }

    fn build_graph(
        &mut self,
        pools: Vec<AMM>,
        whitelist: &HashSet<H160>,
    ) -> (
        Option<NodeIndex>,
        HashMap<EdgeIndex, AMM>,
        HashMap<NodeIndex, H160>,
    ) {
        let eth_addr = H160::from_str("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap();
        let mut added_nodes = HashMap::new();
        let mut edge_to_amm = HashMap::new();
        let mut node_to_token = HashMap::new();

        for amm in pools {
            if !whitelist.contains(&amm.address()) {
                debug!("Pool not in whitelist");
                continue;
            }
            match amm {
                AMM::UniswapV2Pool(ref pool) => {
                    let reserve0 = pool.reserve_0;
                    let reserve1 = pool.reserve_1;

                    if reserve0 < 100 || reserve1 < 100 {
                        continue;
                    }
                    let price_of_1 = pool.calculate_price(pool.token_a);
                    let price_of_0 = pool.calculate_price(pool.token_b);

                    let price_of_1 = match price_of_1 {
                        Ok(p) => p,
                        Err(e) => {
                            debug!("price calcuation falied for {:?} {e:?}", pool.address());
                            continue;
                        }
                    };

                    let price_of_0 = match price_of_0 {
                        Ok(p) => p,
                        Err(e) => {
                            debug!("price calcuation falied for {:?} {e:?}", pool.address());
                            continue;
                        }
                    };

                    let token0_index = *added_nodes
                        .entry(pool.token_a)
                        .or_insert_with(|| self.0.add_node(pool.token_a));

                    let token1_index = *added_nodes
                        .entry(pool.token_b)
                        .or_insert_with(|| self.0.add_node(pool.token_b));

                    node_to_token.insert(token0_index, pool.token_a);
                    node_to_token.insert(token1_index, pool.token_b);

                    let weight0to1 = price_of_1;
                    let weight1to0 = price_of_0;

                    let edge_0to1 = self.0.add_edge(token0_index, token1_index, weight0to1);

                    let edge_1to0 = self.0.add_edge(token1_index, token0_index, weight1to0);
                    edge_to_amm.insert(edge_0to1, amm.clone());
                    edge_to_amm.insert(edge_1to0, amm.clone());
                }
                AMM::UniswapV3Pool(ref pool) => {
                    if pool.fee == 0
                        || pool.fee == 1
                        || pool.token_a_decimals == 0
                        || pool.token_a_decimals == 0
                        || pool.liquidity == 0
                    {
                        continue; // TODO: think about this later
                    }
                    let price_of_1 = pool.calculate_price(pool.token_a);
                    let price_of_0 = pool.calculate_price(pool.token_b);

                    let price_of_1 = match price_of_1 {
                        Ok(p) => p,
                        Err(e) => {
                            debug!("price calcuation falied for {:?} {e:?}", pool.address());
                            continue;
                        }
                    };

                    let price_of_0 = match price_of_0 {
                        Ok(p) => p,
                        Err(e) => {
                            debug!("price calcuation falied for {:?} {e:?}", pool.address());
                            continue;
                        }
                    };

                    let token0_index = *added_nodes
                        .entry(pool.token_a)
                        .or_insert_with(|| self.0.add_node(pool.token_a));

                    let token1_index = *added_nodes
                        .entry(pool.token_b)
                        .or_insert_with(|| self.0.add_node(pool.token_b));

                    node_to_token.insert(token0_index, pool.token_a);
                    node_to_token.insert(token1_index, pool.token_b);

                    let weight0to1 = price_of_1;
                    let weight1to0 = price_of_0;

                    let edge_0to1 = self.0.add_edge(token0_index, token1_index, weight0to1);

                    let edge_1to0 = self.0.add_edge(token1_index, token0_index, weight1to0);
                    edge_to_amm.insert(edge_0to1, amm.clone());
                    edge_to_amm.insert(edge_1to0, amm.clone());
                }
                _ => (),
            }
        }

        (
            Some(*added_nodes.get(&eth_addr).unwrap()),
            edge_to_amm,
            node_to_token,
        )
    }
}

pub fn log_price(price: f64) -> f64 {
    if price > 0.0 {
        -price.log(E)
    } else {
        f64::MAX
    }
}
