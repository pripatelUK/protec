use std::env;

use reqwest::{header::HeaderMap, Client};
use serde::{Deserialize, Serialize};

/// Minimal transaction shape for simulation, modeled after the TS PopulatedTransaction
/// Hex fields should include 0x prefix where appropriate.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SimTx {
    #[serde(skip_serializing_if = "Option::is_none")] pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub gas: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub gas_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub max_fee_per_gas: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub max_priority_fee_per_gas: Option<String>,

    /// Not sent to Tenderly; used in the response when we set gas_limit = gas_used * 2
    #[serde(skip)] pub gas_limit: Option<u128>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationSummary {
    pub reverted: bool,
    pub gas_used: u128,
    #[serde(skip_serializing_if = "Option::is_none")] pub error_message: Option<String>,
    pub uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub assets_in: Option<Vec<AssetChange>>,  // optional wallet view
    #[serde(skip_serializing_if = "Option::is_none")] pub assets_out: Option<Vec<AssetChange>>, // optional wallet view
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationSummaryAndTx {
    pub summary: SimulationSummary,
    pub tx: SimTx,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetChange {
    #[serde(default)] pub from: Option<String>,
    #[serde(default)] pub to: Option<String>,
    #[serde(default)] pub amount: Option<String>,
    #[serde(default)] pub token_info: Option<TokenInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenInfo {
    #[serde(default)] pub r#type: String,
    #[serde(default)] pub name: String,
    #[serde(default)] pub symbol: String,
    #[serde(default)] pub decimals: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationResult {
    pub simulation: SimulationSection,
    pub transaction: TransactionSection,
    #[serde(default)] pub contracts: Vec<ContractInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationSection {
    pub id: String,
    pub status: bool,
    #[serde(default)] pub gas_used: Option<u128>,
    #[serde(default)] pub error_message: Option<String>,
    #[serde(default)] pub receipt: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionSection {
    #[serde(default)] pub from: String,
    #[serde(default)] pub to: String,
    #[serde(default)] pub gas_price: serde_json::Value,
    #[serde(default)] pub gas_used: Option<u128>,
    #[serde(default)] pub value: String,
    #[serde(default)] pub transaction_info: TransactionInfo,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransactionInfo {
    #[serde(default)] pub asset_changes: Option<Vec<AssetChange>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContractInfo {
    #[serde(default)] pub address: String,
    #[serde(default)] pub token_data: Option<TokenInfo>,
}

#[derive(Clone)]
pub struct TenderlySimulator {
    http: Client,
    base_url: String,
    headers: HeaderMap,
    last_simulation: Option<SimulationResult>,
}

#[derive(Clone, Debug, Serialize)]
struct SimulationPayload {
    network_id: String,
    #[serde(skip_serializing_if = "Option::is_none")] from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] gas: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")] gas_price: Option<String>,
    value: String,
    save: bool,
    save_if_fails: bool,
    simulation_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")] max_fee_per_gas: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] max_priority_fee_per_gas: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum TenderlyError {
    #[error("missing environment: {0}")] MissingEnv(&'static str),
    #[error("request failed: {0}")] Request(String),
    #[error("invalid response: {0}")] InvalidResponse(String),
}

impl TenderlySimulator {
    /// Create a new simulator using environment variables for configuration:
    /// - TENDERLY_ACCOUNT
    /// - TENDERLY_PROJECT_ID
    /// - TENDERLY_ACCESS_TOKEN
    /// - TENDERLY_TESTNET_UUID (only used if ENV=dev)
    /// - ENV ("dev" selects the virtual testnet path)
    pub fn new() -> Result<Self, TenderlyError> {
        let _ = dotenvy::dotenv();
        let account = env::var("TENDERLY_ACCOUNT").map_err(|_| TenderlyError::MissingEnv("TENDERLY_ACCOUNT"))?;
        let project = env::var("TENDERLY_PROJECT_ID").map_err(|_| TenderlyError::MissingEnv("TENDERLY_PROJECT_ID"))?;
        let access = env::var("TENDERLY_ACCESS_TOKEN").map_err(|_| TenderlyError::MissingEnv("TENDERLY_ACCESS_TOKEN"))?;
        let env_mode = env::var("ENV").unwrap_or_else(|_| "".to_string());
        let testnet_uuid = env::var("TENDERLY_TESTNET_UUID").unwrap_or_else(|_| String::new());

        let base_url = if env_mode == "dev" {
            if testnet_uuid.is_empty() {
                return Err(TenderlyError::MissingEnv("TENDERLY_TESTNET_UUID"));
            }
            format!(
                "https://api.tenderly.co/api/v1/account/{}/project/{}/testnet/{}",
                account, project, testnet_uuid
            )
        } else {
            format!(
                "https://api.tenderly.co/api/v1/account/{}/project/{}",
                account, project
            )
        };

        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("X-Access-Key", access.parse().unwrap());

        Ok(Self {
            http: Client::new(),
            base_url,
            headers,
            last_simulation: None,
        })
    }

    /// Simulate a transaction using Tenderly's REST API.
    /// Mirrors the TS simulateTransaction():
    /// - sets network_id to mainnet ("1") for the fork base
    /// - uses simulation_type = "quick" for speed
    /// - returns gas_used and a copy of the tx with gas_limit = gas_used * 2
    pub async fn simulate_transaction(
        &mut self,
        tx: &SimTx,
        wallet_address: Option<&str>,
    ) -> Result<SimulationSummaryAndTx, TenderlyError> {
        let payload = SimulationPayload {
            network_id: "1".to_string(),
            from: tx.from.clone(),
            to: tx.to.clone(),
            input: tx.data.clone(),
            gas: tx.gas,
            gas_price: tx.gas_price.clone(),
            value: tx.value.clone().unwrap_or_else(|| "0".to_string()),
            save: true,
            save_if_fails: true,
            simulation_type: "quick",
            max_fee_per_gas: tx.max_fee_per_gas.clone(),
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas.clone(),
        };

        let url = format!("{}/simulate", self.base_url);
        let res = self
            .http
            .post(url)
            .headers(self.headers.clone())
            .json(&payload)
            .send()
            .await
            .map_err(|e| TenderlyError::Request(e.to_string()))?;

        if !res.status().is_success() {
            return Err(TenderlyError::Request(format!(
                "HTTP {}",
                res.status()
            )));
        }

        // Decode carefully to surface server response on errors
        let text = res.text().await.map_err(|e| TenderlyError::InvalidResponse(e.to_string()))?;
        let result: SimulationResult = serde_json::from_str(&text)
            .map_err(|e| TenderlyError::InvalidResponse(format!("{} | body={}", e, text)))?;
        self.last_simulation = Some(result.clone());

        let summary = self.get_simulation_summary_from(&result, wallet_address);
        let mut tx_out = tx.clone();
        tx_out.gas_limit = Some(summary.gas_used.saturating_mul(2));

        Ok(SimulationSummaryAndTx { summary, tx: tx_out })
    }

    /// POST /simulations/{id}/share to make the last simulation public
    pub async fn make_simulation_public(&self) -> Result<(), TenderlyError> {
        let sim_id = self
            .last_simulation
            .as_ref()
            .map(|s| s.simulation.id.clone())
            .ok_or_else(|| TenderlyError::InvalidResponse("no last simulation".to_string()))?;
        let url = format!("{}/simulations/{}/share", self.base_url, sim_id);
        let res = self
            .http
            .post(url)
            .headers(self.headers.clone())
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| TenderlyError::Request(e.to_string()))?;
        if !res.status().is_success() {
            return Err(TenderlyError::Request(format!(
                "HTTP {}",
                res.status()
            )));
        }
        Ok(())
    }

    /// POST /simulations/{id} to load a simulation by id and set it as last_simulation
    pub async fn set_simulation(&mut self, simulation_id: &str) -> Result<SimulationResult, TenderlyError> {
        let url = format!("{}/simulations/{}", self.base_url, simulation_id);
        let res = self
            .http
            .post(url)
            .headers(self.headers.clone())
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| TenderlyError::Request(e.to_string()))?;
        if !res.status().is_success() {
            return Err(TenderlyError::Request(format!(
                "HTTP {}",
                res.status()
            )));
        }
        let sim: SimulationResult = res
            .json()
            .await
            .map_err(|e| TenderlyError::InvalidResponse(e.to_string()))?;
        self.last_simulation = Some(sim.clone());
        Ok(sim)
    }

    fn get_simulation_summary_from(
        &self,
        sim: &SimulationResult,
        wallet_address: Option<&str>,
    ) -> SimulationSummary {
        let reverted = !sim.simulation.status;
        let gas_used = sim
            .simulation
            .gas_used
            .or_else(|| sim.transaction.gas_used)
            .unwrap_or(0);
        let error_message = sim.simulation.error_message.clone();
        let uuid = sim.simulation.id.clone();

        let (assets_in, assets_out) = if let Some(addr) = wallet_address {
            let empty: Vec<AssetChange> = Vec::new();
            let changes_ref: &Vec<AssetChange> = sim
                .transaction
                .transaction_info
                .asset_changes
                .as_ref()
                .unwrap_or(&empty);
            let (ins, outs) = Self::wallet_changes(changes_ref, addr);
            (Some(ins), Some(outs))
        } else {
            (None, None)
        };

        SimulationSummary { reverted, gas_used, error_message, uuid, assets_in, assets_out }
    }

    fn wallet_changes(all: &Vec<AssetChange>, wallet_address: &str) -> (Vec<AssetChange>, Vec<AssetChange>) {
        let addr = wallet_address.to_ascii_lowercase();
        let mut assets_in = Vec::new();
        let mut assets_out = Vec::new();
        for ch in all.iter() {
            if let Some(t) = ch.to.as_ref() {
                if t.to_ascii_lowercase() == addr {
                    assets_in.push(ch.clone());
                }
            }
            if let Some(f) = ch.from.as_ref() {
                if f.to_ascii_lowercase() == addr {
                    assets_out.push(ch.clone());
                }
            }
        }
        (assets_in, assets_out)
    }
}


