use std::{env, fs, process::Command};

use rpc_interceptor::tenderly::{SimTx, TenderlySimulator};

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    // Gather inputs from env to keep this CLI simple and non-invasive.
    let from_address = match env::var("TEST_WALLET_ADDRESS").ok() {
        Some(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("Missing TEST_WALLET_ADDRESS in .env");
            std::process::exit(1);
        }
    };
    let to_address = env::var("TEST_TO").ok().or_else(|| env::var("TO").ok());
    let mut value_hex = env::var("VALUE_HEX").ok().or_else(|| env::var("VALUE").ok());
    let explicit_data_hex = env::var("DATA_HEX").ok();

    // Optional function encoding via Foundry `cast calldata` if provided.
    // FUNCTION_SIG example: "transfer(address,uint256)"
    // ARGS example: "0xabc... 1000000000000000000" or comma-separated
    let function_sig = env::var("FUNCTION_SIG").ok();
    let args_raw = env::var("ARGS").ok();

    // Require destination address.
    let to_address = match to_address {
        Some(v) if !v.is_empty() => v,
        _ => {
            eprintln!("Missing destination address. Set TEST_TO or TO.");
            std::process::exit(1);
        }
    };

    // Normalize value to 0x-prefixed hex (wei). Default to 0.
    let value_hex = match value_hex {
        Some(v) if v.starts_with("0x") || v.starts_with("0X") => v,
        Some(v) => match v.parse::<u128>() {
            Ok(n) => format!("0x{:x}", n),
            Err(_) => {
                eprintln!("Invalid VALUE; provide decimal or 0x-hex");
                std::process::exit(1);
            }
        },
        None => "0x0".to_string(),
    };

    // Compute data: prefer explicit DATA_HEX, else use cast calldata if FUNCTION_SIG provided, else 0x.
    let data_hex = if let Some(d) = explicit_data_hex {
        d
    } else if let Some(sig) = function_sig {
        let cast_bin = env::var("CAST_BIN").unwrap_or_else(|_| "cast".to_string());
        let args_list: Vec<String> = match args_raw {
            Some(a) => {
                if a.contains(',') { a.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() }
                else { a.split_whitespace().map(|s| s.to_string()).collect() }
            }
            None => Vec::new(),
        };
        let mut cmd = Command::new(cast_bin);
        cmd.arg("calldata").arg(sig);
        for a in args_list { cmd.arg(a); }
        match cmd.output() {
            Ok(out) if out.status.success() => {
                let mut s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.starts_with("0x") { s = format!("0x{}", s); }
                s
            }
            Ok(out) => {
                eprintln!("cast calldata failed: {}", String::from_utf8_lossy(&out.stderr));
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Failed to execute cast: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        "0x".to_string()
    };

    let tx = SimTx {
        from: from_address,
        to: Some(to_address),
        data: Some(data_hex),
        gas: None,
        gas_price: None,
        value: Some(value_hex),
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        gas_limit: None,
    };

    let mut simulator = match TenderlySimulator::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("config error: {}", e);
            std::process::exit(1);
        }
    };

    let out_path = env::var("OUTPUT_FILE").unwrap_or_else(|_| "tenderly_output.txt".to_string());
    match simulator.simulate_transaction(&tx, None).await {
        Ok(res) => {
            let tenderly = simulator.last_result();
            let contracts = tenderly.as_ref().map(|t| &t.contracts);
            let asset_changes = tenderly
                .as_ref()
                .and_then(|t| t.transaction.transaction_info.asset_changes.clone());

            let output = serde_json::json!({
                "summary": {
                    "uuid": res.summary.uuid,
                    "reverted": res.summary.reverted,
                    "gas_used": res.summary.gas_used,
                    "error_message": res.summary.error_message,
                    "gas_limit": res.tx.gas_limit,
                },
                "tenderly": {
                    "contracts": contracts,
                },
                "asset_changes": asset_changes.unwrap_or_default(),
            });
            let body = serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
            if let Err(err) = fs::write(&out_path, body) {
                eprintln!("failed to write {}: {}", out_path, err);
                std::process::exit(1);
            }
            println!("wrote {}", out_path);
        }
        Err(e) => {
            let body = format!("simulation failed: {}", e);
            if let Err(err) = fs::write(&out_path, body) {
                eprintln!("failed to write {}: {}", out_path, err);
                std::process::exit(1);
            }
            println!("wrote {} (error)", out_path);
        }
    }
}


