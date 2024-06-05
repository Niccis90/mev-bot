use anyhow::Result;
use ethers::{
    self,
    types::{U256},
};

use fern::colors::{Color, ColoredLevelConfig};
use ethers_core::types::U512;
use log::{info, LevelFilter};
use rand::Rng;


use crate::trading_graph::ArbHop;






// pub fn v3_to_v2(v3_price: U256) -> f64 {
//     let v3_price = U512::from(v3_price);
//     let scale = U512::from(2).pow(U512::from(64 * 3));
//     let num = v3_price.pow(U512::from(2)) / scale;
//     let dem = U512::from(2).pow(U512::from(192)) / scale;

//     // println!("{num1}");
//     // println!("{dem1}");

//     let price = num.as_u128() as f64 / dem.as_u128() as f64;

//     price
// }


// fn determine_exchange(factory_address: H160) -> String {
//     let exchange_mapping: HashMap<H160, &str> = HashMap::from([
//         (
//             H160::from_str("0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f").unwrap(),
//             "Uniswap",
//         ),
//         (
//             H160::from_str("0xC0AEe478e3658e2610c5F7A4A2E1777cE9e4f2Ac").unwrap(),
//             "Sushiswap",
//         ),
//         (
//             H160::from_str("0x1F98431c8aD98523631AE4a59f267346ea31F984").unwrap(),
//             "UniswapV3",
//         ),
//         // (
//         //     H160::from_str("0xbACEB8eC6b9355Dfc0269C18bac9d6E2Bdc29C4F").unwrap(),
//         //     "SushiswapV3",
//         // ),
//     ]);

//     exchange_mapping
//         .get(&factory_address)
//         .unwrap_or(&"Unknown")
//         .to_string()
// }



// let router_mapping: HashMap<String, H160> = HashMap::from([
//             (
//                 "Uniswap".to_string(),
//                 H160::from_str("0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D").unwrap(),
//             ),
//             (
//                 "Sushiswap".to_string(),
//                 H160::from_str("0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F").unwrap(),
//             ),
//             (
//                 "UniswapV3".to_string(),
//                 H160::from_str("0xE592427A0AEce92De3Edee1F18E0157C05861564").unwrap(),
//             ),
//             // Add mappings for other exchanges as needed
//         ]);




pub fn setup_logger() -> Result<()> {
    let colors = ColoredLevelConfig {
        trace: Color::Cyan,
        debug: Color::Magenta,
        info: Color::Green,
        warn: Color::Red,
        error: Color::BrightRed,
        ..ColoredLevelConfig::new()
    };

    fern::Dispatch::new()
        .format(move |out, message, record| {
            out.finish(format_args!(
                "{}[{}] {}",
                chrono::Local::now().format("[%H:%M:%S]"),
                colors.color(record.level()),
                message
            ))
        })
        .chain(std::io::stdout())
        .level(log::LevelFilter::Error)
        .level_for("rust", LevelFilter::Info)
        .apply()?;

    Ok(())
}

pub fn calculate_next_block_base_fee(
    gas_used: U256,
    gas_limit: U256,
    base_fee_per_gas: U256,
) -> U256 {
    let gas_used = gas_used;

    let mut target_gas_used = gas_limit / 2;
    target_gas_used = if target_gas_used == U256::zero() {
        U256::one()
    } else {
        target_gas_used
    };

    let new_base_fee = {
        if gas_used > target_gas_used {
            base_fee_per_gas
                + ((base_fee_per_gas * (gas_used - target_gas_used)) / target_gas_used)
                    / U256::from(8u64)
        } else {
            base_fee_per_gas
                - ((base_fee_per_gas * (target_gas_used - gas_used)) / target_gas_used)
                    / U256::from(8u64)
        }
    };

    let seed = rand::thread_rng().gen_range(0..9);
    new_base_fee + seed
}

pub fn pprint_arbhop(x: &Vec<ArbHop>, y: U256) {
    let url = r#"https://etherscan.io/address/"#;
    let mut router_vec = Vec::new();
    for hop in x{
        router_vec.push(hop.router_address);
    }
    info!("routers used in path: {:?}",router_vec);
    info!("Opt_amount in is: {:?}", wei_to_eth_f(y.as_u128() as f64));
    for (i, hop) in x.iter().enumerate() {
        let n_hop = i + 1;
        let token_in = hop.token_in;
        let token_out = hop.token_out;
        info!("hop: {n_hop} in: {token_in:?} out: {token_out:?} - {url}{token_out:?}");
    }
}

pub fn wei_to_eth_f(x: f64) -> f64 {
    x / 10.0f64.powi(18)
}

pub fn gwei_to_eth_f(x: f64) -> f64 {
    x / 10.0f64.powi(9)
}

pub fn wei_to_gwei_f(x: f64) -> f64 {
    x / 10.0f64.powi(18 - 9)
}




// #[cfg(test)]
// mod tests {
//     use super::*;
    

//     #[test]

//     fn test_v3_to_v2_price() {
//         let v3_price: U256 = U256::from(75876165408899207260948441655_u128); // 75876165408890000000 = min length with 64 * 2 scale

//         let v2_price = v3_to_v2(v3_price);

//         println!(
//             "v2 price for the token0: {:?} and token1 {:?}",
//             1.0 / v2_price,
//             v2_price
//         );
//     }
// }
