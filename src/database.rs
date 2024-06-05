use amms::{
    amm::{
        factory::Factory,
        uniswap_v2::factory::UniswapV2Factory,
        uniswap_v3::{factory::UniswapV3Factory, UniswapV3Pool},
        AutomatedMarketMaker, AMM,
    },
    sync::{self, checkpoint::deconstruct_checkpoint},
};
use ethers::types::H160;
use ethers_providers::{Provider, Ws};
use hashbrown::{HashMap, HashSet};
use indicatif::ProgressBar;
use log::{debug, error, info, warn};
use petgraph::graph::{EdgeIndex, NodeIndex};
use rayon::iter::{IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator};
use std::{
    path::Path,
    str::FromStr,
    sync::Arc,
    thread::sleep,
    time::{Duration, Instant},
};
use tokio::task;

#[derive(Clone)]
pub struct Database {
    pub pools: HashMap<EdgeIndex, AMM>,
    pub tokens: HashMap<NodeIndex, H160>,
    pub routers: HashMap<H160, H160>,
}

impl Database {
    pub fn empty() -> Self {
        Database {
            pools: HashMap::new(),
            tokens: HashMap::new(),
            routers: HashMap::new(),
        }
    }

    pub async fn fill_inital_pools(
        &mut self,
        provider: Arc<Provider<Ws>>,
        node: Arc<Provider<Ws>>,
    ) -> (Vec<AMM>, u64) {
        let univ2router = H160::from_str("0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D").unwrap();
        let sushi2router = H160::from_str("0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F").unwrap();
        let univ3router = H160::from_str("0xE592427A0AEce92De3Edee1F18E0157C05861564").unwrap();

        let checkpoint_path_0 = Path::new("amm_checkpoint_0");
        let checkpoint_path_1 = Path::new("amm_checkpoint_1");
        let checkpoint_path_2 = Path::new("amm_checkpoint_2");

        if checkpoint_path_0.exists() && checkpoint_path_1.exists() && checkpoint_path_2.exists() {
            info!("Using checkpoint");
            let u_v2_pools = deconstruct_checkpoint(checkpoint_path_0.to_str().unwrap())
                .expect("Error restoring checkpoint");
            let s_v2_pools = deconstruct_checkpoint(checkpoint_path_1.to_str().unwrap())
                .expect("Error restoring checkpoint");
            let u_v3_pools = deconstruct_checkpoint(checkpoint_path_2.to_str().unwrap())
                .expect("Error restoring checkpoint");

            let block_number = u_v3_pools.1;

            let mut amm_vec = vec![];

            for pool in &u_v2_pools.0 {
                self.routers.insert(pool.address(), univ2router);
            }

            for pool in &s_v2_pools.0 {
                self.routers.insert(pool.address(), sushi2router);
            }

            for pool in &u_v3_pools.0 {
                self.routers.insert(pool.address(), univ3router);
            }

            amm_vec.extend(u_v2_pools.0);
            amm_vec.extend(s_v2_pools.0);
            amm_vec.extend(u_v3_pools.0);

            return (amm_vec, block_number);
        }
        let filltime = Instant::now();

        let uniswap_v3_factory = Factory::UniswapV3Factory(UniswapV3Factory::new(
            H160::from_str("0x1F98431c8aD98523631AE4a59f267346ea31F984").unwrap(),
            12369621_u64,
        ));
        let uniswap_v2_factory = Factory::UniswapV2Factory(UniswapV2Factory::new(
            H160::from_str("0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f").unwrap(),
            10000835u64,
            300_u32,
        ));
        let sushiswap_v2_factory = Factory::UniswapV2Factory(UniswapV2Factory::new(
            H160::from_str("0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac").unwrap(),
            10794229_u64,
            300_u32,
        ));

        let u_v2_pools = sync::sync_amms(
            vec![uniswap_v2_factory],
            provider.clone(),
            Some("amm_checkpoint_0"),
            500,
        )
        .await
        .expect("Error getting uniswap v2 pools");

        let s_v2_pools = sync::sync_amms(
            vec![sushiswap_v2_factory],
            provider.clone(),
            Some("amm_checkpoint_1"),
            500,
        )
        .await
        .expect("Error getting sushiswap v2 pools");

        let u_v3_pools = sync::sync_amms(
            vec![uniswap_v3_factory],
            provider.clone(),
            Some("amm_checkpoint_2"),
            500,
        )
        .await
        .expect("Error getting uniswap v3 pools");

        let block_number = u_v3_pools.1;

        for pool in &u_v2_pools.0 {
            self.routers.insert(pool.address(), univ2router);
        }

        for pool in &s_v2_pools.0 {
            self.routers.insert(pool.address(), sushi2router);
        }

        for pool in &u_v3_pools.0 {
            self.routers.insert(pool.address(), univ3router);
        }

        info!(
            "Filling initial pools took: {:?}",
            filltime.elapsed().as_millis()
        );

        let mut amm_vec = vec![];

        amm_vec.extend(u_v2_pools.0);
        amm_vec.extend(s_v2_pools.0);
        amm_vec.extend(u_v3_pools.0);

        (amm_vec, block_number)
    }

    pub async fn populate_data(
        &mut self,
        address_mask: Option<HashSet<H160>>,
        block: u64,
        provider: Arc<Provider<Ws>>,
    ) {
        let progress = ProgressBar::new(self.pools.len().try_into().unwrap());

        if address_mask.is_some() {
            info!("updating v2 and v3 pools to newest state");
        } else {
            info!("syncing v2 and v3 pools up to date");
        }

        let timer = Instant::now();

        tokio_scoped::scope(|scope| {
            for (_, amm) in &mut self.pools {
                if let Some(ref address_mask) = address_mask {
                    if !address_mask.contains(&amm.address()) {
                        continue;
                    }
                }
                match amm {
                    AMM::UniswapV3Pool(pool) => {
                        let provider_clone = provider.clone();
                        scope.spawn(async {
                            let res = pool.populate_tick_data(block, provider_clone).await;
                            pool.populate_data(Some(block), provider.clone()).await;
                            progress.inc(1);
                            match res {
                                Ok(_) => debug!("Populated tick data for pool"),
                                Err(e) => warn!("Falied to populate tick data {e:?}"),
                            }
                        });
                        // sleep(Duration::from_millis(10));
                    }
                    AMM::UniswapV2Pool(pool) => {
                        scope.spawn(async {
                            pool.populate_data(Some(block), provider.clone()).await;
                            progress.inc(1);
                        });
                    }
                    _ => panic!(),
                }
            }
        });

        progress.finish();

        info!("Done in {:?}", timer.elapsed());
    }
}
