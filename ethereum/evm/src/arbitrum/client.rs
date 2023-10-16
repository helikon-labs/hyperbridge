use crate::{abi::i_rollup::*, derive_map_key, EvmClient, EvmConfig};
use anyhow::anyhow;
use consensus_client::{
	arbitrum::{ArbitrumPayloadProof, CodecHeader, GlobalState as RustGlobalState},
	presets::NODES_SLOT,
};
use ethabi::ethereum_types::U256;
use ethers::{
	prelude::Provider,
	providers::{Middleware, Ws},
	types::{H160, H256},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tesseract_primitives::IsmpProvider;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbConfig {
	/// WS URL url for beacon execution client
	pub beacon_execution_ws: String,
	/// RollupCore contract address on L1
	pub rollup_core: H160,
	/// General evm config
	#[serde[flatten]]
	pub evm_config: EvmConfig,
}

impl ArbConfig {
	/// Convert the config into a client.
	pub async fn into_client<C: IsmpProvider>(
		self,
		counterparty: &C,
	) -> anyhow::Result<EvmClient<ArbHost>> {
		let host = ArbHost::new(&self).await?;
		let client = EvmClient::new(host, self.evm_config, counterparty).await?;

		Ok(client)
	}
}

#[derive(Clone)]
pub struct ArbHost {
	/// Arbitrum execution client
	pub(crate) arb_execution_client: Arc<Provider<Ws>>,
	/// Beacon execution client
	pub(crate) beacon_execution_client: Arc<Provider<Ws>>,
	/// Rollup core contract address
	pub(crate) rollup_core: H160,
	/// Config
	pub config: ArbConfig,
}

impl ArbHost {
	pub async fn new(config: &ArbConfig) -> Result<Self, anyhow::Error> {
		let provider =
			Provider::<Ws>::connect_with_reconnects(&config.evm_config.execution_ws, 1000).await?;
		let beacon_client =
			Provider::<Ws>::connect_with_reconnects(&config.beacon_execution_ws, 1000).await?;
		Ok(Self {
			arb_execution_client: Arc::new(provider),
			beacon_execution_client: Arc::new(beacon_client),
			rollup_core: config.rollup_core,
			config: config.clone(),
		})
	}

	async fn fetch_header(&self, block: H256) -> Result<CodecHeader, anyhow::Error> {
		let block = self
			.arb_execution_client
			.get_block(block)
			.await?
			.ok_or_else(|| anyhow!("Header not found for {:?}", block))?;
		let arb_header = CodecHeader {
			parent_hash: block.parent_hash,
			uncle_hash: block.uncles_hash,
			coinbase: block.author.unwrap_or_default(),
			state_root: block.state_root,
			transactions_root: block.transactions_root,
			receipts_root: block.receipts_root,
			logs_bloom: block.logs_bloom.unwrap_or_default(),
			difficulty: block.difficulty,
			number: block.number.unwrap_or_default().as_u64().into(),
			gas_limit: block.gas_limit.low_u64(),
			gas_used: block.gas_used.low_u64(),
			timestamp: block.timestamp.low_u64(),
			extra_data: block.extra_data.0.into(),
			mix_hash: block.mix_hash.unwrap_or_default(),
			nonce: block.nonce.unwrap_or_default(),
			base_fee_per_gas: block.base_fee_per_gas,
		};

		Ok(arb_header)
	}

	pub async fn latest_event(
		&self,
		from: u64,
		to: u64,
	) -> Result<Option<NodeCreatedFilter>, anyhow::Error> {
		let client = Arc::new(self.beacon_execution_client.clone());
		let contract = IRollup::new(self.rollup_core, client);
		let mut events = contract
			.event::<NodeCreatedFilter>()
			.address(self.rollup_core.into())
			.from_block(from)
			.to_block(to)
			.query()
			.await?
			.into_iter()
			.collect::<Vec<_>>();

		events.sort_unstable_by(|a, b| a.node_num.cmp(&b.node_num));

		Ok(events.last().cloned())
	}

	pub async fn fetch_arbitrum_payload(
		&self,
		at: u64,
		event: NodeCreatedFilter,
	) -> Result<ArbitrumPayloadProof, anyhow::Error> {
		let mut node_num = [0u8; 32];
		U256::from(event.node_num).to_big_endian(&mut node_num);
		let state_hash_key = derive_map_key(node_num.to_vec(), NODES_SLOT as u64);
		let proof = self
			.beacon_execution_client
			.get_proof(self.rollup_core, vec![state_hash_key], Some(at.into()))
			.await?;
		let arb_block_hash = event.assertion.after_state.global_state.bytes_32_vals[0].into();
		let arbitrum_header = self.fetch_header(arb_block_hash).await?;
		let payload = ArbitrumPayloadProof {
			arbitrum_header,
			global_state: RustGlobalState {
				block_hash: arb_block_hash,
				send_root: event.assertion.after_state.global_state.bytes_32_vals[1].into(),
				inbox_position: event.assertion.after_state.global_state.u_64_vals[0],
				position_in_message: event.assertion.after_state.global_state.u_64_vals[1],
			},
			machine_status: {
				let status = event.assertion.after_state.machine_status;
				status.try_into().map_err(|e| anyhow!("{:?}", e))?
			},
			inbox_max_count: event.inbox_max_count,
			node_number: event.node_num,
			storage_proof: proof
				.storage_proof
				.get(0)
				.cloned()
				.ok_or_else(|| anyhow!("Storage proof not found for arbitrum state_hash"))?
				.proof
				.into_iter()
				.map(|node| node.0.into())
				.collect(),
			contract_proof: proof.account_proof.into_iter().map(|node| node.0.into()).collect(),
		};

		Ok(payload)
	}
}
