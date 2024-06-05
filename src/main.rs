use anyhow::{Ok, Result};
use ethers::providers::{Http, Provider, Ws};
use log::info;
use main_flash::constants::Env;
use main_flash::database::Database;
use main_flash::database_writer::{db_worker, initialize_db};
use main_flash::strategy::*;
use main_flash::streams::{stream_new_blocks, Event};
use main_flash::trading_graph::PriceGraph;
use tokio::runtime::Handle;

use std::sync::Arc;
use tokio::sync::broadcast::{self, Sender};

use std::sync::atomic::AtomicU64;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();
    let env = Env::new();

    let graph_setup_time = Instant::now();

    let mut graph = PriceGraph::new();

    // Start async websocket streams
    let ws = Ws::connect(env.wss_url).await?;
    let ws_provider = Arc::new(Provider::new(ws));

    let http_provider = Arc::new(Provider::<Http>::try_from(env.https_url).unwrap());

    let inf_ws = Ws::connect(env.inf_wss_url).await?;
    let inf_provider = Arc::new(Provider::new(inf_ws));

    // Data management
    let database = Arc::new(RwLock::new(Database::empty()));
    let (eth_index, pools, last_block) = initialize_db(
        database.clone(),
        inf_provider.clone(),
        ws_provider.clone(),
        &mut graph,
    )
    .await;

    info!(
        "graph setup time: {:?}",
        graph_setup_time.elapsed().as_millis()
    );

    let (event_sender, _): (Sender<Event>, _) = broadcast::channel(512);
    // Channel from event_handler to graph_operations_task

    // Channel from graph_operations_task to bundle_sender (if needed)
    let (bundle_sender, bundle_receiver) = tokio::sync::mpsc::channel(512);

    let (path_sender, path_receiver) = tokio::sync::mpsc::channel(512);

    let (state_updates_sender, state_updates_reciever) = tokio::sync::mpsc::channel(512);

    let shared_graph: Arc<RwLock<PriceGraph>> = Arc::new(RwLock::new(graph));

    static GAS_PRICE: AtomicU64 = AtomicU64::new(0); // shared memory used for passing gas price

    let mut set = JoinSet::new();

    set.spawn(db_worker(
        database.clone(),
        event_sender.clone(),
        ws_provider.clone(),
        http_provider.clone(),
        shared_graph.clone(),
        pools,
        state_updates_sender.clone(),
        last_block,
    ));

    set.spawn(stream_new_blocks(ws_provider.clone(), event_sender.clone()));

    set.spawn(path_processing(
        database.clone(),
        path_receiver,
        bundle_sender,
    ));

    let rt: Handle = Handle::current();

    set.spawn(cycle_search(
        database.clone(),
        shared_graph.clone(),
        path_sender,
        &GAS_PRICE,
        eth_index,
        state_updates_reciever,
        rt.clone(),
    ));

    set.spawn(trade_sender(
        event_sender.clone(),
        bundle_receiver,
        &GAS_PRICE,
    ));

    while let Some(res) = set.join_next().await {
        info!("{:?}", res);
    }

    Ok(())
}
