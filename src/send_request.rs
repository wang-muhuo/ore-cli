use std::time::Duration;

use colored::*;
use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::RpcSendTransactionConfig,
};
use solana_program::{
    instruction::Instruction,
    native_token::{lamports_to_sol, sol_to_lamports},
};
use solana_rpc_client::spinner;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::Miner;

const MIN_SOL_BALANCE: f64 = 0.005;

const RPC_RETRIES: usize = 0;
const _SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 150;
const CONFIRM_RETRIES: usize = 1;

const CONFIRM_DELAY: u64 = 0;
const GATEWAY_DELAY: u64 = 300;

pub enum ComputeBudget {
    Dynamic,
    Fixed(u32),
}

impl Miner {


    pub async fn send_request(
        &self,
        ixs: &[Instruction],
        compute_budget: ComputeBudget,
        skip_confirm: bool,
        best_diff: u32,
    ) -> ClientResult<Signature> {
        let progress_bar = spinner::new_progress_bar();
        let signer = self.signer();
        let client = self.rpc_client.clone();
        let fee_payer = self.fee_payer();


        
	    
        // Return error, if balance is zero
        if let Ok(balance) = client.get_balance(&fee_payer.pubkey()).await {
            if balance <= sol_to_lamports(MIN_SOL_BALANCE) {
                panic!(
                    "{} Insufficient balance: {} SOL\nPlease top up with at least {} SOL",
                    "ERROR".bold().red(),
                    lamports_to_sol(balance),
                    MIN_SOL_BALANCE
                );
            }
        }

        // Set compute units
        let mut final_ixs = vec![];
        match compute_budget {
            ComputeBudget::Dynamic => {
                // TODO simulate
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(1_400_000))
            }
            ComputeBudget::Fixed(cus) => {
                final_ixs.push(ComputeBudgetInstruction::set_compute_unit_limit(cus))
            }
        }

        let priority_fee = match &self.dynamic_fee_url {
            Some(_) => {
                self.dynamic_fee().await
            }
            None => {
                self.priority_fee.unwrap_or(0)
            }
        };

        final_ixs.push(ComputeBudgetInstruction::set_compute_unit_price(priority_fee));
        final_ixs.extend_from_slice(ixs);

        // Build tx
        let send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Confirmed),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(RPC_RETRIES),
            min_context_slot: None,
        };
        let mut tx = Transaction::new_with_payer(&final_ixs, Some(&fee_payer.pubkey()));

        // Sign tx
        let (hash, _slot) = client
            .get_latest_blockhash_with_commitment(self.rpc_client.commitment())
            .await
            .unwrap();

        
        if signer.pubkey() == fee_payer.pubkey() {
            tx.sign(&[&signer], hash);
        } else {
            tx.sign(&[&signer, &fee_payer], hash);
        }

	    
        // Submit tx
        let mut attempts = 0;
        loop {

            let message = match &self.dynamic_fee_url {
                Some(_) => format!("Submitting transaction... (attempt {} with dynamic priority fee of {} via {})", attempts, priority_fee, self.dynamic_fee_strategy.as_ref().unwrap()),
                None => format!("Submitting transaction... (attempt {} with static priority fee of {})", attempts, priority_fee),
            };

            progress_bar.set_message(message);

            match client.send_transaction_with_config(&tx, send_cfg).await {
                Ok(sig) => {
                    // Skip confirmation
                    if skip_confirm {
			    return Err(ClientError {
		                    request: None,
		                    kind: ClientErrorKind::Custom("难度值小于20不提交!!!".into()),
		        	});
                    }

                    // Confirm the tx landed
                    for _ in 0..CONFIRM_RETRIES {
                        std::thread::sleep(Duration::from_millis(CONFIRM_DELAY));
                        match client.get_signature_statuses(&[sig]).await {
                            Ok(signature_statuses) => {
                                for status in signature_statuses.value {
                                    if let Some(status) = status {
                                        if let Some(err) = status.err {
                                            progress_bar.finish_with_message(format!(
                                                "{}: {}",
                                                "ERROR".bold().red(),
                                                err
                                            ));
                                            return Err(ClientError {
                                                request: None,
                                                kind: ClientErrorKind::Custom(err.to_string()),
                                            });
                                        }
                                        if let Some(confirmation) = status.confirmation_status {
                                            match confirmation {
                                                TransactionConfirmationStatus::Processed => {}
                                                TransactionConfirmationStatus::Confirmed
                                                | TransactionConfirmationStatus::Finalized => {
                                                    progress_bar.finish_with_message(format!(
                                                        "{} {}",
                                                        "OK".bold().green(),
                                                        sig
                                                    ));
                                                    return Ok(sig);
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Handle confirmation errors
                            Err(err) => {
                                progress_bar.set_message(format!(
                                    "{}: {}",
                                    "ERROR".bold().red(),
                                    err.kind().to_string()
                                ));
                            }
                        }
                    }
                }

                // Handle submit errors
                Err(err) => {
                    progress_bar.set_message(format!(
                        "{}: {}",
                        "ERROR".bold().red(),
                        err.kind().to_string()
                    ));
                }
            }

            // Retry
            std::thread::sleep(Duration::from_millis(GATEWAY_DELAY));
            attempts += 1;
            if attempts > GATEWAY_RETRIES {
                progress_bar.finish_with_message(format!("{}: Max retries", "ERROR".bold().red()));
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Max retries".into()),
                });
            }
        }
    }
}
