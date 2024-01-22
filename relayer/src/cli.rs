// Copyright (C) Polytope Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Tesseract CLI utilities

use crate::{
	config::{AnyClient, HyperbridgeConfig},
	logging,
	tx_payment::Subcommand,
};
use anyhow::anyhow;
use clap::Parser;
use ismp::host::{Ethereum, StateMachine};
use primitives::{IsmpHost, IsmpProvider, NonceProvider};
use std::{collections::HashMap, sync::Arc};
use tesseract_beefy::BeefyHost;
use tesseract_substrate::{
	config::{Blake2SubstrateChain, KeccakSubstrateChain},
	SubstrateClient,
};
use tesseract_sync_committee::L2Host;
use transaction_payment::TransactionPayment;

/// CLI interface for tesseract relayer.
#[derive(Parser, Debug)]
pub struct Cli {
	#[command(subcommand)]
	pub subcommand: Option<Subcommand>,
	/// Path to the relayer config file
	#[arg(short, long)]
	config: String,

	/// Should we initialize the relevant consensus states on Eth chains?
	#[arg(short, long)]
	setup_eth: bool,

	/// Should we initialize the relevant Consensus state on hyperbridge?
	#[arg(short, long)]
	setup_para: bool,
}

impl Cli {
	/// Run the relayer
	pub async fn run(self) -> Result<(), anyhow::Error> {
		logging::setup();
		log::info!("Initializing tesseract");

		let config = {
			let toml = tokio::fs::read_to_string(&self.config).await?;
			HyperbridgeConfig::parse_conf(toml).await?
		};

		let tx_payment = Arc::new(TransactionPayment::initialize().await?);

		let HyperbridgeConfig { hyperbridge: hyperbridge_config, relayer, .. } = config.clone();
		let mut hyperbridge = hyperbridge_config
			.clone()
			.into_client::<Blake2SubstrateChain, KeccakSubstrateChain>()
			.await?;

		let hyperbridge_nonce_provider = hyperbridge.initialize_nonce().await?;
		hyperbridge.set_nonce_provider(hyperbridge_nonce_provider.clone());

		let (client_map, nonce_providers) = create_client_map(config.clone(), &hyperbridge).await?;

		// set up initial consensus states
		if self.setup_eth || self.setup_para {
			initialize_consensus_clients(
				&hyperbridge,
				&client_map,
				self.setup_eth,
				self.setup_para,
			)
			.await?;
			log::info!("Initialized consensus states");
		}

		let mut processes = vec![];
		if relayer.consensus {
			// consensus streams
			for (_, client) in client_map {
				processes.push(tokio::spawn(consensus::relay(
					hyperbridge.clone(),
					client,
					true,
					true,
				)));
			}
			log::info!("Initialized consensus streams");
		}

		if relayer.messaging {
			let (clients, _) = create_client_map(config.clone(), &hyperbridge).await?;
			// messaging streams
			for (state_machine, mut client) in clients {
				let mut hyperbridge = hyperbridge_config
					.clone()
					.into_client::<Blake2SubstrateChain, KeccakSubstrateChain>()
					.await?;
				hyperbridge.set_nonce_provider(hyperbridge_nonce_provider.clone());
				client.set_nonce_provider(
					nonce_providers
						.get(&state_machine)
						.cloned()
						.ok_or_else(|| anyhow!("Expected Nonce Provider"))?,
				);
				processes.push(tokio::spawn(messaging::relay(
					hyperbridge,
					client,
					Some(relayer.clone()),
					tx_payment.clone(),
					None,
					None,
				)));
			}
			log::info!("Initialized messaging streams");
		}

		let _ = futures::future::join_all(processes).await;

		Ok(())
	}
}

type HyperbridgeChain =
	SubstrateClient<BeefyHost<Blake2SubstrateChain, KeccakSubstrateChain>, KeccakSubstrateChain>;

/// Initializes the consensus state across all connected chains.
async fn initialize_consensus_clients(
	hyperbridge: &HyperbridgeChain,
	chains: &HashMap<StateMachine, AnyClient>,
	setup_eth: bool,
	setup_para: bool,
) -> anyhow::Result<()> {
	if setup_eth {
		let initial_state = hyperbridge
			.get_initial_consensus_state()
			.await?
			.ok_or_else(|| anyhow!("Failed to fetch beef consensus state"))?;
		for (state_machine, chain) in chains {
			log::info!("setting consensus state on {state_machine:?}");
			chain.set_initial_consensus_state(initial_state.clone()).await?;
		}
	}

	if setup_para {
		for (state_machine, client) in chains {
			log::info!("setting consensus state for {state_machine:?} on hyperbridge");
			if let Some(consensus_state) = client.get_initial_consensus_state().await? {
				hyperbridge.set_initial_consensus_state(consensus_state).await?;
			}
		}
	}
	Ok(())
}

pub async fn create_client_map<C: IsmpProvider + 'static>(
	config: HyperbridgeConfig,
	hyperbridge: &C,
) -> anyhow::Result<(HashMap<StateMachine, AnyClient>, HashMap<StateMachine, NonceProvider>)> {
	let HyperbridgeConfig { chains, .. } = config.clone();

	let mut clients = HashMap::new();
	let mut nonce_providers = HashMap::new();
	let mut l2_hosts = vec![];

	for (state_machine, config) in chains {
		let mut client = config.into_client(hyperbridge).await?;
		match &client {
			AnyClient::Arbitrum(client) => l2_hosts.push(L2Host::Arb(client.host.clone())),
			AnyClient::Base(client) => l2_hosts.push(L2Host::Base(client.host.clone())),
			AnyClient::Optimism(client) => l2_hosts.push(L2Host::Op(client.host.clone())),
			_ => {},
		}
		let nonce_provider = client.initialize_nonce().await?;
		client.set_nonce_provider(nonce_provider.clone());
		clients.insert(state_machine, client);
		nonce_providers.insert(state_machine, nonce_provider);
	}

	let execution_layer = clients.get_mut(&StateMachine::Ethereum(Ethereum::ExecutionLayer));
	if let Some(exec_layer) = execution_layer {
		match exec_layer {
			AnyClient::Ethereum(client) => client.host.set_l2_hosts(l2_hosts),
			_ => unreachable!(),
		}
	}

	Ok((clients, nonce_providers))
}
