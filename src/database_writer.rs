use crate::streams::Event;
use crate::{constants::Env, database::Database};
use amms::amm::{AutomatedMarketMaker, AMM};
use amms::state_space::StateSpaceManager;

use crate::trading_graph::PriceGraph;
use ethers::{
    self,
    providers::{Http, Provider, Ws},
    types::H160,
};
use hashbrown::{HashMap, HashSet};
use log::info;
use log::{debug, error};
use petgraph::stable_graph::NodeIndex;
use std::result::Result::Ok;
use std::{sync::Arc, time::Instant};
use tokio::sync::mpsc::Sender;
use tokio::sync::RwLock;

pub async fn initialize_db(
    database: Arc<RwLock<Database>>,
    provider: Arc<Provider<Ws>>,
    node: Arc<Provider<Ws>>,
    graph: &mut PriceGraph,
) -> (NodeIndex, Vec<AMM>, u64) {
    let setuptime = Instant::now();
    // Aquire lock for the duration of this entire function
    let mut db_guard = database.write().await;

    let (pools, block) = db_guard
        .fill_inital_pools(provider.clone(), node.clone())
        .await;
    let (eth_index, edge_to_node_map, node_to_token) = graph.initialize(pools.clone());

    db_guard.pools = edge_to_node_map; // fugly, because Eelis dislikes nice code
    db_guard.tokens = node_to_token;

    info!(
        "database setup complete in: {:?}",
        setuptime.elapsed().as_millis()
    );

    (eth_index.expect("no eth index"), pools, block)
}

pub async fn db_worker(
    database: Arc<RwLock<Database>>,
    event_sender: tokio::sync::broadcast::Sender<Event>,
    ws_provider: Arc<Provider<Ws>>,
    http_provider: Arc<Provider<Http>>,
    g: Arc<RwLock<PriceGraph>>,
    vec_of_pools: Vec<AMM>,
    state_update_sender: Sender<Vec<H160>>,
    block: u64,
) {
    info!("DB worker starting..");
    let mut event_receiver = event_sender.subscribe();
    let mut last_synced_block = 0;

    match event_receiver.recv().await {
        Ok(event) => match event {
            Event::Block(block) => {
                info!("{:?}", block);

                last_synced_block = block.block_number.as_u64() - 10;
            }
            _ => (),
        },
        Err(_) => {
            error!("failed to get new block event")
        }
    }

    database
        .write()
        .await
        .populate_data(None, last_synced_block, ws_provider.clone())
        .await;

    let state_space_manager = StateSpaceManager::new(
        vec_of_pools,
        last_synced_block,
        512,
        512,
        http_provider.clone(),
        ws_provider.clone(),
    );

    let (mut rx, _join_handles) = state_space_manager
        .subscribe_state_changes()
        .await
        .expect("state space manager not working");
    info!("DB worker active..");
    loop {
        match event_receiver.recv().await {
            Ok(event) => match event {
                Event::Block(block) => {
                    info!("{:?}", block);

                    last_synced_block = block.block_number.as_u64();
                }
                Event::PendingTx(_) => {
                    // not using pending tx
                }
                Event::Log(_) => {
                    // not using logs
                }
            },
            Err(_) => {
                error!("failed to get new block event")
            }
        }

        if let Some(state_changes) = rx.recv().await {
            // info!("{:?}", state_changes);

            let time = Instant::now();

            let mut locked_db = database.write().await;
            let address_mask = HashSet::from_iter(state_changes.clone());
            locked_db
                .populate_data(Some(address_mask), last_synced_block, ws_provider.clone())
                .await;

            let mut locked_g = g.write().await;

            for (edge_id, pool) in &locked_db.pools {
                let pool_address = pool.address();
                if !state_changes.contains(&pool_address) {
                    continue;
                }
                let (source, _) = locked_g.0.edge_endpoints(*edge_id).unwrap();
                let source = locked_db.tokens.get(&source);

                let price = pool
                    .calculate_price(*source.expect("NEIN NEIN NEIN"))
                    .expect("ror");

                *(locked_g
                    .0
                    .edge_weight_mut(*edge_id)
                    .expect("This edge exists by definition")) = price;
            }

            drop(locked_g);

            state_update_sender
                .send(state_changes.clone())
                .await
                .unwrap();

            info!(
                "tick, graph, price data update took: {:?} for block: {:?}",
                time.elapsed().as_millis(),
                last_synced_block
            );
        } else {
            error!("no updates received")
        }
    }
}
