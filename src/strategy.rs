use crate::bundler::*;
use crate::constants::{Env, GWEI};
use crate::database::Database;
use crate::search::{bettermizer, calculate_gas, find_path, PathT, MAX_DEPTH};
use crate::streams::Event;
use crate::trading_graph::*;
use crate::utils::{pprint_arbhop, wei_to_eth_f, wei_to_gwei_f};
use amms::amm::AutomatedMarketMaker;
use amms::amm::AMM;
use anyhow::anyhow;
use arrayvec::ArrayVec;
use core::panic;
use dashmap::DashMap;
use ethers::abi::Address;
use ethers::middleware::{MiddlewareBuilder, SignerMiddleware};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::Bytes;
use ethers::types::{H160, U256};
use ethers_flashbots::BundleTransaction;
use ethers_flashbots::{BundleRequest, FlashbotsMiddleware};
use ethers_providers::{Http, Middleware, Provider};
use hashbrown::{HashMap, HashSet};
use log::info;
use log::{error, warn};
use petgraph::graph::NodeIndex;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc::Receiver;
use tokio::sync::RwLock;
use tokio::sync::RwLockReadGuard;
use tokio::time::Instant;
use url::Url;

async fn init_bot() -> ArbBot<SignerProvider> {
    let env = Env::new();

    let sender = env
        .private_key
        .parse::<LocalWallet>()
        .unwrap()
        .with_chain_id(env.chain_id.as_u64());

    let provider = Provider::<Http>::try_from(&env.https_url)
        .unwrap()
        .with_signer(sender.clone());

    let client = Arc::new(provider.clone());
    ArbBot::new(env.bot_address.parse::<Address>().unwrap(), client.clone())
}

// Hardest things in computer science...
fn cache_invalidation(
    db: RwLockReadGuard<Database>,
    memo: Arc<DashMap<PathT, f64>>,
    changed_pools: Vec<H160>,
) -> usize {
    let pool_set: HashSet<H160> = HashSet::from_iter(changed_pools);
    let mut remove = Vec::new();
    for (key, _) in <DashMap<
        ArrayVec<(NodeIndex, Option<petgraph::prelude::EdgeIndex>), MAX_DEPTH>,
        f64,
    > as Clone>::clone(&memo)
    .into_iter()
    {
        for (_, edge) in &key {
            if let Some(edge) = edge {
                let addr = db.pools.get(edge).unwrap().address();
                if pool_set.contains(&addr) {
                    remove.push(key.clone());
                }
            }
        }
    }

    for key in &remove {
        memo.remove(key);
    }

    remove.len()
}

pub async fn cycle_search(
    database: Arc<RwLock<Database>>,
    shared_graph: Arc<RwLock<PriceGraph>>,
    path_sender: tokio::sync::mpsc::Sender<(PathT, f64)>,
    gas_price: &AtomicU64,
    eth_index: NodeIndex,
    mut state_update_reciever: Receiver<Vec<H160>>,
    handle: Handle,
) {
    info!("Starting cycle search...");
    let memo = Arc::new(DashMap::new());
    let bot = init_bot().await;
    let max_time = Duration::from_millis(10000);

    while let Some(changed_pools) = state_update_reciever.recv().await {
        info!("Recived changed pools");
        let path_finding_time = Instant::now();

        let gas_price_f = gas_price.load(Ordering::Relaxed) as f64;
        let gas_price_f = wei_to_gwei_f(gas_price_f);

        let price_graph = shared_graph.read().await.clone();

        let db_clone = database.read().await.clone();

        let removed = cache_invalidation(database.read().await, memo.clone(), changed_pools);
        info!("Removed {removed} keys from memo");

        info!("changed pools added to cache invalidation");

        let paths_searched = find_path(
            max_time,
            &price_graph,
            db_clone,
            eth_index,
            gas_price_f,
            memo.clone(),
            &bot,
            path_sender.clone(),
            handle.clone(),
        );

        let k_paths_per_sec = paths_searched / path_finding_time.elapsed().as_millis() as u64;
        info!(
            "Searched {paths_searched} paths in {:?}. {k_paths_per_sec}k paths per second.",
            path_finding_time.elapsed()
        );

        if path_finding_time.elapsed().as_millis() - max_time.as_millis() > 500 {
            warn!("Path finding took a lot longer than max allowed time");
        }
    }
}

pub async fn path_processing(
    database: Arc<RwLock<Database>>,
    mut path_receiver: tokio::sync::mpsc::Receiver<(PathT, f64)>,
    bundle_sender: tokio::sync::mpsc::Sender<(Vec<ArbHop>, U256, f64)>,
) {
    info!(" path_processing started: waiting for paths..");

    while let Some((path, score)) = path_receiver.recv().await {
        let db_guard = database.read().await;
        let mut arbhop_vec: Vec<ArbHop> = Vec::new();

        for i in path.windows(2) {
            if let [a, b] = i {
                let node_in = a.0;
                let node_out = b.0;
                let token_in = *db_guard.tokens.get(&node_in).unwrap();
                let token_out = *db_guard.tokens.get(&node_out).unwrap();

                let edge = b.1.unwrap();
                let pool = db_guard.pools.get(&edge).expect("Pool not in pools");

                let pool_addr = pool.address();
                let router_address = *db_guard.routers.get(&pool_addr).expect("Router not found");

                let fee = match pool {
                    AMM::UniswapV3Pool(pool) => Some(pool.fee),
                    _ => None,
                };

                arbhop_vec.push(ArbHop {
                    router_address,
                    token_in,
                    token_out,
                    fee,
                });
            }
        }

        let bot = init_bot().await;
        let (amount_in, _) = bettermizer(&db_guard.clone(), &path, &bot).await.expect(
            "Paths should not fail to optimize here, since they have already passes in search",
        );
        if let Err(e) = bundle_sender.send((arbhop_vec, amount_in, score)).await {
            error!("Failed to send arbhop {:?}", e);
        }
    }
}

pub async fn trade_sender(
    event_sender: Sender<Event>,
    mut bundle_receiver: tokio::sync::mpsc::Receiver<(Vec<ArbHop>, U256, f64)>,
    gas_price: &AtomicU64,
) {
    info!(" Trade_sender started: looking for trades");
    let mut event_receiver = event_sender.subscribe();

    let mut current_block: ethers::types::U64 = 0.into();
    let mut base_fee = U256::from(0);

    let mut bundler = Bundler::new().await;

    loop {
        tokio::select! {
            Ok(event) = event_receiver.recv() => {
                match event {
                    Event::Block(block) => {
                        current_block = block.block_number;
                        base_fee = block.next_base_fee;
                        gas_price.store(base_fee.try_into().unwrap(), Ordering::Relaxed)
                    },
                    Event::PendingTx(_) => {
                        // not using pending tx
                    },
                    Event::Log(_) => {
                        // not using logs
                    },
                }
            },
            Some(arb_hop) = bundle_receiver.recv() => {
                info!("received arbhop");
                pprint_arbhop(&arb_hop.0,arb_hop.1);

                let arb_hop1 = arb_hop.0;

                let validator_percentage = U256::from(128);

                // TODO: use this for something.
                let hops = arb_hop1.len().try_into().expect("more than 512 hops not supported");
                let _gas_estimate_wei = U256::from(calculate_gas(hops) as i64) * base_fee;

                let opt_amount_in = arb_hop.1;

                let tx = bundler.order_tx(
                    arb_hop1,
                    opt_amount_in,
                    FlashLoan::Balancer,
                    validator_percentage,
                    U256::from(0) * *GWEI,
                    base_fee + U256::from(0) * *GWEI,
                    U256::from(1_000_000),
                ).await.unwrap();

                let signed_tx:Bytes = bundler.sign_tx(tx).await.unwrap();

                info!("Bundle score {}", arb_hop.2);

                let bundlereq: BundleRequest = bundler.to_bundle::<BundleTransaction>(signed_tx, current_block);

                let gas_price_f = gas_price.load(Ordering::Relaxed) as f64;
                let gas_price_f = wei_to_gwei_f(gas_price_f);

                match bundler.send_bundle(bundlereq, validator_percentage, gas_price_f).await {
                    Ok(bundle_hash) => {
                        info!("bundle sent: {:?}", bundle_hash);

                    }
                    Err(e) => {
                        error!("Error sending bundle {:?}",e);

                         bundler.nonce -= 1.into();


                    }
                }
            }
        }
    }
}
