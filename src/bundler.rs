use anyhow::{anyhow, Ok, Result};
use ethers::prelude::*;
use ethers::types::{
    transaction::{eip2718::TypedTransaction, eip2930::AccessList},
    Address, Eip1559TransactionRequest, U256,
};
use ethers::{
    middleware::MiddlewareBuilder,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
};
use ethers_core::abi::encode_packed;
use ethers_flashbots::*;

use ethers_core::abi::Token;
use log::{debug, info};
use std::{str::FromStr, sync::Arc};
use url::Url;

use crate::constants::Env;
use crate::trading_graph::*;
use crate::utils::{gwei_to_eth_f, wei_to_eth_f};

abigen!(
    ArbBot,
    r#"[
        function recoverToken(address token) public payable;
        function recoverEth(uint256 amount) public payable;
        function makeArbHop(uint256 header, bool[] memory protocols, bytes[] memory v3data, address[] memory v3routers, address[] memory v2tokens, address[] memory v2routers) public returns (uint256 remainingBalance)
    ]"#,
);

#[derive(Debug, Clone)]

pub enum FlashLoan {
    Notused = 0,
    Balancer = 1,
    UniswapV2 = 2,
}

#[derive(Debug, PartialEq, Clone)]
pub enum V3Data {
    Token(Address),
    Fee(u32),
}

pub type SignerProvider = SignerMiddleware<Provider<Http>, LocalWallet>;

pub struct Bundler {
    pub nonce: U256,
    pub env: Env,
    pub sender: LocalWallet,
    pub bot: ArbBot<SignerProvider>,
    pub provider: SignerProvider,
    pub flashbots: SignerMiddleware<FlashbotsMiddleware<SignerProvider, LocalWallet>, LocalWallet>,
}

impl Bundler {
    pub async fn new<'a>() -> Self {
        let env = Env::new();

        let sender = env
            .private_key
            .parse::<LocalWallet>()
            .unwrap()
            .with_chain_id(env.chain_id.as_u64());

        let signer = env
            .signing_key
            .parse::<LocalWallet>()
            .unwrap()
            .with_chain_id(env.chain_id.as_u64());

        let provider = Provider::<Http>::try_from(&env.https_url)
            .unwrap()
            .with_signer(sender.clone());

        let flashbots = SignerMiddleware::new(
            FlashbotsMiddleware::new(
                provider.clone(),
                Url::parse("https://relay.flashbots.net").unwrap(),
                signer,
            ),
            sender.clone(),
        );

        // let current_nonce = provider.get_transaction_count(sender.address(), None).await?;
        // let nonce:U256 = U256::from(0);
        let current_nonce = provider
            .get_transaction_count(sender.address(), None)
            .await
            .map_err(anyhow::Error::new)
            .unwrap();

        let client = Arc::new(provider.clone());
        let bot = ArbBot::new(env.bot_address.parse::<Address>().unwrap(), client.clone());

        Ok(Self {
            nonce: current_nonce,
            env,
            sender,
            bot,
            provider,
            flashbots,
        })
        .unwrap()
    }
    pub async fn _common_fields(&mut self) -> Result<(H160, U256, U64)> {
        // Clone the current nonce for use in the transaction
        let old_nonce = self.nonce;

        // Increment the nonce for the next transaction
        self.nonce = self.nonce + U256::from(1);

        Ok((self.sender.address(), old_nonce, self.env.chain_id))
    }

    pub async fn sign_tx(&self, tx: Eip1559TransactionRequest) -> Result<Bytes> {
        let typed = TypedTransaction::Eip1559(tx);
        let signature = self.sender.sign_transaction(&typed).await?;
        let signed = typed.rlp_signed(&signature);
        Ok(signed)
    }

    pub fn to_bundle<T: Into<BundleTransaction>>(
        &self,
        signed_txs: Bytes,
        block_number: U64,
    ) -> BundleRequest {
        let mut bundle = BundleRequest::new();

        // for tx in signed_txs {
        //     let bundle_tx: BundleTransaction = tx.into();
        //     bundle = bundle.push_transaction(bundle_tx);
        // }
        let bundle_tx: BundleTransaction = signed_txs.into();
        bundle = bundle.push_transaction(bundle_tx);

        bundle
            .set_block(block_number + 1)
            .set_simulation_block(block_number)
            .set_simulation_timestamp(0)
    }

    pub async fn send_bundle(
        &self,
        bundle: BundleRequest,
        validator_payment_percentage: U256,
        gas_price: f64,
    ) -> Result<TxHash> {
        let simulated = self.flashbots.inner().simulate_bundle(&bundle).await?;

        for tx in &simulated.transactions {
            info!("coinbase tip: {:?}", tx.coinbase_tip);
            info!("gas used: {:?}", tx.gas_used);
            info!("to {:?}", tx.to);
            let val_percentage = validator_payment_percentage.as_u128() as f64 / 256.0;
            let expected_revenue = (wei_to_eth_f(tx.coinbase_tip.as_u128() as f64)
                / val_percentage)
                * (1.0 - val_percentage);
            info!("expected revenue {expected_revenue}");

            if let Some(e) = &tx.error {
                return Err(anyhow!("Simulation error: {:?}", e));
            }
            if let Some(r) = &tx.revert {
                return Err(anyhow!("Simulation revert: {:?}", r));
            }

            let gas_f = gwei_to_eth_f(tx.gas_used.as_u128() as f64 * gas_price);
            info!("Calculated gas cost {gas_f}");
            info!("Bundle net {}", expected_revenue - gas_f);
            if expected_revenue <= gas_f {
                return Err(anyhow!("Trade not profitable"));
            }
        }

        // For development, you can early return here to skip sending the bundle.
        // return Err(anyhow!("Skipping sending bundle"));

        let pending_bundle = self.flashbots.inner().send_bundle(&bundle).await?;
        // let bundle_hash = pending_bundle.await?;
        let bundle_hash = match pending_bundle.await? {
            Some(hash) => hash,
            None => return Err(anyhow!("Bundle hash not found")),
        };

        Ok(bundle_hash)
    }

    pub async fn send_tx(&self, tx: Eip1559TransactionRequest) -> Result<TxHash> {
        let pending_tx = self.provider.send_transaction(tx, None).await?;
        let receipt = pending_tx.await?.ok_or_else(|| anyhow!("Tx dropped"))?;
        Ok(receipt.transaction_hash)
    }

    pub async fn transfer_in_tx(
        &mut self,
        amount_in: U256,
        max_priority_fee_per_gas: U256,
        max_fee_per_gas: U256,
    ) -> Result<Eip1559TransactionRequest> {
        let common = self._common_fields().await?;
        let to = NameOrAddress::Address(H160::from_str(&self.env.bot_address).unwrap());
        Ok(Eip1559TransactionRequest {
            to: Some(to),
            from: Some(common.0),
            data: Some(Bytes(bytes::Bytes::new())),
            value: Some(amount_in),
            chain_id: Some(common.2),
            max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
            max_fee_per_gas: Some(max_fee_per_gas),
            gas: Some(U256::from(50000)),
            nonce: Some(common.1),
            access_list: AccessList::default(),
        })
    }

    pub async fn transfer_out_tx(
        &mut self,
        token: &str,
        max_priority_fee_per_gas: U256,
        max_fee_per_gas: U256,
    ) -> Result<Eip1559TransactionRequest> {
        let token_address = Address::from_str(token).unwrap();
        let calldata = self.bot.encode("recoverToken", (token_address,))?;

        let common = self._common_fields().await?;
        let to = NameOrAddress::Address(H160::from_str(&self.env.bot_address).unwrap());
        Ok(Eip1559TransactionRequest {
            to: Some(to),
            from: Some(common.0),
            data: Some(calldata),
            value: Some(U256::zero()),
            chain_id: Some(common.2),
            max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
            max_fee_per_gas: Some(max_fee_per_gas),
            gas: Some(U256::from(50000)),
            nonce: Some(common.1),
            access_list: AccessList::default(),
        })
    }

    pub async fn approve_tx(
        &mut self,
        routers: Vec<&str>,
        tokens: Vec<&str>,
        force: bool,
        max_priority_fee_per_gas: U256,
        max_fee_per_gas: U256,
        max_gas: U256,
    ) -> Result<Vec<Eip1559TransactionRequest>> {
        let mut txs = Vec::new();
        for router in routers.iter() {
            let common = self._common_fields().await?;
            let router_address = Address::from_str(router).unwrap();
            let token_addresses: Vec<Address> = tokens
                .iter()
                .map(|token| Address::from_str(token).unwrap())
                .collect();
            let calldata = self
                .bot
                .encode("approveRouter", (router_address, token_addresses, force))?;

            txs.push(Eip1559TransactionRequest {
                to: Some(NameOrAddress::Address(
                    H160::from_str(&self.env.bot_address).unwrap(),
                )),
                from: Some(common.0),
                data: Some(calldata),
                value: Some(U256::zero()),
                chain_id: Some(common.2),
                max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
                max_fee_per_gas: Some(max_fee_per_gas),
                gas: Some(max_gas * U256::from(tokens.len())),
                nonce: Some(common.1),
                access_list: AccessList::default(),
            });
        }
        Ok(txs)
    }

    fn generate_v3_bytecode(&self, v3hops: Vec<V3Data>) -> Token {
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
    fn concat_v3_hops(&self, v3hops: Vec<V3Data>) -> Vec<V3Data> {
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

    fn split_v3_hops(&self, v3hops: Vec<V3Data>) -> Vec<Vec<V3Data>> {
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

    pub async fn order_tx(
        &mut self,
        paths: Vec<ArbHop>,
        amount_in: U256,
        flashloan: FlashLoan,
        validator_payment_percentage: U256,
        max_priority_fee_per_gas: U256,
        max_fee_per_gas: U256,
        max_gas: U256,
    ) -> Result<Eip1559TransactionRequest> {
        let header =
            (amount_in << 16) | (validator_payment_percentage << 8) | (U256::from(flashloan as u8));
        let mut bool_protocols = Vec::new();
        let mut bytes_v3data = Vec::new();
        let mut address_v3routers = Vec::new();
        let mut address_v2tokens = Vec::new();
        let mut address_v2routers = Vec::new();

        // TODO: this function more or less assumes that the v3 router will always be constant
        // if we were to add support for more v3 dexes, it would mean reworking this code.

        let mut v3hops = Vec::new();

        for (i, arbhop) in paths.iter().enumerate() {
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
            let v3hops = self.concat_v3_hops(v3hops);
            let v3split = self.split_v3_hops(v3hops);
            for split in v3split {
                let bytes = self.generate_v3_bytecode(split);
                bytes_v3data.push(bytes);
            }
        }

        let calldata = self
            .bot
            .encode(
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
            .expect("something fucked up!");

        info!("calldata to eip1559: {:?}", calldata);

        let common = self._common_fields().await?;
        let to = NameOrAddress::Address(H160::from_str(&self.env.bot_address).unwrap());
        Ok(Eip1559TransactionRequest {
            to: Some(to),
            from: Some(common.0),
            data: Some(calldata),
            value: Some(U256::zero()),
            chain_id: Some(common.2),
            max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
            max_fee_per_gas: Some(max_fee_per_gas),
            gas: Some(max_gas),
            nonce: Some(common.1),
            access_list: AccessList::default(),
        })
    }
}
