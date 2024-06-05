use core::panic;
use std::{fmt, future::Future, str::FromStr, sync::Arc};

use ethers::{
    abi::{encode_packed, Address, Token},
    contract::abigen,
    middleware::{MiddlewareBuilder, SignerMiddleware},
    providers::{Http, Provider, Ws},
    signers::{LocalWallet, Signer},
    types::{H160, U256},
};

use amms::amm::uniswap_v2::factory::UniswapV2Factory;
use amms::amm::uniswap_v3::factory::UniswapV3Factory;
use amms::amm::AutomatedMarketMaker;
use amms::amm::AMM;
use amms::amm::{factory::Factory, uniswap_v2::UniswapV2Pool, uniswap_v3::UniswapV3Pool};
use amms::sync;
use amms::sync::checkpoint::deconstruct_checkpoint;
use rayon::prelude::*;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use crate::{
    bundler::{ArbBot, SignerProvider, V3Data},
    database::{self, Database},
    search::PathT,
    trading_graph::ArbHop,
};

fn generate_v3_bytecode(v3hops: Vec<V3Data>) -> Token {
    let mut tokens = Vec::new();
    for data in v3hops {
        match data {
            V3Data::Token(t) => tokens.push(Token::Address(t.clone())),
            V3Data::Fee(f) => {
                // This is scuffed :\
                // ethers-rs plz fix

                let fee_bytes = f.to_be_bytes();
                assert_eq!(fee_bytes[0], 0u8);
                let fee_bytes = fee_bytes[1..].to_vec(); // first three bytes for u24
                tokens.push(Token::Bytes(fee_bytes));
            }
        }
    }
    let packed = encode_packed(&tokens).unwrap();
    Token::Bytes(packed)
}

// Turns: tok_a - fee - tok_b - tok_b - fee - tok_c
// Into:  tok_a - fee - tok_b - fee - tok_c
fn concat_v3_hops(v3hops: Vec<V3Data>) -> Vec<V3Data> {
    let mut result: Vec<V3Data> = Vec::new();
    result.push(
        v3hops
            .first()
            .expect("Length of v3 hops should be at least 1")
            .clone(),
    );

    for hop in &v3hops[1..] {
        if hop != result.last().unwrap() {
            result.push(hop.clone().clone())
        }
    }

    result
}

fn split_v3_hops(v3hops: Vec<V3Data>) -> Vec<Vec<V3Data>> {
    let mut result = Vec::new();
    let mut buf = Vec::new();

    for i in 1..v3hops.len() {
        let a = v3hops[i - 1].clone();
        let b = v3hops[i].clone();

        buf.push(a.clone());

        match (a, b) {
            (V3Data::Token(a0), V3Data::Token(a1)) => {
                assert_ne!(a0, a1);

                result.push(buf.clone());
                buf.clear();
            }
            _ => (),
        }
    }

    buf.push(v3hops.last().unwrap().clone());
    result.push(buf.clone());
    result
}

pub async fn simulate_path_node(
    path: &PathT,
    amount_in: U256,
    validator_payment_percentage: U256,
    bot: &ArbBot<SignerProvider>,
    db: &Database,
) -> U256 {
    let flashloan = 1;
    let header =
        (amount_in << 16) | (validator_payment_percentage << 8) | (U256::from(flashloan as u8));
    let mut bool_protocols = Vec::new();
    let mut bytes_v3data = Vec::new();
    let mut address_v3routers = Vec::new();
    let mut address_v2tokens = Vec::new();
    let mut address_v2routers = Vec::new();

    // TODO: this function more or less assumes that the v3 router will always be constant
    // if we were to add support for more v3 dexes, it would mean reworking this code.

    let mut arbhop_vec: Vec<ArbHop> = Vec::new();

    for i in path.windows(2) {
        if let [a, b] = i {
            let node_in = a.0;
            let node_out = b.0;
            let token_in = *db.tokens.get(&node_in).unwrap();
            let token_out = *db.tokens.get(&node_out).unwrap();

            let edge = b.1.unwrap();
            let pool = db.pools.get(&edge).expect("Pool not in pools");

            let pool_addr = pool.address();
            let router_address = *db.routers.get(&pool_addr).expect("Router not found");

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

    let mut v3hops = Vec::new();

    for (i, arbhop) in arbhop_vec.iter().enumerate() {
        if arbhop.fee.is_some() {
            // V3
            v3hops.extend(vec![
                V3Data::Token(arbhop.token_in),
                V3Data::Fee(arbhop.fee.unwrap()),
                V3Data::Token(arbhop.token_out),
            ]);
            if i == 0 {
                bool_protocols.push(true);
                address_v3routers.push(arbhop.router_address);
            } else if bool_protocols.last().unwrap() != &true {
                bool_protocols.push(true);
                address_v3routers.push(arbhop.router_address);
            }
        } else {
            // V2
            bool_protocols.push(false);
            address_v2routers.push(arbhop.router_address);
            address_v2tokens.push(arbhop.token_in);
            address_v2tokens.push(arbhop.token_out);
        }
    }

    if !v3hops.is_empty() {
        let v3hops = concat_v3_hops(v3hops);
        let v3split = split_v3_hops(v3hops);
        for split in v3split {
            let bytes = generate_v3_bytecode(split);
            bytes_v3data.push(bytes);
        }
    }

    let data = bot
        .method(
            "makeArbHop",
            (
                header,
                bool_protocols,
                bytes_v3data,
                address_v3routers,
                address_v2tokens,
                address_v2routers,
            ),
        )
        .expect("Abi encode failed :(")
        .call()
        .await
        .unwrap_or(U256::from(0));

    // let offset = U256::from(3) * U256::from(10).pow(U256::from(16));

    let offset = U256::from(0);

    data.saturating_sub(offset)
}
