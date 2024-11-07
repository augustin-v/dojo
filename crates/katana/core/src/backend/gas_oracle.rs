use katana_primitives::block::GasPrices;
use tokio::sync::watch;
use tokio::time::Duration;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_transport::Transport;
use alloy_rpc_types_eth::BlockNumberOrTag;
use std::collections::VecDeque;
use anyhow::Context;

const BUFFER_SIZE: usize = 60;
const INTERVAL: Duration = Duration::from_secs(60);
const ONE_GWEI: u128 = 1_000_000_000;

// TODO: implement a proper gas oracle function - sample the l1 gas and data gas prices
// currently this just return the hardcoded value set from the cli or if not set, the default value.
#[derive(Debug)]
pub struct L1GasOracle {
    gas_prices: GasPrices,
    data_gas_prices: GasPrices,
}

impl L1GasOracle {
    pub fn fixed(gas_prices: GasPrices, data_gas_prices: GasPrices) -> Self {
        Self { gas_prices, data_gas_prices }
    }

    /// Returns the current gas prices.
    pub fn current_gas_prices(&self) -> GasPrices {
        self.gas_prices.clone()
    }

    /// Returns the current data gas prices.
    pub fn current_data_gas_prices(&self) -> GasPrices {
        self.data_gas_prices.clone()
    }

    pub async fn gas_price_worker(
        &mut self,
        shutdown_rx: &mut watch::Receiver<()>,
    ) -> anyhow::Result<()> {
        let mut buffer = GasPriceBuffer::new();
        let rpc_url = "https://eth.merkle.io".parse()?;
        let provider = ProviderBuilder::new().on_http(rpc_url);
        // every 60 seconds, Starknet samples the base price of gas and data gas on L1
        let mut interval = tokio::time::interval(INTERVAL); 
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    
        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    break;
                }
                // tick every 60 seconds
                _ = interval.tick() => {
                    if let Err(e) = update_gas_price(self, provider.clone(), &mut buffer).await {
                        eprintln!("Error updating gas price: {}", e);
                    }
                }
            }
        }
    
        Ok(())
    }
}

async fn update_gas_price<P: Provider<T>, T: Transport + Clone>(
    l1_oracle: &mut L1GasOracle,
    provider: P,
    buffer: &mut GasPriceBuffer,
) -> anyhow::Result<()> {
    // Attempt to get the gas price from L1
    let last_block_number = provider.get_block_number().await?;
    let fee_history =
        provider.get_fee_history(1, BlockNumberOrTag::Number(last_block_number), &[]).await?;

    let latest_gas_price = fee_history.base_fee_per_gas.last().context("Getting eth gas price")?;
    buffer.add_sample(*latest_gas_price);

    let blob_fee_history = fee_history.base_fee_per_blob_gas;
    let avg_blob_base_fee = blob_fee_history.iter().last().context("Getting blob gas price")?;

    let avg_blob_fee_eth = *avg_blob_base_fee;
    let avg_blob_fee_strk = *avg_blob_base_fee + ONE_GWEI;

    let avg_gas_price = GasPrices {
        eth: buffer.average(),
        // The price of gas on Starknet is set to the average of the last 60 gas price samples, plus 1 gwei.
        strk: buffer.average() + ONE_GWEI,
    };
    let avg_blob_price = GasPrices { eth: avg_blob_fee_eth, strk: avg_blob_fee_strk };

    l1_oracle.gas_prices = avg_gas_price;
    l1_oracle.data_gas_prices = avg_blob_price;
    Ok(())
}
// Buffer to store the last 60 gas price samples
pub struct GasPriceBuffer {
    buffer: VecDeque<u128>,
}

impl GasPriceBuffer {
    fn new() -> Self {
        Self { buffer: VecDeque::with_capacity(BUFFER_SIZE) }
    }

    fn add_sample(&mut self, sample: u128) {
        if self.buffer.len() == BUFFER_SIZE {
            // remove oldest sample if buffer is full
            self.buffer.pop_front();
        }
        self.buffer.push_back(sample);
    }

    fn average(&self) -> u128 {
        if self.buffer.is_empty() {
            return 0;
        }
        let sum: u128 = self.buffer.iter().sum();
        sum / self.buffer.len() as u128
    }
}